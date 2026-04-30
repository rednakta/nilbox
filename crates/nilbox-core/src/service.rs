//! NilBoxService — business logic orchestration
//!
//! Single service aggregating all features. Tauri commands delegate to this.

use crate::audit::AuditAction;
use crate::config::PortMappingConfig;
use crate::config_store::{AdminUrlRecord, AllowlistEntry, ConfigStore, FileMappingRecord, VmRecord};
use crate::events::{EventEmitter, emit_typed};
use crate::vm_install::{install_from_manifest_url, install_from_cache, VmInstallProgress, CachedImageInfo, list_cached_images};
use crate::file_proxy::FileProxy;
use crate::file_proxy::path_manager::PathState;
use crate::file_proxy::protocol::fuse_port_for_mapping;
use crate::mcp_bridge::McpServerConfig;
use crate::monitoring::{
    MonitoringCollector, VmMetrics, CpuSnapshot,
    parse_proc_stat, parse_proc_meminfo,
};
use crate::proxy::auth_router::AuthRouter;
use crate::proxy::domain_gate::{DomainGate, DomainDecision};
use nilbox_blocklist::BloomBlocklist;
use crate::proxy::token_mismatch_gate::{TokenMismatchGate, TokenMismatchDecision};
use crate::proxy::inspect::InspectCertAuthority;
use crate::proxy::reverse_proxy::ReverseProxy;
use crate::proxy::llm_detector::LlmProviderMatcher;
use crate::proxy::token_limit::TokenLimitChecker;
use crate::token_monitor::TokenUsageLogger;
use crate::recovery::RecoveryState;
use crate::ssh;
use crate::state::{CoreState, VmId, VmInfo, VmInstance};
use crate::store::{StoreItem, InstalledItem, AppInstallOutput, AppInstallDone};
use crate::store::auth::AuthStatus;
use crate::validate;
use crate::vm_platform::{VmConfig, VmPlatform, VmStatus};
use crate::vsock::async_adapter::wrap_virtual_stream;
use crate::vsock::VsockStream as _;
#[cfg(target_os = "macos")]
use crate::vsock::apple::AppleVirtConnector;
#[cfg(target_os = "macos")]
use crate::vsock::VsockConnector;

#[cfg(target_os = "windows")]
use crate::vsock::named_pipe::WindowsSocketConnector;
#[cfg(target_os = "windows")]
use crate::vsock::VsockConnector;
#[cfg(target_os = "windows")]
use crate::vm_platform::qemu::QEMU_VSOCK_PORT;

#[cfg(target_os = "linux")]
use crate::vsock::linux_vsock::LinuxVhostConnector;
#[cfg(target_os = "linux")]
use crate::vsock::VsockConnector;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use tokio::sync::RwLock;
use anyhow::{Context, Result, anyhow};
use tracing::{debug, warn, error};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

// ── Timeout & interval constants ──
const LISTENER_READY_DELAY_MS: u64 = 100;
const VMM_STARTUP_TIMEOUT_SECS: u64 = 120;
const METRICS_INTERVAL_SECS: u64 = 15;
const FILE_UNMOUNT_TIMEOUT_SECS: u64 = 30;

// ── Reserved VM ports (cannot be used as vm_port in port mappings) ──
// These TCP ports inside the VM are used internally by nilbox
const RESERVED_VM_PORTS: &[(u16, &str)] = &[
    (22,    "SSH"),
    (18088, "outbound proxy"),
];

/// Commands sent to a shell session's background task.
enum ShellCommand {
    Write(Arc<Vec<u8>>),
    Resize { cols: u32, rows: u32 },
    Close,
}

/// Filesystem info from `df -m /` inside the VM.
#[derive(Debug, serde::Serialize)]
pub struct VmFsInfo {
    pub device: String,
    pub total_mb: u64,
    pub used_mb: u64,
    pub avail_mb: u64,
    pub use_pct: u64,
}

/// A pending app install waiting for the next SSH shell session to pick it up.
#[derive(Debug, Clone)]
struct PendingInstall {
    #[allow(dead_code)]
    manifest_url: String,
}

pub struct NilBoxService {
    pub state: Arc<CoreState>,
    pub emitter: Arc<dyn EventEmitter>,
    shell_sessions: Arc<RwLock<HashMap<u64, (String, tokio::sync::mpsc::Sender<ShellCommand>)>>>,
    pending_installs: Arc<tokio::sync::Mutex<HashMap<VmId, VecDeque<PendingInstall>>>>,
    pending_mapping_removals: Arc<tokio::sync::Mutex<HashSet<i64>>>,
    /// Shared CDP browser handle — one Chrome instance reused across all connected VMs.
    cdp_handle: Arc<std::sync::Mutex<CdpBrowserHandle>>,
    /// Number of VMs currently connected via VSOCK.
    vsock_connected: Arc<std::sync::atomic::AtomicU32>,
}

impl NilBoxService {
    pub fn new(state: Arc<CoreState>, emitter: Arc<dyn EventEmitter>) -> Self {
        Self {
            state,
            emitter,
            shell_sessions: Arc::new(RwLock::new(HashMap::new())),
            pending_installs: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            pending_mapping_removals: Arc::new(tokio::sync::Mutex::new(HashSet::new())),
            cdp_handle: Arc::new(std::sync::Mutex::new(CdpBrowserHandle::new())),
            vsock_connected: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        }
    }

    /// Returns the SSH keypair, initializing it on first call.
    /// Initialization triggers the macOS Keychain prompt — deferred until
    /// the window is visible so the dialog does not appear before the UI.
    async fn get_ssh_keys(&self) -> Result<(Arc<russh::keys::PrivateKey>, String)> {
        let pair = self.state.ssh_keys
            .get_or_try_init(|| {
                crate::ssh::keys::ensure_keypair(
                    self.state.keystore.as_ref(),
                    &self.state.app_data_dir,
                )
            })
            .await?;
        Ok((pair.0.clone(), pair.1.clone()))
    }

    /// Run token usage aggregation and cleanup maintenance.
    /// - Aggregates raw logs → daily table (for past days missing daily records)
    /// - Aggregates daily → weekly table (for completed weeks with no weekly record)
    /// - Finalizes daily → monthly table (for completed months)
    /// - Deletes raw logs older than 60 days
    pub async fn run_token_usage_maintenance(&self) {
        use chrono::{Duration, Local, Datelike};

        let today = Local::now().date_naive();
        let yesterday = today - Duration::days(1);
        let sixty_days_ago = today - Duration::days(60);

        // 1. Daily catch-up: aggregate missing dates from raw logs → daily table
        let from_str = sixty_days_ago.format("%Y-%m-%d").to_string();
        let to_str   = yesterday.format("%Y-%m-%d").to_string();
        match self.state.config_store.get_dates_needing_daily_aggregation(&from_str, &to_str) {
            Ok(dates) => {
                for date in &dates {
                    if let Err(e) = self.state.config_store.aggregate_daily_from_logs(date) {
                        warn!("[token_maintenance] daily agg failed for {}: {}", date, e);
                    } else {
                        debug!("[token_maintenance] aggregated daily: {}", date);
                    }
                }
                if !dates.is_empty() {
                    debug!("[token_maintenance] aggregated {} missing daily records", dates.len());
                }
            }
            Err(e) => warn!("[token_maintenance] failed to find missing daily dates: {}", e),
        }

        // 2. Weekly catch-up: aggregate completed weeks that have no weekly record
        // Iterate Sundays from (60 days ago) up to last completed Sunday (before today)
        let days_since_sunday = today.weekday().num_days_from_sunday();
        let last_sunday = today - Duration::days(days_since_sunday as i64);
        // last_sunday is start of current week — we want completed weeks, so go to previous Sunday
        let last_completed_sunday = last_sunday - Duration::days(7);

        let mut week_start = sixty_days_ago - Duration::days(sixty_days_ago.weekday().num_days_from_sunday() as i64);
        while week_start <= last_completed_sunday {
            let week_end = week_start + Duration::days(6);
            let ws_str = week_start.format("%Y-%m-%d").to_string();
            let we_str = week_end.format("%Y-%m-%d").to_string();
            if let Err(e) = self.state.config_store.aggregate_weekly_from_daily(&ws_str, &we_str) {
                warn!("[token_maintenance] weekly agg failed for {}: {}", ws_str, e);
            }
            week_start += Duration::days(7);
        }

        // 3. Monthly catch-up: finalize completed months from daily data
        // Finalize previous 2 months using proper month subtraction
        let mut year  = today.year();
        let mut month = today.month() as i32;
        for _ in 0..2 {
            month -= 1;
            if month == 0 {
                month = 12;
                year -= 1;
            }
            let ym = format!("{:04}-{:02}", year, month);
            if let Err(e) = self.state.config_store.finalize_monthly_from_daily(&ym) {
                warn!("[token_maintenance] monthly finalize failed for {}: {}", ym, e);
            }
        }

        // 4. Delete raw logs older than 60 days
        match self.state.config_store.delete_old_token_usage_logs(60) {
            Ok(n) if n > 0 => debug!("[token_maintenance] deleted {} old token logs (>60 days)", n),
            Ok(_) => {}
            Err(e) => warn!("[token_maintenance] failed to delete old logs: {}", e),
        }
    }

    /// Load blocklist.bin from app_data_dir at startup.
    /// Non-fatal: logs a warning and continues if the file is missing or corrupt.
    pub async fn load_blocklist_on_startup(&self) {
        let _ = self.state.config_store.delete_old_block_logs();
        let path = self.state.app_data_dir.join("blocklist").join("blocklist.bin");
        if !path.exists() {
            return;
        }
        match std::fs::read(&path) {
            Ok(data) => {
                match BloomBlocklist::load(&data, false) {
                    Ok(bl) => {
                        debug!("blocklist loaded: {} domains, timestamp={}", bl.domain_count(), bl.build_timestamp());
                        self.state.domain_gate.set_blocklist(Some(std::sync::Arc::new(bl))).await;
                    }
                    Err(e) => {
                        tracing::warn!("blocklist load failed: {}", e);
                    }
                }
            }
            Err(e) => {
                tracing::warn!("blocklist read failed: {}", e);
            }
        }
    }

    /// Reload blocklist from disk (called by reload_blocklist Tauri command).
    pub async fn reload_blocklist(&self) -> Result<nilbox_blocklist::BlocklistInfo> {
        let path = self.state.app_data_dir.join("blocklist").join("blocklist.bin");
        let data = std::fs::read(&path)
            .map_err(|e| anyhow::anyhow!("blocklist file not found: {}", e))?;
        let bl = BloomBlocklist::load(&data, false)?;
        let info = nilbox_blocklist::BlocklistInfo::from(&bl);
        debug!("blocklist reloaded: {} domains", bl.domain_count());
        self.state.domain_gate.set_blocklist(Some(std::sync::Arc::new(bl))).await;
        Ok(info)
    }

    /// Remove domain_token_accounts entries whose token no longer exists in the keystore.
    /// Call once at startup to clean up after e.g. keys.db deletion.
    pub async fn cleanup_orphan_tokens(&self) {
        let accounts = match self.state.config_store.all_domain_token_accounts() {
            Ok(a) => a,
            Err(e) => {
                error!("Failed to list domain token accounts for orphan cleanup: {}", e);
                return;
            }
        };
        if accounts.is_empty() {
            return;
        }

        let mut orphans = Vec::new();
        for account in &accounts {
            match self.state.keystore.get(account).await {
                Ok(_) => {} // exists in keystore
                Err(_) => orphans.push(account.clone()),
            }
        }

        if orphans.is_empty() {
            return;
        }

        debug!("Cleaning up {} orphan domain token account(s)", orphans.len());
        match self.state.config_store.delete_domain_tokens_by_accounts(&orphans) {
            Ok(n) => debug!("Removed {} orphan domain token record(s)", n),
            Err(e) => warn!("Failed to clean up orphan tokens: {}", e),
        }
    }

    // ── Multi-VM ──────────────────────────────────────────────

    /// Register a VM from DB record with its existing (stable) ID.
    /// Used at startup to re-register persisted VMs.
    pub async fn register_vm(&self, record: &VmRecord) -> Result<VmId> {
        let id = record.id.clone();
        let vm_config = VmConfig {
            disk_image: std::path::PathBuf::from(&record.disk_image),
            kernel: record.kernel.as_ref().map(|s| std::path::PathBuf::from(s)),
            initrd: record.initrd.as_ref().map(|s| std::path::PathBuf::from(s)),
            append: record.append.clone(),
            memory_mb: record.memory_mb,
            cpus: record.cpus,
        };

        #[cfg(target_os = "macos")]
        let platform: Box<dyn VmPlatform> =
            Box::new(crate::vm_platform::apple::AppleVmPlatform::new([0u8; 32]));
        #[cfg(not(target_os = "macos"))]
        let platform: Box<dyn VmPlatform> = match self.state.qemu_binary_path.clone() {
            Some(p) => Box::new(crate::vm_platform::qemu::QemuVmPlatform::with_binary_path(p)),
            None    => Box::new(crate::vm_platform::qemu::QemuVmPlatform::new()),
        };

        let instance = Arc::new(VmInstance {
            id: id.clone(),
            name: RwLock::new(record.name.clone()),
            description: RwLock::new(record.description.clone()),
            last_boot_at: RwLock::new(record.last_boot_at.clone()),
            created_at: record.created_at.clone(),
            memory_mb: RwLock::new(record.memory_mb),
            cpus: RwLock::new(record.cpus),
            base_os: record.base_os.clone(),
            base_os_version: record.base_os_version.clone(),
            target_platform: record.target_platform.clone(),
            admin_urls: RwLock::new(
                self.state.config_store.list_vm_admin_urls(&id).unwrap_or_else(|e| {
                    warn!("Failed to load admin URLs for VM {}: {}", id, e);
                    vec![]
                })
            ),
            platform: RwLock::new(platform),
            multiplexer: RwLock::new(None),
            ssh_client: RwLock::new(None),
            file_proxies: RwLock::new(std::collections::HashMap::new()),
            metrics_cancel: RwLock::new(tokio_util::sync::CancellationToken::new()),
            disk_image: record.disk_image.clone(),
        });

        instance.platform.write().await.create(vm_config).await?;
        self.state.vms.write().await.insert(id.clone(), instance);

        // Auto-select if first VM or if this is the default (atomic write lock)
        {
            let mut active = self.state.active_vm.write().await;
            if active.is_none() || record.is_default {
                *active = Some(id.clone());
            }
        }

        debug!("VM registered: {} ({})", record.name, id);
        Ok(id)
    }

    /// Create a new VM instance and return its ID.
    pub async fn create_vm(&self, name: String, vm_config: VmConfig) -> Result<VmId> {
        let id = uuid::Uuid::new_v4().to_string();

        // Persist to DB
        let record = VmRecord {
            id: id.clone(),
            name: name.clone(),
            disk_image: vm_config.disk_image.to_string_lossy().to_string(),
            kernel: vm_config.kernel.as_ref().map(|p| p.to_string_lossy().to_string()),
            initrd: vm_config.initrd.as_ref().map(|p| p.to_string_lossy().to_string()),
            append: vm_config.append.clone(),
            memory_mb: vm_config.memory_mb,
            cpus: vm_config.cpus,
            is_default: true,
            description: None,
            last_boot_at: None,
            created_at: String::new(),
            admin_url: None,
            admin_label: None,
            base_os: None,
            base_os_version: None,
            target_platform: None,
        };
        let created_at = self.state.config_store.insert_vm(&record, &[])?;
        self.state.config_store.set_default_vm(&id)?;

        #[cfg(target_os = "macos")]
        let platform: Box<dyn VmPlatform> =
            Box::new(crate::vm_platform::apple::AppleVmPlatform::new([0u8; 32]));
        #[cfg(not(target_os = "macos"))]
        let platform: Box<dyn VmPlatform> = match self.state.qemu_binary_path.clone() {
            Some(p) => Box::new(crate::vm_platform::qemu::QemuVmPlatform::with_binary_path(p)),
            None    => Box::new(crate::vm_platform::qemu::QemuVmPlatform::new()),
        };

        let instance = Arc::new(VmInstance {
            id: id.clone(),
            name: RwLock::new(name.clone()),
            description: RwLock::new(None),
            last_boot_at: RwLock::new(None),
            created_at,
            memory_mb: RwLock::new(vm_config.memory_mb),
            cpus: RwLock::new(vm_config.cpus),
            base_os: None,
            base_os_version: None,
            target_platform: None,
            admin_urls: RwLock::new(vec![]),
            platform: RwLock::new(platform),
            multiplexer: RwLock::new(None),
            ssh_client: RwLock::new(None),
            file_proxies: RwLock::new(std::collections::HashMap::new()),
            metrics_cancel: RwLock::new(tokio_util::sync::CancellationToken::new()),
            disk_image: vm_config.disk_image.to_string_lossy().to_string(),
        });

        instance.platform.write().await.create(vm_config).await?;
        self.state.vms.write().await.insert(id.clone(), instance);

        // Auto-select if first VM (atomic write lock)
        {
            let mut active = self.state.active_vm.write().await;
            if active.is_none() {
                *active = Some(id.clone());
            }
        }

        emit_typed(&self.emitter, "vm-created", &serde_json::json!({ "id": &id, "name": &name }));
        debug!("VM created: {} ({})", name, id);
        Ok(id)
    }

    /// Delete a VM instance.
    pub async fn delete_vm(&self, id: &VmId) -> Result<()> {
        let instance = self.state.vms.write().await.remove(id)
            .ok_or_else(|| anyhow!("VM not found: {}", id))?;

        // Cancel background metrics collection before stopping
        instance.metrics_cancel.read().await.cancel();

        let mut platform = instance.platform.write().await;
        let status = platform.status_async().await;
        if status == VmStatus::Running || status == VmStatus::Starting {
            platform.stop().await?;
        }
        drop(platform);

        let mut active = self.state.active_vm.write().await;
        if active.as_ref() == Some(id) {
            *active = None;
        }
        drop(active);

        // Collect file paths before DB deletion
        let vm_record = self.state.config_store.get_vm(id)
            .unwrap_or_else(|e| {
                warn!("Failed to get VM record for {}: {}", id, e);
                None
            });

        // Delete from DB (CASCADE deletes port_mappings, file_mappings, admin_urls)
        self.state.config_store.delete_vm(id)?;

        // Delete VM files from filesystem
        let vm_dir = self.state.app_data_dir.join("vms").join(id.as_str());
        match tokio::fs::remove_dir_all(&vm_dir).await {
            Ok(_) => debug!("Deleted VM directory: {}", vm_dir.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Not a store-installed VM — delete individual files
                if let Some(ref record) = vm_record {
                    match tokio::fs::remove_file(&record.disk_image).await {
                        Ok(_) => debug!("Deleted disk image: {}", record.disk_image),
                        Err(e) => warn!("Failed to delete disk image {}: {}", record.disk_image, e),
                    }
                    match self.state.config_store.list_vms() {
                        Ok(all_vms) => {
                            let remaining: std::collections::HashSet<String> = all_vms
                                .into_iter()
                                .flat_map(|r| {
                                    let mut v = vec![r.disk_image];
                                    v.extend(r.kernel);
                                    v.extend(r.initrd);
                                    v
                                })
                                .collect();
                            if let Some(ref k) = record.kernel {
                                if !remaining.contains(k) {
                                    match tokio::fs::remove_file(k).await {
                                        Ok(_) => debug!("Deleted kernel: {}", k),
                                        Err(e) => warn!("Failed to delete kernel {}: {}", k, e),
                                    }
                                }
                            }
                            if let Some(ref ini) = record.initrd {
                                if !remaining.contains(ini) {
                                    match tokio::fs::remove_file(ini).await {
                                        Ok(_) => debug!("Deleted initrd: {}", ini),
                                        Err(e) => warn!("Failed to delete initrd {}: {}", ini, e),
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!("Failed to list VMs for shared file check: {} — skipping kernel/initrd cleanup to avoid deleting shared files", e);
                        }
                    }
                }
            }
            Err(e) => warn!("Failed to remove VM directory {}: {}", vm_dir.display(), e),
        }

        emit_typed(&self.emitter, "vm-deleted", &serde_json::json!({ "id": id }));
        debug!("VM deleted: {}", id);
        Ok(())
    }

    /// Get the current disk image size in bytes for a VM.
    pub async fn get_vm_disk_size(&self, vm_id: &VmId) -> Result<u64> {
        let record = self.state.config_store.get_vm(vm_id)?
            .ok_or_else(|| anyhow!("VM not found: {}", vm_id))?;
        let meta = std::fs::metadata(&record.disk_image)
            .map_err(|e| anyhow!("Failed to stat disk image: {}", e))?;
        Ok(meta.len())
    }

    /// Filesystem info from inside the VM via Control Port.
    pub async fn get_vm_fs_info(&self, vm_id: &VmId) -> Result<VmFsInfo> {
        let vms = self.state.vms.read().await;
        let instance = vms.get(vm_id).ok_or_else(|| anyhow!("VM not registered"))?;
        let mux = instance.multiplexer.read().await;
        let multiplexer = mux.as_ref()
            .ok_or_else(|| anyhow!("VM is not running or multiplexer not ready"))?;

        let ctrl = crate::control_client::ControlClient::new(Arc::clone(multiplexer));
        let info = ctrl.get_fs_info("/").await?;
        Ok(VmFsInfo {
            device: info.device,
            total_mb: info.total_mb,
            used_mb: info.used_mb,
            avail_mb: info.avail_mb,
            use_pct: info.use_pct,
        })
    }

    /// Rewrite /etc/profile.d/nilbox-envs.sh in a running VM to reflect the
    /// current set of enabled env entries.  Returns Ok(()) if the VM multiplexer
    /// is not connected (the script will be written at next boot instead).
    /// `changed`: if Some(("VARNAME", false)) — previously used to unset that var (now ignored).
    pub async fn apply_env_injection(&self, vm_id: &VmId, changed: Option<(&str, bool)>) -> Result<()> {
        let _ = changed; // No longer used — re-sourcing env file overwrites all values
        let vms = self.state.vms.read().await;
        let instance = vms.get(vm_id).ok_or_else(|| anyhow!("VM not registered"))?;

        let mut enabled_names: Vec<String> = self.state.config_store
            .list_env_entry_overrides(vm_id)
            .unwrap_or_else(|e| {
                warn!("Failed to load env entry overrides for VM {}: {}", vm_id, e);
                vec![]
            })
            .into_iter()
            .filter(|e| e.enabled)
            .map(|e| e.name)
            .collect();

        // Load OAuth script engine for token_path → filename mapping
        let oauth_engine = crate::proxy::oauth_script_engine::OAuthScriptEngine::load_all(
            self.state.keystore.as_ref(),
            &self.state.config_store,
        ).await.unwrap_or_else(|_| crate::proxy::oauth_script_engine::OAuthScriptEngine::empty());

        // Build token_path → provider_name map for _FILE vars
        let file_var_map: std::collections::HashMap<String, String> = oauth_engine.providers()
            .map(|p| (p.info.token_path.clone(), format!("oauth_{}.json", p.info.name)))
            .collect();

        // Add OAuth providers' token_path to enabled_names for all providers with credentials
        for provider in oauth_engine.providers() {
            let oauth_key = format!("oauth:{}", provider.info.name);
            let has_creds = self.state.keystore.has(&provider.info.token_path).await.unwrap_or(false)
                || self.state.keystore.has(&oauth_key).await.unwrap_or(false);
            if has_creds {
                enabled_names.push(provider.info.token_path.clone());
            }
        }

        // Validate env var names contain only safe characters
        enabled_names.retain(|name| {
            let valid = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            if !valid {
                warn!("Skipping env var with invalid name: {:?}", name);
            }
            valid
        });

        let mut env_script = String::from("#!/bin/sh\n");
        for name in &enabled_names {
            if name.ends_with("_FILE") {
                let filename = file_var_map.get(name)
                    .cloned()
                    .unwrap_or_else(|| {
                        let base = name.trim_end_matches("_FILE").to_lowercase();
                        format!("{}.json", base)
                    });
                env_script.push_str(&format!("export {}=/etc/nilbox/{}\n", name, filename));
            } else {
                env_script.push_str(&format!("export {}={}\n", name, name));
            }
        }
        // Write env script via Control Port
        let mux = instance.multiplexer.read().await;
        if let Some(ref multiplexer) = *mux {
            let ctrl = crate::control_client::ControlClient::new(Arc::clone(multiplexer));
            ctrl.write_file(
                "/etc/profile.d/nilbox-envs.sh",
                env_script.as_bytes(),
                "0644",
                "root",
            ).await
                .map_err(|e| anyhow!("Failed to apply env injection: {}", e))?;

            // Re-inject OAuth dummy secrets for any newly enabled _FILE vars
            self.reinject_oauth_dummy_secrets(vm_id, &ctrl, &enabled_names).await;
        } else {
            warn!("Multiplexer not available for env injection");
        }

        // Inject into active shell sessions so running PTYs pick up the change immediately.
        // Use a fixed constant string — no user data flows into the PTY injection.
        self.inject_env_to_sessions(vm_id, "\n. /etc/profile.d/nilbox-envs.sh\n").await;

        Ok(())
    }

    /// Re-inject OAuth dummy secret files into a running VM for enabled _FILE env vars.
    /// Called by apply_env_injection to ensure /etc/nilbox/*.json files exist.
    async fn reinject_oauth_dummy_secrets(
        &self,
        _vm_id: &VmId,
        ctrl: &crate::control_client::ControlClient,
        enabled_names: &[String],
    ) {
        let file_vars: Vec<&String> = enabled_names.iter()
            .filter(|n| n.ends_with("_FILE"))
            .collect();
        if file_vars.is_empty() {
            return;
        }

        // Load OAuth script engine from keystore
        let oauth_engine = match crate::proxy::oauth_script_engine::OAuthScriptEngine::load_all(
            self.state.keystore.as_ref(),
            &self.state.config_store,
        ).await {
            Ok(e) => e,
            Err(e) => {
                warn!("Failed to load OAuth scripts: {}", e);
                return;
            }
        };

        for provider in oauth_engine.providers() {
            // PKCE providers don't use dummy secret files
            if provider.info.flow_type == "pkce" {
                continue;
            }
            let token_path = &provider.info.token_path;
            if !file_vars.iter().any(|n| *n == token_path) {
                continue;
            }
            // Check if real credentials exist under token_path, OAUTH_{NAME}_*, or oauth:{id}
            let has_credentials = if self.state.keystore.has(token_path).await.unwrap_or(false) {
                true
            } else {
                let oauth_key = format!("oauth:{}", provider.info.name);
                if self.state.keystore.has(&oauth_key).await.unwrap_or(false) {
                    true
                } else {
                    let prefix = format!("OAUTH_{}_", provider.info.name.to_uppercase());
                    self.state.keystore.list().await.unwrap_or_else(|e| {
                        warn!("Failed to list keystore entries: {}", e);
                        vec![]
                    }).iter()
                        .any(|k| k.starts_with(&prefix))
                }
            };
            if !has_credentials {
                continue;
            }
            match oauth_engine.call_make_dummy_secret(provider) {
                Ok(dummy_json) => {
                    let filename = format!("oauth_{}.json", provider.info.name);
                    let path = format!("/etc/nilbox/{}", filename);
                    match ctrl.write_file(&path, dummy_json.as_bytes(), "0644", "root").await {
                        Ok(_) => debug!("Re-injected OAuth dummy secret for provider: {}", provider.info.name),
                        Err(e) => warn!("Failed to re-inject OAuth dummy secret for {}: {}", provider.info.name, e),
                    }
                }
                Err(e) => warn!("make_dummy_secret() failed for {}: {}", provider.info.name, e),
            }
        }
    }

    /// Run growpart + resize2fs inside running VM, return combined output.
    ///
    /// The root device is derived from the `root=` token in VmRecord.append.
    /// - `root=/dev/vda`  (no partition) → `resize2fs /dev/vda` only
    /// - `root=/dev/vdaN` (partitioned)  → `growpart /dev/vda N` + `resize2fs /dev/vdaN`
    pub async fn expand_vm_partition(&self, vm_id: &VmId) -> Result<String> {
        // Derive root device from stored kernel cmdline
        let record = self.state.config_store.get_vm(vm_id)?
            .ok_or_else(|| anyhow!("VM not found: {}", vm_id))?;
        let root_dev = record.append.as_deref()
            .and_then(|s| s.split_whitespace().find(|t| t.starts_with("root=")))
            .map(|t| t.trim_start_matches("root="))
            .unwrap_or("/dev/vda1");   // safe default for existing partitioned images

        // Step 1: Validate device path to prevent shell injection via crafted kernel cmdline
        if !validate::is_valid_device_path(root_dev) {
            return Err(anyhow!("Invalid root device path: {}", root_dev));
        }
        debug!("VM {} expand: root_dev={}", vm_id, root_dev);

        let vms = self.state.vms.read().await;
        let instance = vms.get(vm_id).ok_or_else(|| anyhow!("VM not registered"))?;
        let mux = instance.multiplexer.read().await;
        let multiplexer = mux.as_ref()
            .ok_or_else(|| anyhow!("VM is not running or multiplexer not ready"))?;

        let ctrl = crate::control_client::ControlClient::new(Arc::clone(multiplexer));
        let output = ctrl.expand_partition(root_dev).await?;

        debug!("VM {} partition expanded:\n{}", vm_id, output);
        Ok(output)
    }

    /// Resize a stopped VM's disk image to a larger size.
    pub async fn resize_vm_disk(&self, vm_id: &VmId, new_size_gb: u32) -> Result<u64> {
        let record = self.state.config_store.get_vm(vm_id)?
            .ok_or_else(|| anyhow!("VM not found: {}", vm_id))?;

        // VM must be stopped
        let instance = {
            let vms = self.state.vms.read().await;
            vms.get(vm_id).ok_or_else(|| anyhow!("VM not registered"))?.clone()
        }; // vms 락 해제 — platform write lock이 concurrent start를 막는다
        let platform = instance.platform.write().await;
        let status = platform.status();
        if status != VmStatus::Stopped {
            return Err(anyhow!("VM must be stopped before resizing (status: {:?})", status));
        }

        let current = std::fs::metadata(&record.disk_image)?.len();
        let new_bytes = (new_size_gb as u64) * 1024 * 1024 * 1024;
        if new_bytes <= current {
            return Err(anyhow!(
                "New size ({} GB) must be larger than current ({:.1} GB)",
                new_size_gb,
                current as f64 / (1024.0_f64.powi(3))
            ));
        }

        let file = std::fs::OpenOptions::new().write(true).open(&record.disk_image)
            .map_err(|e| anyhow!("Cannot open disk image: {}", e))?;
        file.set_len(new_bytes)
            .map_err(|e| anyhow!("Failed to resize disk: {}", e))?;
        drop(platform);

        debug!("VM {} disk resized: {} -> {} bytes", vm_id, current, new_bytes);
        Ok(new_bytes)
    }

    /// Switch active VM.
    pub async fn select_vm(&self, id: &VmId) -> Result<()> {
        let vms = self.state.vms.read().await;
        if !vms.contains_key(id) {
            return Err(anyhow!("VM not found: {}", id));
        }
        drop(vms);
        *self.state.active_vm.write().await = Some(id.clone());
        emit_typed(&self.emitter, "vm-selected", &serde_json::json!({ "id": id }));
        Ok(())
    }

    /// List all VM instances.
    pub async fn list_vms(&self) -> Vec<VmInfo> {
        let vms = self.state.vms.read().await;
        let futs: Vec<_> = vms.values().map(|vm| {
            let vm = vm.clone();
            async move {
                let (name, status, ssh_ready, description, last_boot_at, memory_mb, cpus, admin_urls) = tokio::join!(
                    vm.name.read(),
                    vm.status(),
                    vm.ssh_client.read(),
                    vm.description.read(),
                    vm.last_boot_at.read(),
                    vm.memory_mb.read(),
                    vm.cpus.read(),
                    vm.admin_urls.read(),
                );
                let vm_dir = std::path::Path::new(&vm.disk_image)
                    .parent()
                    .map(|p| p.to_string_lossy().to_string());
                VmInfo {
                    id: vm.id.clone(),
                    name: name.clone(),
                    status,
                    ssh_ready: ssh_ready.is_some(),
                    description: description.clone(),
                    last_boot_at: last_boot_at.clone(),
                    created_at: vm.created_at.clone(),
                    memory_mb: *memory_mb,
                    cpus: *cpus,
                    base_os: vm.base_os.clone(),
                    base_os_version: vm.base_os_version.clone(),
                    target_platform: vm.target_platform.clone(),
                    admin_urls: admin_urls.clone(),
                    vm_dir,
                }
            }
        }).collect();
        drop(vms);
        futures::future::join_all(futs).await
    }

    /// Update the configured memory for a stopped VM.
    pub async fn update_vm_memory(&self, id: &VmId, memory_mb: u32) -> Result<()> {
        if memory_mb < 256 || memory_mb > 65536 {
            return Err(anyhow!("memory_mb must be between 256 and 65536 (got {})", memory_mb));
        }

        let vms = self.state.vms.read().await;
        let vm = vms.get(id).ok_or_else(|| anyhow!("VM not found: {}", id))?;

        let status = vm.status().await;
        if status != VmStatus::Stopped {
            return Err(anyhow!("VM must be stopped to change memory (current status: {:?})", status));
        }

        let mut record = self.state.config_store.get_vm(id)?
            .ok_or_else(|| anyhow!("VM record not found: {}", id))?;
        record.memory_mb = memory_mb;
        self.state.config_store.update_vm(&record)?;

        *vm.memory_mb.write().await = memory_mb;

        let new_config = VmConfig {
            disk_image: std::path::PathBuf::from(&record.disk_image),
            kernel: record.kernel.as_ref().map(|s| std::path::PathBuf::from(s)),
            initrd: record.initrd.as_ref().map(|s| std::path::PathBuf::from(s)),
            append: record.append.clone(),
            memory_mb,
            cpus: record.cpus,
        };
        vm.platform.write().await.create(new_config).await?;

        debug!("VM {} memory updated to {} MB", id, memory_mb);
        Ok(())
    }

    /// Update the configured CPU count for a stopped VM.
    pub async fn update_vm_cpus(&self, id: &VmId, cpus: u32) -> Result<()> {
        let max_cpus = std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(32);
        if cpus < 1 || cpus > max_cpus {
            return Err(anyhow!("cpus must be between 1 and {} (got {})", max_cpus, cpus));
        }

        let vms = self.state.vms.read().await;
        let vm = vms.get(id).ok_or_else(|| anyhow!("VM not found: {}", id))?;

        let status = vm.status().await;
        if status != VmStatus::Stopped {
            return Err(anyhow!("VM must be stopped to change CPUs (current status: {:?})", status));
        }

        let mut record = self.state.config_store.get_vm(id)?
            .ok_or_else(|| anyhow!("VM record not found: {}", id))?;
        record.cpus = cpus;
        self.state.config_store.update_vm(&record)?;

        *vm.cpus.write().await = cpus;

        let new_config = VmConfig {
            disk_image: std::path::PathBuf::from(&record.disk_image),
            kernel: record.kernel.as_ref().map(|s| std::path::PathBuf::from(s)),
            initrd: record.initrd.as_ref().map(|s| std::path::PathBuf::from(s)),
            append: record.append.clone(),
            memory_mb: record.memory_mb,
            cpus,
        };
        vm.platform.write().await.create(new_config).await?;

        debug!("VM {} CPU count updated to {}", id, cpus);
        Ok(())
    }

    /// Update the name of a VM.
    pub async fn update_vm_name(&self, id: &VmId, name: String) -> Result<()> {
        let vms = self.state.vms.read().await;
        let vm = vms.get(id).ok_or_else(|| anyhow!("VM not found: {}", id))?;

        let mut record = self.state.config_store.get_vm(id)?
            .ok_or_else(|| anyhow!("VM record not found: {}", id))?;
        record.name = name.clone();
        self.state.config_store.update_vm(&record)?;

        *vm.name.write().await = name.clone();

        debug!("VM {} name updated to '{}'", id, name);
        Ok(())
    }

    /// Update the description of a VM.
    pub async fn update_vm_description(&self, id: &VmId, description: Option<String>) -> Result<()> {
        let vms = self.state.vms.read().await;
        let vm = vms.get(id).ok_or_else(|| anyhow!("VM not found: {}", id))?;

        let mut record = self.state.config_store.get_vm(id)?
            .ok_or_else(|| anyhow!("VM record not found: {}", id))?;
        record.description = description.clone();
        self.state.config_store.update_vm(&record)?;

        *vm.description.write().await = description;

        debug!("VM {} description updated", id);
        Ok(())
    }

    /// Append a new admin URL entry for a VM.
    pub async fn add_vm_admin_url(
        &self,
        vm_id: &VmId,
        url: String,
        label: String,
    ) -> Result<i64> {
        let new_id = self.state.config_store.insert_vm_admin_url(vm_id, &url, &label)?;
        let vms = self.state.vms.read().await;
        if let Some(vm) = vms.get(vm_id) {
            vm.admin_urls.write().await.push(
                AdminUrlRecord { id: new_id, url, label }
            );
        }
        Ok(new_id)
    }

    /// Remove an admin URL entry by its DB id.
    pub async fn remove_vm_admin_url(
        &self,
        vm_id: &VmId,
        url_id: i64,
    ) -> Result<()> {
        self.state.config_store.delete_vm_admin_url(url_id)?;
        let vms = self.state.vms.read().await;
        if let Some(vm) = vms.get(vm_id) {
            vm.admin_urls.write().await.retain(|a| a.id != url_id);
        }
        Ok(())
    }

    /// Install a VM from a manifest URL (store download flow).
    ///
    /// Downloads, extracts, and registers the VM.
    /// Emits `vm-install-progress` events during the process.
    /// Returns the new VM's ID on success.
    pub async fn install_vm_from_manifest_url(&self, url: &str) -> Result<VmId> {
        let user_verified = self.extract_verified_from_token().await;
        let result = install_from_manifest_url(
            url,
            &self.state.app_data_dir,
            &self.emitter,
            &self.state.config_store,
            Some(&self.state.store_client),
            user_verified,
        )
        .await;

        match result {
            Ok(record) => {
                let vm_name = record.name.clone();
                let id = self.register_vm(&record).await?;
                emit_typed(
                    &self.emitter,
                    "vm-install-progress",
                    &VmInstallProgress {
                        stage: "complete".into(),
                        percent: 100,
                        vm_name,
                        vm_id: Some(id.clone()),
                        error: None,
                    },
                );
                debug!("VM install complete: {}", id);
                Ok(id)
            }
            Err(e) => {
                let msg = e.to_string();
                emit_typed(
                    &self.emitter,
                    "vm-install-progress",
                    &VmInstallProgress {
                        stage: "error".into(),
                        percent: 0,
                        vm_name: String::new(),
                        vm_id: None,
                        error: Some(msg.clone()),
                    },
                );
                Err(anyhow!(msg))
            }
        }
    }

    /// List locally cached OS images (no network required).
    pub fn list_cached_os_images(&self) -> Vec<CachedImageInfo> {
        list_cached_images(&self.state.app_data_dir)
    }

    /// Install a VM from local cache only — no network access.
    pub async fn install_vm_from_cache(&self, app_id: &str) -> Result<VmId> {
        let result = install_from_cache(
            app_id,
            &self.state.app_data_dir,
            &self.emitter,
            &self.state.config_store,
        );

        match result {
            Ok(record) => {
                let vm_name = record.name.clone();
                let id = self.register_vm(&record).await?;
                emit_typed(
                    &self.emitter,
                    "vm-install-progress",
                    &VmInstallProgress {
                        stage: "complete".into(),
                        percent: 100,
                        vm_name,
                        vm_id: Some(id.clone()),
                        error: None,
                    },
                );
                debug!("VM cache install complete: {}", id);
                Ok(id)
            }
            Err(e) => {
                let msg = format!("{:#}", e);
                emit_typed(
                    &self.emitter,
                    "vm-install-progress",
                    &VmInstallProgress {
                        stage: "error".into(),
                        percent: 0,
                        vm_name: String::new(),
                        vm_id: None,
                        error: Some(msg.clone()),
                    },
                );
                Err(anyhow!(msg))
            }
        }
    }

    /// Start a VM.
    pub async fn start_vm(&self, id: &VmId) -> Result<()> {
        // Deferred from startup: clean orphan tokens on first VM use
        // (triggers OS keyring access here, not at app launch)
        self.cleanup_orphan_tokens().await;

        let instance = self.get_vm(id).await?;

        // Check inbound ports are not already in use
        let port_mappings = self.state.config_store.list_port_mappings(id)?;
        // Step 8: TOCTOU note — a port can become occupied between this
        // check and the actual bind in the gateway. This is inherent to
        // pre-flight checks; the real bind will still fail-safe with an
        // OS-level EADDRINUSE, which the gateway surfaces as an error.
        let mut blocked: Vec<u16> = Vec::new();
        for pm in &port_mappings {
            if tokio::net::TcpListener::bind(
                std::net::SocketAddr::from(([127, 0, 0, 1], pm.host_port))
            ).await.is_err() {
                blocked.push(pm.host_port);
            }
        }
        if !blocked.is_empty() {
            let msg = format!(
                "Cannot start VM: port(s) already in use: {}",
                blocked.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", ")
            );
            warn!("{}", msg);
            emit_typed(&self.emitter, "vm-status-changed", &serde_json::json!({
                "id": id, "status": "Error", "error": msg
            }));
            return Err(anyhow!(msg));
        }

        // Generate agent auth token (32 bytes random) — used on ALL platforms
        let agent_auth_token: [u8; 32] = rand::random();

        // Get relay socket path before starting (platform needs read lock only)
        let relay_path = instance.platform.read().await.vsock_socket_path();

        // On macOS: bind relay socket BEFORE starting VMM so it's ready when VMM connects
        #[cfg(target_os = "macos")]
        if let Some(ref relay) = relay_path {
            let relay_token: [u8; 32] = rand::random();

            // Set the relay token on the platform so VMM receives it in the start command,
            // and inject agent auth token into kernel cmdline for vm-agent to read.
            {
                let token_hex: String = agent_auth_token.iter()
                    .map(|b| format!("{:02x}", b)).collect();
                let mut platform = instance.platform.write().await;
                if let Some(apple) = platform.as_any_mut().downcast_mut::<crate::vm_platform::apple::AppleVmPlatform>() {
                    apple.set_relay_token(relay_token);
                    // Inject agent auth token into VmConfig.append for kernel cmdline
                    apple.inject_cmdline_token(&token_hex);
                }
            }

            let connector = AppleVirtConnector::new(relay.clone(), relay_token);
            let listener = connector.listen(0).await?;
            debug!("VSOCK relay listener ready at {}", relay.display());

            // Spawn background task to accept relay connection, create multiplexer, and setup SSH
            let instance_clone = instance.clone();
            let emitter = self.emitter.clone();
            let vm_id = id.clone();
            let (ssh_private_key, ssh_public_key) = self.get_ssh_keys().await?;
            let gateway = self.state.gateway.clone();
            let config_store = self.state.config_store.clone();
            let keystore = self.state.keystore.clone();
            let domain_gate = self.state.domain_gate.clone();
            let token_mismatch_gate = self.state.token_mismatch_gate.clone();
            let auth_router = self.state.auth_router.clone();
            let monitoring = self.state.monitoring.clone();
            let inspect_ca = Arc::new(
                InspectCertAuthority::load_or_create(&keystore).await?
            );

            // Load OAuth script engine from keystore and update shared cell
            {
                let new_engine = Arc::new(
                    crate::proxy::oauth_script_engine::OAuthScriptEngine::load_all(
                        self.state.keystore.as_ref(),
                        &self.state.config_store,
                    ).await
                        .unwrap_or_else(|e| {
                            warn!("Failed to load OAuth scripts: {}, creating empty engine", e);
                            crate::proxy::oauth_script_engine::OAuthScriptEngine::empty()
                        })
                );
                *self.state.oauth_engine.write().await = new_engine;
            }
            let oauth_engine = self.state.oauth_engine.clone();
            let oauth_vault = self.state.oauth_vault.clone();
            let llm_matcher  = self.state.llm_matcher.clone();
            let store_auth = self.state.store_auth.clone();

            let agent_token = agent_auth_token;
            let cdp_handle_clone = self.cdp_handle.clone();
            let vsock_connected_clone = self.vsock_connected.clone();
            tokio::spawn(async move {
                Self::setup_vsock_and_ssh(
                    listener, agent_token, instance_clone, emitter, vm_id,
                    ssh_private_key, ssh_public_key, gateway, config_store, keystore,
                    domain_gate, token_mismatch_gate, auth_router, inspect_ca, monitoring,
                    oauth_engine, oauth_vault, llm_matcher, store_auth,
                    cdp_handle_clone, vsock_connected_clone,
                ).await;
            });

            // Give listener a moment to be ready
            tokio::time::sleep(tokio::time::Duration::from_millis(LISTENER_READY_DELAY_MS)).await;
        }

        // On Windows: inject agent auth token into QEMU fw_cfg before start
        #[cfg(target_os = "windows")]
        {
            let token_hex: String = agent_auth_token.iter()
                .map(|b| format!("{:02x}", b)).collect();
            let mut platform = instance.platform.write().await;
            if let Some(qemu) = platform.as_any_mut()
                .downcast_mut::<crate::vm_platform::qemu::QemuVmPlatform>()
            {
                qemu.inject_fw_cfg_token(&token_hex);
            }
        }

        // On Linux: inject agent auth token into QEMU fw_cfg before start
        #[cfg(target_os = "linux")]
        {
            let token_hex: String = agent_auth_token.iter()
                .map(|b| format!("{:02x}", b)).collect();
            let mut platform = instance.platform.write().await;
            if let Some(qemu) = platform.as_any_mut()
                .downcast_mut::<crate::vm_platform::qemu::QemuVmPlatform>()
            {
                qemu.inject_fw_cfg_token(&token_hex);
            }
        }

        // On Windows: bind TCP listener BEFORE starting QEMU.
        // QEMU connects to this listener as a chardev client, ensuring
        // the virtio-serial chardev is connected before the guest boots.
        #[cfg(target_os = "windows")]
        {
            let vsock_port = QEMU_VSOCK_PORT;
            let connector = WindowsSocketConnector::new(vsock_port);
            let listener = connector.listen(0).await?;

            let instance_clone = instance.clone();
            let emitter = self.emitter.clone();
            let vm_id = id.clone();
            let (ssh_private_key, ssh_public_key) = self.get_ssh_keys().await?;
            let gateway = self.state.gateway.clone();
            let config_store = self.state.config_store.clone();
            let keystore = self.state.keystore.clone();
            let domain_gate = self.state.domain_gate.clone();
            let token_mismatch_gate = self.state.token_mismatch_gate.clone();
            let auth_router = self.state.auth_router.clone();
            let monitoring = self.state.monitoring.clone();
            let inspect_ca = Arc::new(
                InspectCertAuthority::load_or_create(&keystore).await?
            );

            {
                let new_engine = Arc::new(
                    crate::proxy::oauth_script_engine::OAuthScriptEngine::load_all(
                        self.state.keystore.as_ref(),
                        &self.state.config_store,
                    ).await
                        .unwrap_or_else(|e| {
                            warn!("Failed to load OAuth scripts: {}, creating empty engine", e);
                            crate::proxy::oauth_script_engine::OAuthScriptEngine::empty()
                        })
                );
                *self.state.oauth_engine.write().await = new_engine;
            }
            let oauth_engine = self.state.oauth_engine.clone();
            let oauth_vault = self.state.oauth_vault.clone();
            let llm_matcher  = self.state.llm_matcher.clone();
            let store_auth = self.state.store_auth.clone();

            let agent_token = agent_auth_token;
            let cdp_handle_clone = self.cdp_handle.clone();
            let vsock_connected_clone = self.vsock_connected.clone();
            tokio::spawn(async move {
                Self::setup_vsock_and_ssh(
                    listener, agent_token, instance_clone, emitter, vm_id,
                    ssh_private_key, ssh_public_key, gateway, config_store, keystore,
                    domain_gate, token_mismatch_gate, auth_router, inspect_ca, monitoring,
                    oauth_engine, oauth_vault, llm_matcher, store_auth,
                    cdp_handle_clone, vsock_connected_clone,
                ).await;
            });
        }

        instance.platform.write().await.start().await?;

        // On Linux: connect to guest via AF_VSOCK (vhost-vsock-pci, CID=3, port=1024)
        #[cfg(target_os = "linux")]
        {
            let connector = LinuxVhostConnector::new();
            let listener = connector.listen(0).await?;

            let instance_clone = instance.clone();
            let emitter = self.emitter.clone();
            let vm_id = id.clone();
            let (ssh_private_key, ssh_public_key) = self.get_ssh_keys().await?;
            let gateway = self.state.gateway.clone();
            let config_store = self.state.config_store.clone();
            let keystore = self.state.keystore.clone();
            let domain_gate = self.state.domain_gate.clone();
            let token_mismatch_gate = self.state.token_mismatch_gate.clone();
            let auth_router = self.state.auth_router.clone();
            let monitoring = self.state.monitoring.clone();
            let inspect_ca = Arc::new(
                InspectCertAuthority::load_or_create(&keystore).await?
            );

            {
                let new_engine = Arc::new(
                    crate::proxy::oauth_script_engine::OAuthScriptEngine::load_all(
                        self.state.keystore.as_ref(),
                        &self.state.config_store,
                    ).await
                        .unwrap_or_else(|e| {
                            warn!("Failed to load OAuth scripts: {}, creating empty engine", e);
                            crate::proxy::oauth_script_engine::OAuthScriptEngine::empty()
                        })
                );
                *self.state.oauth_engine.write().await = new_engine;
            }
            let oauth_engine = self.state.oauth_engine.clone();
            let oauth_vault = self.state.oauth_vault.clone();
            let llm_matcher  = self.state.llm_matcher.clone();
            let store_auth = self.state.store_auth.clone();

            let agent_token = agent_auth_token;
            let cdp_handle_clone = self.cdp_handle.clone();
            let vsock_connected_clone = self.vsock_connected.clone();
            tokio::spawn(async move {
                Self::setup_vsock_and_ssh(
                    listener, agent_token, instance_clone, emitter, vm_id,
                    ssh_private_key, ssh_public_key, gateway, config_store, keystore,
                    domain_gate, token_mismatch_gate, auth_router, inspect_ca, monitoring,
                    oauth_engine, oauth_vault, llm_matcher, store_auth,
                    cdp_handle_clone, vsock_connected_clone,
                ).await;
            });
        }

        emit_typed(&self.emitter, "vm-status-changed", &serde_json::json!({
            "id": id, "status": "Starting"
        }));
        debug!("VM starting: {}", id);
        Ok(())
    }

    /// Background: accept relay connection, create multiplexer, inject SSH key, connect SSH.
    async fn setup_vsock_and_ssh(
        mut listener: Box<dyn crate::vsock::VsockListener>,
        agent_auth_token: [u8; 32],
        instance: Arc<VmInstance>,
        emitter: Arc<dyn EventEmitter>,
        vm_id: VmId,
        ssh_private_key: Arc<russh::keys::PrivateKey>,
        ssh_public_key: String,
        gateway: Arc<crate::gateway::Gateway>,
        config_store: Arc<ConfigStore>,
        keystore: Arc<dyn crate::keystore::KeyStore>,
        domain_gate: Arc<DomainGate>,
        token_mismatch_gate: Arc<TokenMismatchGate>,
        auth_router: Arc<AuthRouter>,
        inspect_ca: Arc<InspectCertAuthority>,
        monitoring: Arc<MonitoringCollector>,
        oauth_engine: Arc<tokio::sync::RwLock<Arc<crate::proxy::oauth_script_engine::OAuthScriptEngine>>>,
        oauth_vault: Arc<crate::proxy::oauth_token_vault::OAuthTokenVault>,
        llm_matcher:  Arc<LlmProviderMatcher>,
        store_auth:   Arc<crate::store::auth::StoreAuth>,
        cdp_handle:   Arc<std::sync::Mutex<CdpBrowserHandle>>,
        vsock_connected: Arc<std::sync::atomic::AtomicU32>,
    ) {
        emit_typed(&emitter, "vm-ssh-status", &serde_json::json!({
            "id": &vm_id, "status": "connecting"
        }));

        // Accept physical stream with timeout (blocks until VMM subprocess connects)
        let mut physical_stream = match tokio::time::timeout(
            tokio::time::Duration::from_secs(VMM_STARTUP_TIMEOUT_SECS),
            listener.accept(),
        ).await {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                error!("Failed to accept VSOCK relay connection: {}", e);
                emit_typed(&emitter, "vm-ssh-status", &serde_json::json!({
                    "id": &vm_id, "status": format!("error:{}", e)
                }));
                return;
            }
            Err(_) => {
                error!("VSOCK relay accept timed out after {}s", VMM_STARTUP_TIMEOUT_SECS);
                emit_typed(&emitter, "vm-ssh-status", &serde_json::json!({
                    "id": &vm_id, "status": "error:VSOCK relay accept timed out"
                }));
                return;
            }
        };

        debug!("VSOCK relay connected, sending agent auth token for VM {}", vm_id);

        // Send 32-byte agent auth token before creating multiplexer
        if let Err(e) = physical_stream.write(&agent_auth_token).await {
            error!("Failed to send agent auth token: {}", e);
            emit_typed(&emitter, "vm-ssh-status", &serde_json::json!({
                "id": &vm_id, "status": format!("error:auth token send failed: {}", e)
            }));
            return;
        }
        debug!("Agent auth token sent successfully");

        // Create reverse-proxy channel for incoming VM→host streams
        let (reverse_tx, mut reverse_rx) =
            tokio::sync::mpsc::channel::<crate::vsock::stream::VirtualStream>(256);
        let multiplexer = Arc::new(
            crate::vsock::stream::StreamMultiplexer::new(physical_stream, Some(reverse_tx))
        );
        *instance.multiplexer.write().await = Some(multiplexer.clone());

        // Register multiplexer with gateway for port forwarding
        gateway.set_multiplexer(&vm_id, multiplexer.clone()).await;

        // Re-apply persisted port mappings for this VM
        if let Ok(saved_mappings) = config_store.list_port_mappings(&vm_id) {
            for pm in saved_mappings {
                if let Err(e) = gateway.add_mapping(&vm_id, pm.host_port, pm.vm_port).await {
                    warn!("Failed to re-apply port mapping {}→{}: {}", pm.host_port, pm.vm_port, e);
                } else {
                    debug!("Re-applied port mapping: localhost:{} → VM:{}", pm.host_port, pm.vm_port);
                }
            }
        }

        // Spawn reverse-proxy handler for outbound VM traffic
        {
            let gate_clone = domain_gate.clone();
            let token_mismatch_gate_clone = token_mismatch_gate.clone();
            let auth_router_clone = auth_router.clone();
            let inspect_ca_clone = inspect_ca.clone();
            let config_store_clone = config_store.clone();
            let keystore_clone = keystore.clone();
            let emitter_clone = emitter.clone();
            let vm_id_clone = vm_id.clone();
            let gateway_clone = gateway.clone();
            let oauth_engine_clone = oauth_engine.clone();
            let oauth_vault_clone = oauth_vault.clone();

            let llm_matcher_clone   = llm_matcher.clone();
            let token_logger        = Arc::new(TokenUsageLogger::new(config_store.clone(), emitter.clone()));
            let token_limit_checker = Arc::new(TokenLimitChecker::new(config_store.clone(), emitter.clone()));
            let monitoring_clone    = monitoring.clone();
            let store_auth_clone    = store_auth.clone();

            let config_store_for_cdp = config_store.clone();
            let cdp_handle_for_rx = cdp_handle.clone();
            let vsock_connected_for_rx = vsock_connected.clone();
            vsock_connected.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            debug!("VM {} VSOCK connected (active: {})", vm_id, vsock_connected.load(std::sync::atomic::Ordering::Relaxed));
            tokio::spawn(async move {
                while let Some(stream) = reverse_rx.recv().await {
                    if stream.initial_frame_type == Some(crate::vsock::protocol::FrameType::DnsRequest) {
                        tokio::spawn(async move {
                            if let Err(e) = crate::proxy::dns_resolver::handle_dns(stream).await {
                                tracing::error!("DNS resolver error: {}", e);
                            }
                        });
                    } else if stream.initial_frame_type == Some(crate::vsock::protocol::FrameType::HostConnect) {
                        let cs = config_store_for_cdp.clone();
                        let ch = cdp_handle_for_rx.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_host_connect_stream(stream, cs, ch).await {
                                tracing::error!("HostConnect error: {}", e);
                            }
                        });
                    } else {
                        let proxy = ReverseProxy::new(
                            gate_clone.clone(),
                            token_mismatch_gate_clone.clone(),
                            auth_router_clone.clone(),
                            inspect_ca_clone.clone(),
                            config_store_clone.clone(),
                            keystore_clone.clone(),
                            emitter_clone.clone(),
                            vm_id_clone.clone(),
                            gateway_clone.clone(),
                            oauth_engine_clone.clone(),
                            oauth_vault_clone.clone(),
                            llm_matcher_clone.clone(),
                            token_logger.clone(),
                            token_limit_checker.clone(),
                            monitoring_clone.clone(),
                            store_auth_clone.clone(),
                        );
                        tokio::spawn(async move {
                            if let Err(e) = proxy.handle_request(stream).await {
                                let msg = e.to_string();
                                if msg.contains("tls handshake eof") || msg.contains("Stream closed by peer") || msg.contains("unexpected eof") {
                                    tracing::warn!("ReverseProxy connection closed early: {}", e);
                                } else {
                                    tracing::error!("ReverseProxy error: {}", e);
                                }
                            }
                        });
                    }
                }

                // VM VSOCK disconnected — decrement counter.
                // If this was the last connected VM, kill headless Chrome (if auto-launched).
                let remaining = vsock_connected_for_rx.fetch_sub(1, std::sync::atomic::Ordering::Relaxed) - 1;
                debug!("VM VSOCK disconnected (remaining: {})", remaining);
                if remaining == 0 {
                    let should_kill = {
                        let h = cdp_handle_for_rx.lock().unwrap();
                        h.headless && h.child.is_some()
                    };
                    if should_kill {
                        debug!("Last VM disconnected: killing headless Chrome");
                        {
                            let mut h = cdp_handle_for_rx.lock().unwrap();
                            h.cancel_kill_timer();
                            if let Some(mut child) = h.child.take() {
                                let _ = child.kill();
                            }
                        }
                        kill_nilbox_cdp_chrome_by_profile(&cdp_profile_dir(true));
                    }
                }
            });
        }

        debug!("VSOCK multiplexer established for VM {}", vm_id);

        // Inject SSH public key via CONTROL_PORT and establish SSH connection
        const CONTROL_PORT: u32 = 9402;
        const SSH_PORT: u32 = 22;
        const MAX_RETRIES: u32 = 10;
        const RETRY_DELAY_MS: u64 = 2000;

        // Validate SSH key format before injection attempts
        const VALID_SSH_KEY_PREFIXES: &[&str] = &[
            "ssh-rsa ", "ssh-ed25519 ", "ecdsa-sha2-", "ssh-dss ",
            "sk-ssh-ed25519@", "sk-ecdsa-sha2-",
        ];
        if !VALID_SSH_KEY_PREFIXES.iter().any(|p| ssh_public_key.starts_with(p)) {
            warn!("SSH public key has unexpected format (first 20 chars: {:?}), skipping injection",
                ssh_public_key.get(..20).unwrap_or(&ssh_public_key));
        }

        // Connect SSH with retries (key injection included in retry loop)
        const SSH_CONNECT_TIMEOUT_SECS: u64 = 15;
        const CONTROL_TIMEOUT_SECS: u64 = 5;
        let mut key_injected = false;

        debug!("Establishing SSH connection to VM {}", vm_id);
        for attempt in 1..=MAX_RETRIES {
            // Re-attempt key injection if not yet successful
            if !key_injected {
                match multiplexer.create_stream(CONTROL_PORT).await {
                    Ok(mut control_stream) => {
                        let inject_cmd = serde_json::json!({
                            "action": "inject_ssh_key",
                            "pubkey": ssh_public_key,
                        });
                        if let Err(e) = control_stream.write(inject_cmd.to_string().as_bytes()).await {
                            warn!("SSH key injection write failed (attempt {}): {}", attempt, e);
                        } else {
                            match tokio::time::timeout(
                                tokio::time::Duration::from_secs(CONTROL_TIMEOUT_SECS),
                                control_stream.read(),
                            ).await {
                                Ok(Ok(data)) => {
                                    debug!("SSH key injected via control port: {}", String::from_utf8_lossy(&data));
                                    key_injected = true;
                                }
                                Ok(Err(e)) => warn!("Control port read error (attempt {}): {}", attempt, e),
                                Err(_) => warn!("Control port timed out (attempt {}), VM may still be booting", attempt),
                            }
                        }
                    }
                    Err(e) => warn!("Control stream open failed (attempt {}): {}", attempt, e),
                }
            }
            let vs = match multiplexer.create_stream(SSH_PORT).await {
                Ok(vs) => vs,
                Err(e) => {
                    warn!("SSH attempt {}/{}: failed to create stream: {}", attempt, MAX_RETRIES, e);
                    tokio::time::sleep(tokio::time::Duration::from_millis(RETRY_DELAY_MS)).await;
                    continue;
                }
            };

            let duplex = wrap_virtual_stream(vs);

            let ssh_result = tokio::time::timeout(
                tokio::time::Duration::from_secs(SSH_CONNECT_TIMEOUT_SECS),
                crate::ssh::client::SshClient::connect(duplex, ssh_private_key.clone()),
            ).await;

            let ssh_connect = match ssh_result {
                Ok(inner) => inner,
                Err(_) => {
                    warn!("SSH attempt {}/{}: connection timed out after {}s", attempt, MAX_RETRIES, SSH_CONNECT_TIMEOUT_SECS);
                    tokio::time::sleep(tokio::time::Duration::from_millis(RETRY_DELAY_MS)).await;
                    continue;
                }
            };

            match ssh_connect {
                Ok(client) => {
                    debug!("SSH connection established on attempt {}", attempt);

                    // Inject Inspect CA certificate via Control Port (structured commands).
                    let ca_pem = inspect_ca.ca_cert_pem();
                    let ctrl = crate::control_client::ControlClient::new(Arc::clone(&multiplexer));

                    // 1. Write cert file
                    match ctrl.write_file(
                        "/usr/local/share/ca-certificates/nilbox-inspect.crt",
                        ca_pem.as_bytes(),
                        "0644",
                        "root",
                    ).await {
                        Ok(_) => debug!("Inspect CA cert written to VM"),
                        Err(e) => warn!("Failed to write Inspect CA cert: {}", e),
                    }

                    // 2. Run update-ca-certificates + append to bundles + certifi + npm
                    match ctrl.update_ca_certificates().await {
                        Ok(_) => debug!("Inspect CA certificate update chain completed"),
                        Err(e) => warn!("Failed to update CA certificates: {}", e),
                    }

                    // 3. Write /etc/profile.d/nilbox-proxy.sh (env vars for interactive shells)
                    let proxy_env_script = concat!(
                        "export REQUESTS_CA_BUNDLE=/etc/ssl/certs/ca-certificates.crt\n",
                        "export AWS_CA_BUNDLE=/etc/ssl/certs/ca-certificates.crt\n",
                        "export SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt\n",
                        "export NODE_EXTRA_CA_CERTS=/usr/local/share/ca-certificates/nilbox-inspect.crt\n",
                    );
                    match ctrl.write_file(
                        "/etc/profile.d/nilbox-proxy.sh",
                        proxy_env_script.as_bytes(),
                        "0644",
                        "root",
                    ).await {
                        Ok(_) => debug!("Proxy env profile written"),
                        Err(e) => warn!("Failed to write proxy env profile: {}", e),
                    }

                    // 4. Write /etc/environment (applies to ALL processes)
                    let etc_env_content = concat!(
                        "NODE_EXTRA_CA_CERTS=/usr/local/share/ca-certificates/nilbox-inspect.crt\n",
                        "REQUESTS_CA_BUNDLE=/etc/ssl/certs/ca-certificates.crt\n",
                        "AWS_CA_BUNDLE=/etc/ssl/certs/ca-certificates.crt\n",
                        "SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt\n",
                    );
                    match ctrl.write_file(
                        "/etc/environment",
                        etc_env_content.as_bytes(),
                        "0644",
                        "root",
                    ).await {
                        Ok(_) => debug!("/etc/environment updated with CA env vars"),
                        Err(e) => warn!("Failed to update /etc/environment: {}", e),
                    }

                    // Inject enabled env vars into /etc/profile.d/nilbox-envs.sh
                    let env_overrides = config_store.list_env_entry_overrides(&vm_id)
                        .unwrap_or_else(|e| { warn!("Failed to load env entries: {}", e); vec![] });
                    let mut enabled_names: Vec<String> = env_overrides
                        .into_iter()
                        .filter(|e| e.enabled)
                        .map(|e| e.name)
                        .collect();

                    // Add OAuth providers' token_path to enabled_names for all providers with credentials
                    let oauth_engine = oauth_engine.read().await.clone();
                    for provider in oauth_engine.providers() {
                        let oauth_key = format!("oauth:{}", provider.info.name);
                        let has_creds = keystore.has(&provider.info.token_path).await.unwrap_or(false)
                            || keystore.has(&oauth_key).await.unwrap_or(false);
                        if has_creds {
                            enabled_names.push(provider.info.token_path.clone());
                        }
                    }

                    // Validate env var names contain only safe characters
                    enabled_names.retain(|name| {
                        let valid = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                        if !valid {
                            warn!("Skipping env var with invalid name: {:?}", name);
                        }
                        valid
                    });

                    if !enabled_names.is_empty() {
                        // Build token_path → filename map from oauth engine
                        let file_var_map: std::collections::HashMap<String, String> =
                            oauth_engine.providers().map(|p| {
                                (p.info.token_path.clone(), format!("oauth_{}.json", p.info.name))
                            }).collect();

                        // Inject env vars into VM profile via Control Port.
                        let mut env_script = String::from("#!/bin/sh\n");
                        for name in &enabled_names {
                            if name.ends_with("_FILE") {
                                let filename = file_var_map.get(name)
                                    .cloned()
                                    .unwrap_or_else(|| {
                                        let base = name.trim_end_matches("_FILE").to_lowercase();
                                        format!("{}.json", base)
                                    });
                                env_script.push_str(&format!("export {}=/etc/nilbox/{}\n", name, filename));
                            } else {
                                env_script.push_str(&format!("export {}={}\n", name, name));
                            }
                        }
                        match ctrl.write_file(
                            "/etc/profile.d/nilbox-envs.sh",
                            env_script.as_bytes(),
                            "0644",
                            "root",
                        ).await {
                            Ok(_) => debug!("Injected {} env var name(s) into VM", enabled_names.len()),
                            Err(e) => warn!("Failed to inject env vars: {}", e),
                        }
                    }

                    // Inject OAuth dummy secrets via Control Port
                    {
                        use crate::proxy::auth_delegator::scripted_oauth::ScriptedOAuthDelegator;
                        for provider in oauth_engine.providers() {
                            // PKCE providers: skip dummy secret injection, only register domain gate
                            if provider.info.flow_type == "pkce" {
                                for td in &provider.info.token_endpoint_domains {
                                    domain_gate.add_always(td, None).await;
                                }
                                for ad in &provider.info.auth_domains {
                                    domain_gate.add_always(ad, None).await;
                                }
                                for cd in &provider.info.cross_domains {
                                    domain_gate.add_always(cd, None).await;
                                }
                                debug!("Registered PKCE OAuth provider: {} (cross_domains={:?})", provider.info.name, provider.info.cross_domains);
                                continue;
                            }

                            let has_credentials = if keystore.has(&provider.info.token_path).await.unwrap_or(false) {
                                true
                            } else {
                                let oauth_key = format!("oauth:{}", provider.info.name);
                                if keystore.has(&oauth_key).await.unwrap_or(false) {
                                    true
                                } else {
                                    let prefix = format!("OAUTH_{}_", provider.info.name.to_uppercase());
                                    keystore.list().await.unwrap_or_else(|e| {
                                        warn!("Failed to list keystore entries: {}", e);
                                        vec![]
                                    }).iter()
                                        .any(|k| k.starts_with(&prefix))
                                }
                            };
                            if !has_credentials {
                                continue;
                            }

                            // ② Inject dummy secret via Control Port
                            match oauth_engine.call_make_dummy_secret(provider) {
                                Ok(dummy_json) => {
                                    if !validate::is_valid_safe_name(&provider.info.name) {
                                        warn!("Skipping provider with invalid name: {}", provider.info.name);
                                        continue;
                                    }
                                    let path = format!("/etc/nilbox/oauth_{}.json", provider.info.name);
                                    match ctrl.write_file(&path, dummy_json.as_bytes(), "0644", "root").await {
                                        Ok(_) => debug!("Injected OAuth dummy secret for provider: {}", provider.info.name),
                                        Err(e) => warn!("Failed to inject OAuth files for {}: {}", provider.info.name, e),
                                    }
                                }
                                Err(e) => {
                                    warn!("make_dummy_secret() failed for {}: {} (route registration continues)", provider.info.name, e);
                                }
                            }

                            // ③ Register ScriptedOAuthDelegator + domain_gate (always, even if dummy injection failed)
                            let delegator = Arc::new(ScriptedOAuthDelegator::new(
                                oauth_engine.clone(),
                                keystore.clone(),
                            ));

                            if let Ok(instructions) = oauth_engine.call_build_token_request_instructions(provider, &std::collections::HashMap::new()) {
                                if let Ok(target_url) = url::Url::parse(&instructions.target_url) {
                                    if let Some(token_domain) = target_url.host_str() {
                                        auth_router.add_route(
                                            token_domain.to_string(),
                                            delegator.clone(),
                                            provider.info.token_path.to_string(),
                                        ).await;
                                        domain_gate.add_always(token_domain, None).await;
                                    }
                                }
                            }

                        }
                    }

                    *instance.ssh_client.write().await = Some(client);

                    // Record successful boot time
                    if let Ok(ts) = config_store.update_vm_last_boot(&vm_id) {
                        *instance.last_boot_at.write().await = Some(ts);
                    }

                    // Start background metrics collection (15s interval)
                    Self::start_metrics_collection(
                        instance.clone(),
                        monitoring.clone(),
                        vm_id.clone(),
                    ).await;

                    // Start 500ms metrics stream emitter for StatusBar & Home
                    Self::spawn_metrics_stream_emitter(
                        monitoring.clone(),
                        emitter.clone(),
                        instance.clone(),
                    );

                    // Start FUSE file proxies for all file mappings
                    let all_mappings = config_store.list_file_mappings(&vm_id).unwrap_or_default();
                    if !all_mappings.is_empty() {
                        let ctrl = crate::control_client::ControlClient::new(Arc::clone(&multiplexer));
                        for m in &all_mappings {
                            if let Err(e) = ctrl.ensure_dir(&m.vm_mount).await {
                                warn!("Failed to pre-create mount point {} in VM: {}", m.vm_mount, e);
                            }
                            let expanded = Self::expand_tilde(&m.host_path);
                            Self::setup_file_proxy(
                                &instance, &multiplexer,
                                std::path::PathBuf::from(expanded),
                                m.read_only, m.vm_mount.clone(), m.id,
                            ).await;
                        }
                    } else {
                        debug!("No file mappings configured; FUSE file proxy not started");
                    }

                    emit_typed(&emitter, "vm-ssh-status", &serde_json::json!({
                        "id": &vm_id, "status": "ready"
                    }));
                    return;
                }
                Err(e) => {
                    warn!("SSH attempt {}/{}: {}", attempt, MAX_RETRIES, e);
                    if attempt < MAX_RETRIES {
                        tokio::time::sleep(tokio::time::Duration::from_millis(RETRY_DELAY_MS)).await;
                    }
                }
            }
        }

        error!("Failed to establish SSH connection after {} attempts", MAX_RETRIES);
        emit_typed(&emitter, "vm-ssh-status", &serde_json::json!({
            "id": &vm_id, "status": "error:SSH connection failed"
        }));
    }

    /// Start FUSE file proxy on VSOCK stream for a VM (internal).
    async fn setup_file_proxy(
        instance: &Arc<VmInstance>,
        multiplexer: &Arc<crate::vsock::stream::StreamMultiplexer>,
        shared_path: std::path::PathBuf,
        read_only: bool,
        vm_mount: String,
        mapping_id: i64,
    ) {
        let port = fuse_port_for_mapping(mapping_id);
        match multiplexer.create_stream(port).await {
            Ok(fuse_stream) => {
                let file_proxy = Arc::new(FileProxy::new(shared_path.clone(), read_only, vm_mount));
                instance.file_proxies.write().await.insert(mapping_id, file_proxy.clone());

                tokio::spawn(async move {
                    file_proxy.listen(fuse_stream).await;
                });

                debug!("FUSE file proxy started for mapping {} on port {} path {:?}", mapping_id, port, shared_path);
            }
            Err(e) => {
                warn!("Failed to create FUSE stream for mapping {}: {}", mapping_id, e);
            }
        }
    }

    /// Spawn a background task that collects VM metrics via SSH exec every 30 seconds.
    /// Stops automatically when the SSH connection is gone, after 3 consecutive failures,
    /// or when the VM's `metrics_cancel` token is cancelled (e.g. on stop/delete).
    async fn start_metrics_collection(
        instance: Arc<VmInstance>,
        monitoring: Arc<MonitoringCollector>,
        vm_id: VmId,
    ) {
        // Replace with a fresh token so this VM can be restarted
        let cancel = tokio_util::sync::CancellationToken::new();
        *instance.metrics_cancel.write().await = cancel.clone();
        tokio::spawn(async move {
            let mut prev_cpu: Option<CpuSnapshot> = None;
            let mut consecutive_failures: u32 = 0;
            debug!("Metrics collection started for VM {}", vm_id);

            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        debug!("Metrics collection cancelled for VM {}", vm_id);
                        return;
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_secs(METRICS_INTERVAL_SECS)) => {}
                }

                // Acquire multiplexer and fetch metrics via Control Port
                let exec_result = {
                    let mux = instance.multiplexer.read().await;
                    match mux.as_ref() {
                        Some(multiplexer) => {
                            let ctrl = crate::control_client::ControlClient::new(Arc::clone(multiplexer));
                            ctrl.get_system_metrics().await
                        }
                        None => {
                            debug!("Multiplexer gone, stopping metrics collection for VM {}", vm_id);
                            return;
                        }
                    }
                };

                let output = match exec_result {
                    Ok(data) => {
                        consecutive_failures = 0;
                        data
                    }
                    Err(e) => {
                        warn!("Metrics exec failed for VM {}: {}", vm_id, e);
                        consecutive_failures += 1;
                        if consecutive_failures >= 3 {
                            debug!("Too many metrics failures, stopping collection for VM {}", vm_id);
                            return;
                        }
                        continue;
                    }
                };

                let text = String::from_utf8_lossy(&output);

                let cpu_pct = if let Some(snap) = parse_proc_stat(&text) {
                    let pct = prev_cpu.as_ref().map_or(0.0, |prev| {
                        let delta_total = snap.total.saturating_sub(prev.total);
                        let delta_idle = snap.idle.saturating_sub(prev.idle);
                        if delta_total > 0 {
                            (1.0 - delta_idle as f64 / delta_total as f64) * 100.0
                        } else {
                            0.0
                        }
                    });
                    prev_cpu = Some(snap);
                    pct
                } else {
                    0.0
                };

                let (mem_used, mem_total) = parse_proc_meminfo(&text).unwrap_or((0, 0));
                // Network bytes are tracked by the proxy layer (all VM traffic goes through
                // vsock, not the VirtIO NIC, so /proc/net/dev would always read 0).
                let (rx_bytes, tx_bytes) = monitoring.get_proxy_bytes();

                monitoring.update(VmMetrics {
                    cpu_percent: cpu_pct,
                    memory_used_mb: mem_used,
                    memory_total_mb: mem_total,
                    network_rx_bytes: rx_bytes,
                    network_tx_bytes: tx_bytes,
                    timestamp: std::time::SystemTime::now(),
                });

            }
        });
    }

    /// Spawn a 500ms ticker that emits `vm-metrics-stream` events for StatusBar & Home.
    /// Reads the VM's `metrics_cancel` token once; stops when cancelled (e.g. on VM stop).
    fn spawn_metrics_stream_emitter(
        monitoring: Arc<MonitoringCollector>,
        emitter: Arc<dyn EventEmitter>,
        instance: Arc<VmInstance>,
    ) {
        tokio::spawn(async move {
            let cancel = instance.metrics_cancel.read().await.clone();
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => { return; }
                    _ = interval.tick() => {}
                }
                let snapshot = monitoring.take_interval_snapshot();
                emit_typed(&emitter, "vm-metrics-stream", &snapshot);
            }
        });
    }

    /// Stop a VM.
    pub async fn stop_vm(&self, id: &VmId) -> Result<()> {
        let instance = self.get_vm(id).await?;
        instance.platform.write().await.stop().await?;

        // Cancel background metrics collection task
        instance.metrics_cancel.read().await.cancel();

        // Clean up gateway multiplexer and listeners for this VM
        self.state.gateway.remove_multiplexer(id).await;
        self.state.gateway.remove_mappings_for_vm(id).await;

        // OAuth sessions are preserved across VM restarts.
        // Users can manually delete them via Mappings → OAuth → Active Sessions UI.

        // Clear stale multiplexer and SSH client so reconnect attempts don't
        // hit a dead connection after stop.
        *instance.multiplexer.write().await = None;
        *instance.ssh_client.write().await = None;

        emit_typed(&self.emitter, "vm-status-changed", &serde_json::json!({
            "id": id, "status": "Stopping"
        }));
        debug!("VM stopping: {}", id);
        Ok(())
    }

    /// Get VM status.
    pub async fn vm_status(&self, id: &VmId) -> Result<VmStatus> {
        let instance = self.get_vm(id).await?;
        Ok(instance.status().await)
    }

    // ── Shell ─────────────────────────────────────────────────

    /// Register a pending app install for a VM. The next `open_shell` call will
    /// run `nilbox-install` interactively instead of opening a plain shell.
    pub async fn store_register_install(&self, vm_id: &VmId, manifest_url: &str) -> Result<()> {
        let mut map = self.pending_installs.lock().await;
        map.entry(vm_id.clone())
            .or_insert_with(VecDeque::new)
            .push_back(PendingInstall { manifest_url: manifest_url.to_string() });
        debug!("Pending install registered for vm={}: {}", vm_id, manifest_url);
        Ok(())
    }

    /// Pop the next pending install for a VM, if any.
    #[allow(dead_code)]
    async fn pop_pending_install(&self, vm_id: &VmId) -> Option<PendingInstall> {
        let mut map = self.pending_installs.lock().await;
        map.get_mut(vm_id)?.pop_front()
    }

    /// Open a shell session on a VM. Returns session ID.
    /// If `install_url` is provided, runs `nilbox-install` via PTY exec instead of a plain shell.
    /// Spawns a background read loop that emits `shell-output-{session_id}` events.
    pub async fn open_shell(&self, vm_id: &VmId, cols: u32, rows: u32, install_url: Option<&str>) -> Result<u64> {
        let instance = self.get_vm(vm_id).await?;

        let mux = {
            let lock = instance.multiplexer.read().await;
            lock.clone().ok_or_else(|| anyhow!("VM not ready: SSH connection not established for {}", vm_id))?
        };

        let vs = mux.create_stream(22).await?;
        let duplex = wrap_virtual_stream(vs);

        let mut client = ssh::client::SshClient::connect(
            duplex,
            self.get_ssh_keys().await?.0,
        ).await?;

        // If this is an install session, fetch the manifest now to extract app_id/name/inbound_ports/functions
        // so we can register the app in the DB when install completes.
        // (app_id, app_name, inbound_ports, admin_urls, functions)
        let install_app_info: Option<(String, String, Vec<u16>, Vec<(String, String)>, Vec<(String, String)>)> = if let Some(url) = install_url {
            // fetch_store_url only attaches the bearer for STORE_BASE_URL hosts
            // (avoids leaking the token to arbitrary hosts) and refreshes once
            // on 401 if the server-side has revoked the cached token.
            match self.fetch_store_url(url).await {
                Ok(resp) if resp.status().is_success() => {
                    match resp.json::<serde_json::Value>().await {
                        Ok(raw) => {
                            use crate::store::envelope::parse_envelope;
                            use crate::store::verify::verify_envelope;

                            match parse_envelope(&raw).and_then(|env| verify_envelope(&env).map(|v| v.clone())) {
                                Ok(manifest) => {
                                    // Check min_disk requirement before proceeding with install
                                    if let Some(min_disk_mb) = manifest["min_disk"].as_u64() {
                                        if min_disk_mb > 0 {
                                            let min_disk_gb = (min_disk_mb + 1023) / 1024;
                                            match self.get_vm_fs_info(vm_id).await {
                                                Ok(fs_info) => {
                                                    if (fs_info.avail_mb as u64) < min_disk_mb {
                                                        let need_mb = min_disk_mb - fs_info.avail_mb as u64;
                                                        let need_gb = (need_mb + 1023) / 1024;
                                                        return Err(anyhow!(
                                                            "Insufficient disk space: this app requires at least {} GB of free disk space, \
                                                             but only {} MB is available. \
                                                             Please add at least {} GB via VM Manager (Resize Disk) before installing.",
                                                            min_disk_gb,
                                                            fs_info.avail_mb,
                                                            need_gb
                                                        ));
                                                    }
                                                }
                                                Err(e) => {
                                                    warn!("Could not check VM disk space ({}), proceeding with install", e);
                                                }
                                            }
                                        }
                                    }

                                    let app_id = manifest["id"].as_str().unwrap_or("").to_string();
                                    let app_name = manifest["name"].as_str()
                                        .filter(|s| !s.is_empty())
                                        .unwrap_or(&app_id)
                                        .to_string();
                                    let inbound_ports: Vec<u16> = manifest["permissions"]["inbound_ports"]
                                        .as_array()
                                        .map(|arr| arr.iter().filter_map(|v| v.as_u64().map(|p| p as u16)).collect())
                                        .unwrap_or_default();
                                    let admin_urls: Vec<(String, String)> = manifest["admin"]
                                        .as_array()
                                        .map(|arr| {
                                            arr.iter()
                                                .filter_map(|entry| {
                                                    let url = entry["url"].as_str()?.to_string();
                                                    if !crate::validate::is_valid_http_url(&url) {
                                                        warn!("Skipping admin URL with invalid scheme: {}", url);
                                                        return None;
                                                    }
                                                    let label = entry["label"].as_str().unwrap_or("").to_string();
                                                    Some((url, label))
                                                })
                                                .collect()
                                        })
                                        .unwrap_or_default();
                                    let functions: Vec<(String, String)> = manifest["functions"]
                                        .as_array()
                                        .map(|arr| {
                                            arr.iter()
                                                .filter_map(|f| {
                                                    let label = f["label"].as_str()?.to_string();
                                                    let bash = f["bash"].as_str()?.to_string();
                                                    Some((label, bash))
                                                })
                                                .collect()
                                        })
                                        .unwrap_or_default();
                                    if app_id.is_empty() { None } else { Some((app_id, app_name, inbound_ports, admin_urls, functions)) }
                                }
                                Err(e) => {
                                    return Err(anyhow!(
                                        "Manifest signature verification failed: {}. Installation blocked.",
                                        e
                                    ));
                                }
                            }
                        }
                        Err(e) => {
                            return Err(anyhow!("Failed to parse manifest response: {}", e));
                        }
                    }
                }
                Ok(resp) => {
                    return Err(anyhow!("Failed to fetch manifest: HTTP {}", resp.status()));
                }
                Err(e) => {
                    return Err(anyhow!("Failed to fetch manifest: {}", e));
                }
            }
        } else {
            None
        };

        let is_install = install_url.is_some();
        let user_verified_for_install = if is_install { self.extract_verified_from_token().await } else { None };
        let state_for_install = if is_install { Some(self.state.clone()) } else { None };
        let vm_id_str = vm_id.to_string();

        // Register admin URLs immediately when install starts (regardless of install outcome)
        if let Some((_, _, _, ref admin_urls, _)) = install_app_info {
            if !admin_urls.is_empty() {
                let vms = self.state.vms.read().await;
                if let Some(vm) = vms.get(vm_id.as_str()) {
                    let mut urls = vm.admin_urls.write().await;
                    for (url, label) in admin_urls {
                        if urls.iter().any(|r| r.url == *url) {
                            debug!("Admin URL already registered, skipping: {}", url);
                            continue;
                        }
                        match self.state.config_store.insert_vm_admin_url(vm_id.as_str(), url, label) {
                            Ok(new_id) => {
                                urls.push(AdminUrlRecord {
                                    id: new_id,
                                    url: url.clone(),
                                    label: label.clone(),
                                });
                                debug!("Registered admin URL: {} (id={})", url, new_id);
                            }
                            Err(e) => warn!("Failed to register admin URL: {}", e),
                        }
                    }
                    // Notify frontend so it can refresh the VM list and show the new admin menu
                    emit_typed(&self.emitter, "admin-urls-changed", &serde_json::json!({ "vm_id": vm_id }));
                }
            }
        }

        // 2c. Register function keys from manifest immediately (before VM communication may fail)
        if let Some((ref app_id, ref app_name, _, _, ref functions)) = install_app_info {
            if !functions.is_empty() {
                let _ = self.state.config_store.delete_function_keys_by_app(vm_id.as_str(), app_id);
                for (label, bash) in functions {
                    if let Err(e) = self.state.config_store.insert_function_key(
                        vm_id.as_str(), label, bash, Some(app_id), Some(app_name),
                    ) {
                        warn!("Failed to insert function key '{}': {}", label, e);
                    }
                }
                emit_typed(&self.emitter, "function-keys-changed", &serde_json::json!({ "vm_id": vm_id }));
                debug!("Registered {} function keys for app '{}' (pre-install)", functions.len(), app_id);
            }
        }

        let mut channel = if let Some(url) = install_url {
            let url_b64 = URL_SAFE_NO_PAD.encode(url);
            // Completion marker: printf emits SOH(0x01)+NILBOX_DONE:<exit_code>+SOH
            // Uses octal \001 (POSIX) because dash's printf does not support \xHH hex escapes.
            let cmd = format!(
                "/bin/bash -lc 'nilbox-install {}; __e=$?; printf \"\\001NILBOX_DONE:%d\\001\\n\" $__e; exec /bin/bash -l'",
                url_b64
            );
            debug!("Opening install channel for vm={}: nilbox-install", vm_id);
            client.open_install_channel(cols, rows, &cmd).await?
        } else {
            client.open_shell_channel(cols, rows).await?
        };
        // Use random session ID to prevent predictability.
        // Mask to 53 bits so the value stays within JavaScript's Number.MAX_SAFE_INTEGER.
        let session_id: u64 = rand::random::<u64>() & ((1u64 << 53) - 1);

        // Create command channel for this session
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<ShellCommand>(64);
        self.shell_sessions.write().await.insert(session_id, (vm_id.to_string(), cmd_tx));

        let emitter = self.emitter.clone();
        let output_event = format!("shell-output-{}", session_id);
        let close_event = format!("shell-closed-{}", session_id);

        // Background task: read SSH output + handle write/resize/close commands
        let sessions_ref = self.shell_sessions.clone();
        tokio::spawn(async move {
            let _client = client; // keep SSH connection alive until task ends
            use russh::ChannelMsg;

            // Marker detection state for install sessions
            let mut install_complete = false;

            loop {
                tokio::select! {
                    msg = channel.wait() => {
                        match msg {
                            Some(ChannelMsg::Data { data }) => {
                                // For install sessions, scan for the completion marker
                                // \x01NILBOX_DONE:<exit_code>\x01 emitted by the wrapper command.
                                if is_install && !install_complete {
                                    let bytes: &[u8] = &data;
                                    const PREFIX: &[u8] = b"\x01NILBOX_DONE:";
                                    if let Some(start) = bytes.windows(PREFIX.len()).position(|w| w == PREFIX) {
                                        let rest = &bytes[start + PREFIX.len()..];
                                        if let Some(end) = rest.iter().position(|&b| b == 0x01) {
                                            install_complete = true;
                                            let exit_code: i32 = std::str::from_utf8(&rest[..end])
                                                .ok()
                                                .and_then(|s| s.parse().ok())
                                                .unwrap_or(-1);
                                            debug!("Shell install session {} completed, exit_code={}", session_id, exit_code);
                                            if exit_code == 0 {
                                                if let (Some((ref app_id, ref app_name, ref inbound_ports, _, _)), Some(ref state)) =
                                                    (&install_app_info, &state_for_install)
                                                {
                                                    if let Err(e) = state.config_store.upsert_installed_app(
                                                        &vm_id_str, app_id, app_name, "latest",
                                                    ) {
                                                        warn!("Failed to persist installed app: {}", e);
                                                    }
                                                    let tag = app_name.to_string();
                                                    if let Err(e) = append_vm_description(state, &vm_id_str, &tag).await {
                                                        warn!("Failed to update VM description: {}", e);
                                                    }
                                                    if user_verified_for_install.as_deref() == Some("admin") {
                                                        for &port in inbound_ports {
                                                            if let Err(e) = state.config_store.insert_port_mapping(&vm_id_str, port, port, app_name) {
                                                                warn!("Failed to insert port mapping {}: {}", port, e);
                                                            } else {
                                                                debug!("Port mapping added: {}", port);
                                                            }
                                                        }
                                                    } else if !inbound_ports.is_empty() {
                                                        debug!("Skipping inbound ports (user verified={:?}, not admin)", user_verified_for_install);
                                                    }
                                                    // Function keys already registered before install (pre-install)
                                                }
                                            }
                                            debug!("Emitting app-install-done: success={}, exit_code={}", exit_code == 0, exit_code);
                                            emit_typed(&emitter, "app-install-done", &serde_json::json!({
                                                "uuid": "", "success": exit_code == 0, "exit_code": exit_code
                                            }));
                                            // Strip the marker from the output before emitting to terminal
                                            let marker_end = (start + PREFIX.len() + end + 1).min(bytes.len());
                                            let mut clean = bytes.to_vec();
                                            clean.drain(start..marker_end);
                                            emitter.emit_bytes(&output_event, &clean);
                                            continue;
                                        }
                                    }
                                }
                                emitter.emit_bytes(&output_event, &data);
                            }
                            Some(ChannelMsg::ExtendedData { data, ext: _ }) => {
                                emitter.emit_bytes(&output_event, &data);
                            }
                            Some(ChannelMsg::ExitStatus { exit_status }) => {
                                debug!("Shell session {} exited with status {}", session_id, exit_status);
                            }
                            Some(ChannelMsg::Eof) | None => {
                                debug!("Shell session {} channel closed", session_id);
                                break;
                            }
                            _ => {}
                        }
                    }
                    cmd = cmd_rx.recv() => {
                        match cmd {
                            Some(ShellCommand::Write(data)) => {
                                if let Err(e) = channel.data(&data[..]).await {
                                    error!("SSH channel write error: {}", e);
                                    break;
                                }
                            }
                            Some(ShellCommand::Resize { cols, rows }) => {
                                if let Err(e) = channel.window_change(cols, rows, 0, 0).await {
                                    warn!("SSH window change error: {}", e);
                                }
                            }
                            Some(ShellCommand::Close) | None => {
                                let _ = channel.close().await;
                                break;
                            }
                        }
                    }
                }
            }
            sessions_ref.write().await.remove(&session_id);
            emitter.emit(&close_event, "");
            debug!("Shell session {} cleaned up", session_id);
        });

        emit_typed(&self.emitter, "shell-opened", &serde_json::json!({
            "vm_id": vm_id, "session_id": session_id
        }));
        debug!("Shell opened: vm={}, session={}", vm_id, session_id);
        Ok(session_id)
    }

    /// Write data to a shell session.
    pub async fn write_shell(&self, session_id: u64, data: Vec<u8>) -> Result<()> {
        let sessions = self.shell_sessions.read().await;
        let (_, tx) = sessions.get(&session_id)
            .ok_or_else(|| anyhow!("Shell session {} not found", session_id))?;
        tx.send(ShellCommand::Write(Arc::new(data))).await
            .map_err(|_| anyhow!("Shell session {} channel closed", session_id))
    }

    /// Resize a shell session PTY.
    pub async fn resize_shell(&self, session_id: u64, cols: u32, rows: u32) -> Result<()> {
        let sessions = self.shell_sessions.read().await;
        let (_, tx) = sessions.get(&session_id)
            .ok_or_else(|| anyhow!("Shell session {} not found", session_id))?;
        tx.send(ShellCommand::Resize { cols, rows }).await
            .map_err(|_| anyhow!("Shell session {} channel closed", session_id))
    }

    /// Close a shell session.
    ///
    /// Sends a Close command to the background task which handles channel close
    /// and session cleanup. We do NOT remove the session here to avoid racing
    /// with the background task's own `sessions_ref.write().remove()`.
    pub async fn close_shell(&self, session_id: u64) -> Result<()> {
        let sessions = self.shell_sessions.read().await;
        if let Some((_, tx)) = sessions.get(&session_id) {
            let _ = tx.send(ShellCommand::Close).await;
        }
        Ok(())
    }

    /// Inject a shell command string into all active sessions for a given VM.
    async fn inject_env_to_sessions(&self, vm_id: &VmId, script: &str) {
        let sessions = self.shell_sessions.read().await;
        let bytes = Arc::new(script.as_bytes().to_vec());
        for (_, (vid, tx)) in sessions.iter() {
            if vid == vm_id.as_str() {
                let _ = tx.send(ShellCommand::Write(Arc::clone(&bytes))).await;
            }
        }
    }

    // ── Shell OAuth URL ───────────────────────────────────────

    /// Open an OAuth authorization URL detected from shell output.
    /// Registers a temporary callback port mapping and opens the URL in the host browser.
    pub async fn open_oauth_url_from_shell(&self, vm_id: &VmId, url: &str) -> Result<()> {
        // Validate URL contains an OAuth authorize path
        let parsed = url::Url::parse(url).map_err(|e| anyhow!("invalid URL: {}", e))?;
        // Only allow http/https schemes
        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            return Err(anyhow!("URL scheme must be http or https"));
        }
        let path = parsed.path();
        // Match /authorize or /auth as a complete path segment (not substrings like /authentication)
        let has_oauth_path = path.split('/').any(|seg| seg == "authorize" || seg == "auth");
        if !has_oauth_path {
            return Err(anyhow!("URL does not contain an OAuth authorize path"));
        }
        let has_response_type_code = parsed.query_pairs().any(|(k, v)| k == "response_type" && v == "code");
        if !has_response_type_code {
            return Err(anyhow!("URL missing response_type=code parameter"));
        }

        // Delay 1s so the VM's xdg-open hook (if any) has time to hit /__nilbox__/open-url first.
        // Then dedupe: if the same URL was already opened in the browser within 2s, drop this one.
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        if crate::proxy::reverse_proxy::was_recently_opened(url, tokio::time::Duration::from_secs(2)) {
            debug!("shell-oauth: same URL opened within 2s by another path, dropping");
            return Ok(());
        }

        // Register temporary callback port mapping (skip if already mapped by inspect)
        if let Some(callback_port) = crate::proxy::reverse_proxy::extract_redirect_port(url) {
            let already_mapped = self.state.gateway.get_mappings_for_vm(vm_id).await
                .iter().any(|(hp, _)| *hp == callback_port);
            if already_mapped {
                debug!("shell-oauth: callback port {} already mapped, skipping re-registration", callback_port);
            } else {
                match self.state.gateway.add_mapping(vm_id, callback_port, callback_port).await {
                    Ok(_) => {
                        debug!("shell-oauth: callback port mapping localhost:{} → VM:{} registered", callback_port, callback_port);
                        let gateway = self.state.gateway.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(tokio::time::Duration::from_secs(300)).await;
                            gateway.remove_mapping(callback_port).await;
                            debug!("shell-oauth: expired callback port mapping {}", callback_port);
                        });
                    }
                    Err(e) => warn!("shell-oauth: failed to register callback port mapping: {}", e),
                }
            }
        }

        // Rewrite dummy NILBOX_OAUTH_* placeholders to real credentials before opening.
        // Uses the same OAuthUrlRewriter that handle_nilbox_control uses on the xdg-open path.
        let domain = parsed.host_str().unwrap_or("").to_string();
        let rewriter = crate::proxy::oauth_url_rewriter::OAuthUrlRewriter::new(
            self.state.oauth_engine.clone(),
        );
        let final_url = if url.contains("NILBOX_OAUTH_") && !domain.is_empty() {
            match rewriter
                .rewrite(
                    url,
                    &domain,
                    &self.state.config_store,
                    self.state.keystore.as_ref(),
                    &[],
                )
                .await
            {
                Ok(rewritten) => {
                    if rewritten != url {
                        debug!("shell-oauth: rewrote OAuth dummy credentials for domain {}", domain);
                    }
                    rewritten
                }
                Err(e) => {
                    warn!("shell-oauth: OAuth rewrite failed for {}: {}, using original URL", domain, e);
                    url.to_string()
                }
            }
        } else {
            url.to_string()
        };

        // Open in host browser and record (so a late-arriving xdg-open hook skips).
        crate::proxy::reverse_proxy::record_browser_open(url);
        crate::proxy::reverse_proxy::open_in_browser(&final_url)?;
        debug!("shell-oauth: opened OAuth URL in browser for VM {}", vm_id);
        Ok(())
    }

    // ── Port Mapping ──────────────────────────────────────────

    /// Add a port mapping for a specific VM. Host port must be globally unique.
    pub async fn add_mapping(&self, vm_id: &str, host_port: u16, vm_port: u16, label: String) -> Result<()> {
        if host_port < 1024 {
            return Err(anyhow!("host_port must be >= 1024 (got {}), privileged ports are not allowed", host_port));
        }
        // Reject vm_port == 0
        if vm_port == 0 {
            return Err(anyhow!("vm_port must be > 0"));
        }
        // Reject reserved VM ports (used internally by nilbox)
        for &(port, name) in RESERVED_VM_PORTS {
            if vm_port == port {
                return Err(anyhow!("vm_port {} is reserved for {} and cannot be used", port, name));
            }
        }
        self.state.gateway.add_mapping(vm_id, host_port, vm_port).await?;
        self.state.config_store.insert_port_mapping(vm_id, host_port, vm_port, &label)?;

        emit_typed(&self.emitter, "mapping-added", &serde_json::json!({
            "vm_id": vm_id, "host_port": host_port, "vm_port": vm_port, "label": label
        }));
        Ok(())
    }

    /// Remove a port mapping by host port (globally unique).
    pub async fn remove_mapping(&self, host_port: u16) -> Result<()> {
        self.state.gateway.remove_mapping(host_port).await;
        // Ignore DB error if mapping doesn't exist in DB (could be in-memory only)
        let _ = self.state.config_store.delete_port_mapping(host_port);

        emit_typed(&self.emitter, "mapping-removed", &serde_json::json!({
            "host_port": host_port
        }));
        Ok(())
    }

    /// List port mappings for a specific VM.
    pub async fn list_mappings(&self, vm_id: &str) -> Vec<PortMappingConfig> {
        self.state.config_store.list_port_mappings(vm_id).unwrap_or_else(|e| {
            warn!("Failed to list port mappings for VM {}: {}", vm_id, e);
            vec![]
        })
    }

    // ── Admin Proxy (ephemeral, session-only) ────────────────────

    /// Open an ephemeral admin proxy: localhost:<random> → VM:<vm_port> via vsock.
    /// Returns the assigned host port. Reuses an existing mapping if one exists for the same vm+port.
    pub async fn open_admin_proxy(&self, vm_id: &str, vm_port: u16) -> Result<u16> {
        // Reject reserved vm_port (used internally by nilbox)
        for &(port, name) in RESERVED_VM_PORTS {
            if vm_port == port {
                return Err(anyhow!("vm_port {} is reserved for {} and cannot be used", port, name));
            }
        }
        // Check VM exists and is running
        let instance = self.get_vm(vm_id).await?;
        let status = instance.status().await;
        if status != VmStatus::Running {
            return Err(anyhow!("VM is not running (status: {:?})", status));
        }

        // Check if an existing gateway mapping already forwards to this vm+port
        let existing = self.state.gateway.get_mappings_for_vm(vm_id).await;
        for (host_port, mapped_vm_port) in &existing {
            if *mapped_vm_port == vm_port {
                debug!("Reusing existing mapping localhost:{} -> VM {}:{}", host_port, vm_id, vm_port);
                return Ok(*host_port);
            }
        }

        // Create ephemeral mapping (not persisted to DB)
        let host_port = self.state.gateway.add_mapping_ephemeral(vm_id, vm_port).await?;
        Ok(host_port)
    }

    /// Close an ephemeral admin proxy by host port (memory only, no DB).
    pub async fn close_admin_proxy(&self, host_port: u16) -> Result<()> {
        self.state.gateway.remove_mapping(host_port).await;
        Ok(())
    }

    // ── File Mapping (Config CRUD) ──────────────────────────────

    /// List file mappings for a specific VM.
    pub async fn list_file_mappings(&self, vm_id: &str) -> Vec<FileMappingRecord> {
        self.state.config_store.list_file_mappings(vm_id).unwrap_or_else(|e| {
            warn!("Failed to list file mappings for VM {}: {}", vm_id, e);
            vec![]
        })
    }

    /// Add a file mapping for a VM. Immediately mounts if the VM is running.
    pub async fn add_file_mapping(
        &self,
        vm_id: &str,
        host_path: &str,
        vm_mount: &str,
        read_only: bool,
        label: &str,
    ) -> Result<()> {
        use crate::file_proxy::protocol::MAX_FILE_MAPPINGS;

        // H7: Input validation
        if !vm_mount.starts_with('/') {
            return Err(anyhow::anyhow!("vm_mount must be an absolute path: {}", vm_mount));
        }

        // Prevent duplicate vm_mount for the same VM
        let existing = self.state.config_store.list_file_mappings(vm_id)?;
        if existing.iter().any(|m| m.vm_mount == vm_mount) {
            return Err(anyhow::anyhow!("vm_mount already exists: {}", vm_mount));
        }
        if existing.len() >= MAX_FILE_MAPPINGS {
            return Err(anyhow::anyhow!("Maximum {} file mappings reached", MAX_FILE_MAPPINGS));
        }

        let expanded_host = Self::expand_tilde(host_path);
        let host = std::path::Path::new(&expanded_host);
        let canonical_host = host.canonicalize().map_err(|e| {
            anyhow::anyhow!("host_path does not exist or is not accessible: {} ({})", host_path, e)
        })?;

        if canonical_host != host {
            tracing::debug!(
                "host_path resolved through symlink: {} -> {}",
                host_path,
                canonical_host.display()
            );
        }

        // Store the canonicalized path to prevent symlink TOCTOU
        let canonical_str = canonical_host.to_string_lossy();
        let mapping_id = self.state.config_store.insert_file_mapping(
            vm_id, &canonical_str, vm_mount, read_only, label,
        )?;

        // Mount immediately if VM is running
        if let Ok(instance) = self.get_vm(vm_id).await {
            let mux = instance.multiplexer.read().await.clone();
            if let Some(mux) = mux {
                let ctrl = crate::control_client::ControlClient::new(Arc::clone(&mux));
                if let Err(e) = ctrl.ensure_dir(vm_mount).await {
                    tracing::warn!("Failed to pre-create mount point {} in VM: {}", vm_mount, e);
                }
                let expanded = Self::expand_tilde(host_path);
                Self::setup_file_proxy(
                    &instance, &mux,
                    std::path::PathBuf::from(expanded),
                    read_only, vm_mount.to_string(), mapping_id,
                ).await;
            }
        }

        emit_typed(&self.emitter, "file-mapping-added", &serde_json::json!({
            "vm_id": vm_id, "host_path": host_path, "vm_mount": vm_mount
        }));
        Ok(())
    }

    /// Remove a file mapping by its database ID.
    pub async fn remove_file_mapping(&self, vm_id: &str, mapping_id: i64) -> Result<()> {
        // 0. 동시 호출 guard — 이미 제거 진행 중이면 거부
        {
            let mut pending = self.pending_mapping_removals.lock().await;
            if !pending.insert(mapping_id) {
                warn!("remove_file_mapping: mapping_id={} already being removed, skipping", mapping_id);
                return Err(anyhow!("file mapping {} is already being removed", mapping_id));
            }
        }

        let (result, spawned_bg) = self.remove_file_mapping_inner(vm_id, mapping_id).await;

        // background task가 spawn되지 않은 경우에만 여기서 guard 해제
        // (spawn된 경우는 task 완료 시 해제)
        if !spawned_bg {
            self.pending_mapping_removals.lock().await.remove(&mapping_id);
        }

        result
    }

    /// Inner implementation of remove_file_mapping.
    /// Returns (result, whether a background unmount task was spawned).
    async fn remove_file_mapping_inner(&self, vm_id: &str, mapping_id: i64) -> (Result<()>, bool) {
        let mut spawned_bg = false;

        // 1. DB에서 삭제
        if let Err(e) = self.state.config_store.delete_file_mapping(mapping_id) {
            return (Err(e.into()), false);
        }

        // 2. 실행 중인 프록시가 있으면 언마운트
        let vm_id_owned = vm_id.to_string();
        if let Ok(instance) = self.get_vm(&vm_id_owned).await {
            let proxy_opt = instance.file_proxies.write().await.remove(&mapping_id);

            if let Some(proxy) = proxy_opt {
                let (pending_count, unmount_rx) = proxy.request_unmount().await;

                if pending_count > 0 {
                    emit_typed(&self.emitter, "file-proxy-unmount-pending", &serde_json::json!({
                        "vm_id": vm_id,
                        "mapping_id": mapping_id,
                        "pending_handles": pending_count as u32
                    }));

                    let emitter = self.emitter.clone();
                    let removals = self.pending_mapping_removals.clone();
                    if let Some(rx) = unmount_rx {
                        spawned_bg = true;
                        let proxy_clone = proxy.clone();
                        tokio::spawn(async move {
                            let result = tokio::time::timeout(
                                std::time::Duration::from_secs(FILE_UNMOUNT_TIMEOUT_SECS),
                                rx,
                            ).await;
                            if result.is_err() {
                                proxy_clone.force_unmount().await;
                            } else {
                                proxy_clone.shutdown();
                            }
                            removals.lock().await.remove(&mapping_id);
                            emit_typed(&emitter, "file-proxy-unmounted", &serde_json::json!({
                                "vm_id": vm_id_owned,
                                "mapping_id": mapping_id
                            }));
                        });
                    }
                } else {
                    proxy.shutdown();
                    emit_typed(&self.emitter, "file-proxy-unmounted", &serde_json::json!({
                        "vm_id": vm_id,
                        "mapping_id": mapping_id
                    }));
                }
            }
        }

        emit_typed(&self.emitter, "file-mapping-removed", &serde_json::json!({
            "vm_id": vm_id, "mapping_id": mapping_id
        }));
        (Ok(()), spawned_bg)
    }

    /// 사용자가 UI에서 "강제 해제" 버튼 클릭 시 호출
    pub async fn force_unmount_file_proxy(&self, vm_id: &VmId, mapping_id: i64) -> Result<()> {
        let instance = self.get_vm(vm_id).await?;
        let proxy = instance.file_proxies.write().await.remove(&mapping_id);
        if let Some(proxy) = proxy {
            proxy.force_unmount().await;
            emit_typed(&self.emitter, "file-proxy-unmounted", &serde_json::json!({
                "vm_id": vm_id,
                "mapping_id": mapping_id
            }));
        }
        Ok(())
    }

    /// Expand `~/` to `$HOME/` in paths.
    fn expand_tilde(path: &str) -> String {
        if path.starts_with("~/") {
            if let Ok(home) = std::env::var("HOME") {
                return format!("{}{}", home, &path[1..]);
            }
        }
        path.to_string()
    }


    // ── Security: API Keys ────────────────────────────────────

    pub async fn set_api_key(&self, account: &str, value: &str) -> Result<()> {
        self.state.keystore.set(account, value).await
    }

    pub async fn delete_api_key(&self, account: &str) -> Result<()> {
        self.state.keystore.delete(account).await
    }

    pub async fn list_api_keys(&self) -> Result<Vec<String>> {
        self.state.keystore.list().await
    }

    pub async fn has_api_key(&self, account: &str) -> Result<bool> {
        self.state.keystore.has(account).await
    }

    // ── Domain Allowlist ──────────────────────────────────────

    /// Resolve user decision for a domain access request.
    /// action: "allow_once" | "allow_always" | "deny"
    pub async fn resolve_domain_access(&self, domain: &str, action: String, env_names: Vec<String>) -> Result<()> {
        let decision = match action.as_str() {
            "allow_once" => DomainDecision::AllowOnce,
            "allow_always" => DomainDecision::AllowAlways,
            _ => DomainDecision::Deny,
        };
        // 1. Gate resolve FIRST → domain_allowlist INSERT (AllowAlways)
        self.state.domain_gate.resolve(&domain, decision, env_names.clone()).await;
        // 2. THEN persist env mappings (FK on domain_allowlist now satisfied)
        if action == "allow_always" && !env_names.is_empty() {
            self.set_domain_env_mappings(&domain, env_names).await?;
        }
        Ok(())
    }

    /// Resolve user decision for a token mismatch warning.
    /// action: "pass_through" | "block"
    pub async fn resolve_token_mismatch(&self, request_id: String, action: String) -> Result<()> {
        let decision = match action.as_str() {
            "pass_through" => TokenMismatchDecision::PassThrough,
            _ => TokenMismatchDecision::Block,
        };
        self.state.token_mismatch_gate.resolve(&request_id, decision).await;
        Ok(())
    }

    pub async fn add_allowlist_domain(&self, domain: &str) -> Result<()> {
        self.state.domain_gate.add_always(domain, None).await;
        Ok(())
    }

    pub async fn add_allowlist_domain_with_mode(
        &self,
        domain: &str,
        mode: crate::config_store::InspectMode,
    ) -> Result<()> {
        self.state.domain_gate.add_always_with_mode(domain, None, mode).await;
        Ok(())
    }

    pub async fn remove_allowlist_domain(&self, domain: &str) -> Result<()> {
        // ON DELETE CASCADE in domain_token_accounts handles DB cleanup.
        // Env values are owned by the environments feature — no keystore deletion here.
        self.state.domain_gate.remove_always(domain).await;
        Ok(())
    }

    /// Map an existing env variable (already in keystore) to a domain.
    /// Only creates the DB mapping — does NOT write to keystore.
    pub async fn map_env_to_domain(&self, domain: &str, env_name: &str) -> Result<()> {
        let store = self.state.config_store.clone();
        let domain = domain.to_string();
        let env_name = env_name.to_string();
        tokio::task::spawn_blocking(move || store.add_domain_token(&domain, &env_name))
            .await
            .context("spawn_blocking join error")?
    }

    /// Unmap an env variable from a domain.
    /// Only removes the DB mapping — does NOT delete from keystore.
    pub async fn unmap_env_from_domain(&self, domain: &str, env_name: &str) -> Result<()> {
        let store = self.state.config_store.clone();
        let domain = domain.to_string();
        let env_name = env_name.to_string();
        tokio::task::spawn_blocking(move || store.remove_domain_token(&domain, &env_name))
            .await
            .context("spawn_blocking join error")?
    }

    /// Replace all env-variable mappings for a domain in one transaction.
    pub async fn set_domain_env_mappings(&self, domain: &str, env_names: Vec<String>) -> Result<()> {
        let store = self.state.config_store.clone();
        let domain = domain.to_string();
        tokio::task::spawn_blocking(move || store.set_domain_tokens(&domain, &env_names))
            .await
            .context("spawn_blocking join error")?
    }

    pub async fn remove_domain_token_account(&self, domain: &str, token_account: &str) -> Result<()> {
        if let Err(e) = self.state.keystore.delete(token_account).await {
            warn!("Failed to delete keyring entry {}: {}", token_account, e);
        }
        self.state.config_store.remove_domain_token(domain, token_account)?;
        Ok(())
    }

    pub async fn add_domain_token_account(&self, domain: &str, token_account: &str, token_value: &str) -> Result<()> {
        self.state.keystore.set(token_account, token_value).await
            .context("Failed to store token in keystore")?;
        self.state.config_store.add_domain_token(domain, token_account)?;
        Ok(())
    }

    pub async fn list_allowlist_domains(&self) -> Vec<String> {
        self.state.domain_gate.list_allowlist().await
    }

    pub fn list_allowlist_entries(&self) -> Vec<AllowlistEntry> {
        self.state.config_store.list_allowlist_entries().unwrap_or_else(|e| {
            warn!("Failed to list allowlist entries: {}", e);
            vec![]
        })
    }

    pub fn count_allowlist_entries(&self) -> u32 {
        self.state.config_store.count_allowlist_entries().unwrap_or_else(|e| {
            warn!("Failed to count allowlist entries: {}", e);
            0
        })
    }

    pub fn list_allowlist_entries_paginated(&self, page: u32, page_size: u32) -> Vec<AllowlistEntry> {
        self.state.config_store
            .list_allowlist_entries_paginated(page, page_size)
            .unwrap_or_else(|e| {
                warn!("Failed to list allowlist entries paginated: {}", e);
                vec![]
            })
    }

    pub async fn add_denylist_domain(&self, domain: &str) -> Result<()> {
        self.state.domain_gate.add_deny(domain).await;
        Ok(())
    }

    pub async fn remove_denylist_domain(&self, domain: &str) -> Result<()> {
        self.state.domain_gate.remove_deny(domain).await;
        Ok(())
    }

    pub async fn list_denylist_domains(&self) -> Vec<String> {
        self.state.domain_gate.list_denylist().await
    }

    // ── API Key Gate ──────────────────────────────────────────

    /// Resolve a pending API key request from the frontend modal.
    /// `key`: Some(secret) to save & provide, None to cancel.
    pub async fn resolve_api_key_request(&self, account: &str, key: Option<String>) {
        self.state.api_key_gate.resolve(account, key).await;
    }

    // ── Store ─────────────────────────────────────────────────

    pub async fn store_list_catalog(&self) -> Vec<StoreItem> {
        self.state.store.list_catalog().await
    }

    pub async fn store_install(&self, item_id: &str) -> Result<()> {
        self.state.store.install(item_id).await
    }

    pub async fn store_uninstall(&self, item_id: &str) -> Result<()> {
        // Clean up function keys associated with this app
        let _ = self.state.config_store.delete_all_function_keys_by_app(item_id);
        self.state.store.uninstall(item_id).await
    }

    pub async fn store_list_installed(&self) -> Vec<InstalledItem> {
        self.state.store.list_installed().await
    }

    /// List installed apps for a VM from the persistent DB.
    pub fn store_list_installed_apps(&self, vm_id: &VmId) -> Result<Vec<InstalledItem>> {
        let records = self.state.config_store.list_installed_apps(vm_id)?;
        Ok(records.into_iter().map(|r| InstalledItem {
            item_id: r.app_id,
            name: r.name,
            version: r.version,
            installed_at: Some(r.installed_at),
        }).collect())
    }

    // ── Function Key helpers ──────────────────────────────────

    pub fn list_function_keys(&self, vm_id: &str) -> Vec<crate::config_store::FunctionKeyRecord> {
        self.state.config_store.list_function_keys(vm_id).unwrap_or_default()
    }

    pub fn add_function_key(&self, vm_id: &str, label: &str, bash: &str) -> Result<()> {
        self.state.config_store.insert_function_key(vm_id, label, bash, None, None)?;
        Ok(())
    }

    pub fn remove_function_key(&self, key_id: i64) -> Result<()> {
        self.state.config_store.delete_function_key(key_id)
    }

    /// Install an app from a store manifest URL.
    ///
    /// 1. Download + verify manifest (host-side SHA256 check)
    /// 2. Send VSOCK `install_app` command with uuid + manifest_url
    /// 3. Spawn background reader to stream output as Tauri events
    /// 4. Return uuid for frontend correlation
    pub async fn store_install_app(
        &self,
        vm_id: &VmId,
        manifest_url: &str,
        verify_config: Option<crate::store::InstallVerifyConfig>,
    ) -> Result<String> {
        use crate::store::envelope::parse_envelope;
        use crate::store::verify::verify_envelope;

        // 1. Fetch manifest from store. fetch_store_url scopes the bearer to
        // STORE_BASE_URL hosts and refreshes once on 401.
        let resp = self.fetch_store_url(manifest_url).await
            .map_err(|e| anyhow!("Failed to fetch manifest: {}", e))?;
        if !resp.status().is_success() {
            return Err(anyhow!("Manifest fetch failed: HTTP {}", resp.status()));
        }
        let text = resp.text().await
            .map_err(|e| anyhow!("Failed to read manifest body: {}", e))?;

        let raw: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| anyhow!("Failed to parse manifest JSON: {}", e))?;

        let envelope = parse_envelope(&raw)
            .map_err(|e| anyhow!("Failed to parse manifest envelope: {}", e))?;

        let manifest_value = verify_envelope(&envelope)
            .map_err(|e| anyhow!("Manifest verification failed: {}", e))?
            .clone();

        let manifest_sha256 = match &envelope {
            crate::store::envelope::ManifestEnvelope::V3(v3) => v3.inner.signed_payload.manifest_sha256.clone(),
            crate::store::envelope::ManifestEnvelope::V2(v2) => v2.signed_payload.manifest_sha256.clone(),
            crate::store::envelope::ManifestEnvelope::Legacy(p) => p.manifest_sha256.clone(),
        };

        // Type guard
        let manifest_type = manifest_value["type"]
            .as_str()
            .unwrap_or("");
        if manifest_type != "application" {
            return Err(anyhow!(
                "Invalid manifest type: expected 'application', got '{}'",
                manifest_type
            ));
        }

        // Check min_disk requirement before proceeding
        if let Some(min_disk_mb) = manifest_value["min_disk"].as_u64() {
            if min_disk_mb > 0 {
                match self.get_vm_fs_info(vm_id).await {
                    Ok(fs_info) => {
                        if (fs_info.avail_mb as u64) < min_disk_mb {
                            let need_mb = min_disk_mb - fs_info.avail_mb as u64;
                            let need_gb = (need_mb + 1023) / 1024;
                            return Err(anyhow!(
                                "Insufficient disk space: this app requires at least {} GB of free disk space, \
                                 but only {} MB is available. \
                                 Please add at least {} GB via VM Manager (Resize Disk) before installing.",
                                (min_disk_mb + 1023) / 1024,
                                fs_info.avail_mb,
                                need_gb
                            ));
                        }
                    }
                    Err(e) => {
                        warn!("Could not check VM disk space ({}), proceeding with install", e);
                    }
                }
            }
        }

        // Extract app id and name from manifest
        let app_id = manifest_value["id"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing 'id' field in manifest"))?
            .to_string();

        // Step 3: Validate app_id to prevent path traversal / injection
        if !validate::is_valid_identifier(&app_id) {
            return Err(anyhow!("Invalid app id: '{}' — only [a-zA-Z0-9._-] allowed", app_id));
        }
        let app_name = manifest_value["name"]
            .as_str()
            .unwrap_or(&app_id)
            .to_string();

        debug!("Host-side manifest verified: {} (id={})", manifest_sha256.get(..16).unwrap_or(&manifest_sha256), app_id);

        // 2. Register admin URLs from manifest immediately (before VM communication may fail)
        {
            let admin_entries: Vec<(String, String)> = manifest_value["admin"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|entry| {
                            let url = entry["url"].as_str()?.to_string();
                            // Reject non-HTTP(S) URLs to prevent javascript:/data: XSS
                            if !validate::is_valid_http_url(&url) {
                                warn!("Skipping admin URL with invalid scheme: {}", url);
                                return None;
                            }
                            let label = entry["label"].as_str().unwrap_or("").to_string();
                            Some((url, label))
                        })
                        .collect()
                })
                .unwrap_or_default();
            if !admin_entries.is_empty() {
                let vms = self.state.vms.read().await;
                if let Some(vm) = vms.get(vm_id.as_str()) {
                    let mut urls = vm.admin_urls.write().await;
                    for (url, label) in &admin_entries {
                        if urls.iter().any(|r| r.url == *url) {
                            debug!("Admin URL already registered, skipping: {}", url);
                            continue;
                        }
                        match self.state.config_store.insert_vm_admin_url(vm_id.as_str(), url, label) {
                            Ok(new_id) => {
                                urls.push(AdminUrlRecord {
                                    id: new_id,
                                    url: url.clone(),
                                    label: label.clone(),
                                });
                                debug!("Registered admin URL: {} (id={})", url, new_id);
                            }
                            Err(e) => warn!("Failed to register admin URL: {}", e),
                        }
                    }
                    // Notify frontend so it can refresh the VM list and show the new admin menu
                    emit_typed(&self.emitter, "admin-urls-changed", &serde_json::json!({ "vm_id": vm_id }));
                }
            }
        }

        // 2b. Register function keys from manifest immediately (before VM communication may fail)
        {
            let app_functions: Vec<(String, String)> = manifest_value["functions"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|f| {
                            let label = f["label"].as_str()?.to_string();
                            let bash = f["bash"].as_str()?.to_string();
                            Some((label, bash))
                        })
                        .collect()
                })
                .unwrap_or_default();

            if !app_functions.is_empty() {
                let _ = self.state.config_store.delete_function_keys_by_app(vm_id.as_str(), &app_id);
                for (label, bash) in &app_functions {
                    if let Err(e) = self.state.config_store.insert_function_key(
                        vm_id.as_str(), label, bash, Some(&app_id), Some(&app_name),
                    ) {
                        warn!("Failed to insert function key '{}': {}", label, e);
                    }
                }
                emit_typed(&self.emitter, "function-keys-changed", &serde_json::json!({ "vm_id": vm_id }));
            }
        }

        // 3. Generate UUID
        let uuid = uuid::Uuid::new_v4().to_string();

        // 4. Get multiplexer for the active VM
        let instance = self.get_vm(vm_id).await?;

        let mux = {
            let lock = instance.multiplexer.read().await;
            lock.clone().ok_or_else(|| anyhow!(
                "VM is not connected. Please start the VM and wait for it to be ready."
            ))?
        };

        // 5. Open control stream and send install command (send id only, VM constructs URL)
        const CONTROL_PORT: u32 = 9402;
        let mut stream = mux.create_stream(CONTROL_PORT).await
            .map_err(|e| anyhow!("Failed to open control stream: {}", e))?;

        let mut cmd = serde_json::json!({
            "action": "install_app",
            "uuid": &uuid,
            "id": &app_id,
        });
        if let Some(ref vc) = verify_config {
            cmd["verify_token"] = serde_json::Value::String(vc.verify_token.clone());
            cmd["store_callback_url"] = serde_json::Value::String(vc.callback_url.clone());
        }
        stream.write(cmd.to_string().as_bytes()).await
            .map_err(|e| anyhow!("Failed to send install command: {}", e))?;

        // 6. Read accepted response
        let resp_data = stream.read().await
            .map_err(|e| anyhow!("Failed to read install response: {}", e))?;
        let resp_text = String::from_utf8_lossy(&resp_data);
        debug!("Install command response: {}", resp_text);

        let resp_json: serde_json::Value = serde_json::from_str(&resp_text)
            .map_err(|e| anyhow!("Invalid install response: {}", e))?;
        if resp_json["status"].as_str() != Some("accepted") {
            return Err(anyhow!("Install command rejected: {}", resp_text));
        }

        // 7. Spawn background reader for streaming output
        let emitter = self.emitter.clone();
        let uuid_clone = uuid.clone();
        let state = self.state.clone();
        let vm_id_clone = vm_id.to_string();
        let app_name_clone = app_name.clone();
        let app_id_clone = app_id.clone();
        tokio::spawn(async move {
            loop {
                match stream.read().await {
                    Ok(data) => {
                        let text = String::from_utf8_lossy(&data);
                        // Try parsing as done message first
                        if let Ok(done) = serde_json::from_str::<AppInstallDone>(&text) {
                            emit_typed(&emitter, "app-install-done", &done);
                            debug!("App install done: uuid={}, success={}", done.uuid, done.success);

                            // On success, persist to DB and append to VM description
                            if done.success {
                                if let Err(e) = state.config_store.upsert_installed_app(
                                    &vm_id_clone, &app_id_clone, &app_name_clone, "latest",
                                ) {
                                    warn!("Failed to persist installed app: {}", e);
                                }
                                if let Err(e) = append_vm_description(&state, &vm_id_clone, &app_name_clone).await {
                                    warn!("Failed to update VM description: {}", e);
                                }
                            }
                            break;
                        }
                        // Try parsing as output line
                        if let Ok(output) = serde_json::from_str::<AppInstallOutput>(&text) {
                            emit_typed(&emitter, "app-install-output", &output);
                        } else {
                            warn!("Unrecognized install stream data: {}", text);
                        }
                    }
                    Err(e) => {
                        error!("Install stream read error: {}", e);
                        emit_typed(&emitter, "app-install-done", &AppInstallDone {
                            uuid: uuid_clone.clone(),
                            success: false,
                            exit_code: -1,
                            error: Some(e.to_string()),
                        });
                        break;
                    }
                }
            }
        });

        Ok(uuid)
    }

    // ── Store Auth ────────────────────────────────────────────

    pub async fn store_begin_login(&self) -> Result<String> {
        self.state.store_auth.begin_login().await
    }

    pub async fn store_begin_login_browser(&self) -> Result<()> {
        self.state.store_auth.begin_login_with_browser().await
    }

    pub fn store_cancel_login(&self) {
        self.state.store_auth.cancel_login();
    }

    pub async fn store_login(&self) -> Result<AuthStatus> {
        self.state.store_auth.login().await
    }

    pub async fn store_logout(&self) {
        self.state.store_auth.logout().await;
    }

    pub async fn store_auth_status(&self) -> AuthStatus {
        // Deferred from startup: restore persisted session on first call
        self.state.store_auth.ensure_restored().await;
        self.state.store_auth.auth_status().await
    }

    /// Returns in-memory auth state only — no keyring access.
    pub async fn store_auth_status_memory_only(&self) -> AuthStatus {
        self.state.store_auth.auth_status().await
    }

    /// Initializes the LazyKeyStore (triggers macOS master-key keychain prompt).
    pub async fn warmup_keystore(&self) {
        let _ = self.state.keystore.list().await;
    }

    pub async fn store_access_token(&self) -> Option<String> {
        self.state.store_auth.access_token().await
    }

    /// GET a store URL with the current bearer token, transparently refreshing
    /// once on 401. Bearer is only attached when `url` targets `STORE_BASE_URL`.
    async fn fetch_store_url(&self, url: &str) -> reqwest::Result<reqwest::Response> {
        let client = reqwest::Client::new();
        let is_store = url.starts_with(crate::store::STORE_BASE_URL);

        let send_with_token = |token: Option<String>| {
            let mut req = client.get(url);
            if let Some(t) = token {
                req = req.header("Authorization", format!("Bearer {}", t));
            }
            req.send()
        };

        let token = if is_store { self.state.store_auth.access_token().await } else { None };
        let resp = send_with_token(token).await?;

        // Server may have revoked the token even though our cached `exp` is in
        // the future. Force a refresh and retry once.
        if is_store && resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            warn!("fetch_store_url: 401 from {}, attempting one refresh+retry", url);
            if let Some(fresh) = self.state.store_auth.force_refresh_access_token().await {
                return send_with_token(Some(fresh)).await;
            }
        }
        Ok(resp)
    }

    /// JWT access token에서 `verified` 클레임을 추출합니다.
    ///
    /// # Trust model (Step 5)
    /// The JWT is received over HTTPS from the store server and stored in a
    /// SQLCipher-encrypted database.  We do **not** verify the signature here
    /// because (a) the token never crosses an untrusted boundary after receipt,
    /// and (b) we lack the server's public key at this layer.
    /// As defense-in-depth, `verified` is restricted to known values only.
    async fn extract_verified_from_token(&self) -> Option<String> {
        let token = self.state.store_auth.access_token().await?;
        // JWT payload는 두 번째 base64url 파트
        let payload = token.splitn(3, '.').nth(1)?;
        // padding 추가 후 decode
        let padded = format!("{}{}", payload, "=".repeat((4 - payload.len() % 4) % 4));
        use base64::{Engine as _, engine::general_purpose::URL_SAFE};
        let decoded = URL_SAFE.decode(&padded).ok()?;
        let claims: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
        let value = claims.get("verified")?.as_str()?;
        // Only accept known claim values
        match value {
            "admin" | "user" => Some(value.to_string()),
            other => {
                warn!("Unexpected 'verified' claim value: {}", other);
                None
            }
        }
    }

    // ── MCP Bridge ────────────────────────────────────────────

    pub async fn mcp_register(&self, config: McpServerConfig) -> Result<String> {
        self.state.mcp_bridge.register(config).await
    }

    pub async fn mcp_unregister(&self, id: &str) -> Result<()> {
        self.state.mcp_bridge.unregister(id).await
    }

    pub async fn mcp_list(&self) -> Vec<crate::mcp_bridge::McpServerInfo> {
        self.state.mcp_bridge.list().await
    }

    pub async fn mcp_generate_claude_config(&self) -> serde_json::Value {
        self.state.mcp_bridge.generate_claude_config().await
    }

    // ── Monitoring ────────────────────────────────────────────

    pub fn get_vm_metrics(&self) -> VmMetrics {
        self.state.monitoring.get_metrics()
    }

    pub fn subscribe_metrics(&self) -> tokio::sync::watch::Receiver<VmMetrics> {
        self.state.monitoring.subscribe()
    }

    // ── Audit Log ─────────────────────────────────────────────

    pub async fn audit_query(&self, limit: Option<usize>) -> Vec<crate::audit::AuditEntry> {
        let filter = crate::audit::AuditFilter {
            action_type: None,
            limit,
        };
        self.state.audit_log.query(&filter).await
    }

    pub async fn audit_export_json(&self) -> Result<Vec<u8>> {
        self.state.audit_log.export_json().await
    }

    // ── Recovery ──────────────────────────────────────────────

    pub async fn recovery_enable(&self, vm_id: &str) -> Result<()> {
        self.state.recovery.enable(vm_id).await
    }

    pub async fn recovery_disable(&self, vm_id: &str) {
        self.state.recovery.disable(vm_id).await;
    }

    pub async fn recovery_status(&self, vm_id: &str) -> RecoveryState {
        self.state.recovery.status(vm_id).await
    }

    // ── SSH Gateway ───────────────────────────────────────────

    pub async fn ssh_gateway_enable(&self, vm_id: &str, host_port: u16) -> Result<()> {
        let instance = self.get_vm(vm_id).await?;
        let mux = {
            let lock = instance.multiplexer.read().await;
            lock.clone().ok_or_else(|| anyhow!("VSOCK not connected for VM {}", vm_id))?
        };
        self.state.ssh_gateway.enable(vm_id, host_port, mux).await?;
        self.state.audit_log.record(AuditAction::PortMappingAdded {
            host_port,
            vm_port: 22,
        }).await;
        Ok(())
    }

    pub async fn ssh_gateway_disable(&self, vm_id: &str) {
        self.state.ssh_gateway.disable(vm_id).await;
    }

    pub async fn ssh_gateway_status(&self, vm_id: &str) -> Option<u16> {
        self.state.ssh_gateway.status(vm_id).await
    }

    // ── File Mapping (FUSE) ────────────────────────────────────

    /// Change the shared directory path for a specific file proxy.
    pub async fn change_shared_path(&self, vm_id: &VmId, mapping_id: i64, new_path: std::path::PathBuf) -> Result<bool> {
        let instance = self.get_vm(vm_id).await?;
        let proxies = instance.file_proxies.read().await;
        let proxy = proxies.get(&mapping_id)
            .ok_or_else(|| anyhow!("File proxy not active for mapping {} on VM {}", mapping_id, vm_id))?;
        let pm_arc = proxy.path_manager();
        let mut pm = pm_arc.write().await;
        pm.request_path_change(new_path).await
    }

    /// Get current path state for a specific file proxy.
    pub async fn get_path_state(&self, vm_id: &VmId, mapping_id: i64) -> Result<(String, String)> {
        let instance = self.get_vm(vm_id).await?;
        let proxies = instance.file_proxies.read().await;
        let proxy = proxies.get(&mapping_id)
            .ok_or_else(|| anyhow!("File proxy not active for mapping {} on VM {}", mapping_id, vm_id))?;
        let pm_arc = proxy.path_manager();
        let pm = pm_arc.read().await;
        let state_str = match pm.state() {
            PathState::Active => "active",
            PathState::Pending => "pending",
            PathState::Switching => "switching",
        };
        Ok((state_str.to_string(), pm.current_path().to_string_lossy().to_string()))
    }

    /// Force switch the shared path even with open handles.
    pub async fn force_switch_path(&self, vm_id: &VmId, mapping_id: i64) -> Result<()> {
        let instance = self.get_vm(vm_id).await?;
        let proxies = instance.file_proxies.read().await;
        let proxy = proxies.get(&mapping_id)
            .ok_or_else(|| anyhow!("File proxy not active for mapping {} on VM {}", mapping_id, vm_id))?;
        let pm_arc = proxy.path_manager();
        let mut pm = pm_arc.write().await;
        pm.force_switch().await
    }

    /// Cancel a pending path change.
    pub async fn cancel_path_change(&self, vm_id: &VmId, mapping_id: i64) -> Result<()> {
        let instance = self.get_vm(vm_id).await?;
        let proxies = instance.file_proxies.read().await;
        let proxy = proxies.get(&mapping_id)
            .ok_or_else(|| anyhow!("File proxy not active for mapping {} on VM {}", mapping_id, vm_id))?;
        let pm_arc = proxy.path_manager();
        let mut pm = pm_arc.write().await;
        pm.cancel_path_change().await;
        Ok(())
    }

    // ── Internal helpers ──────────────────────────────────────

    async fn get_vm(&self, id: &str) -> Result<Arc<VmInstance>> {
        let vms = self.state.vms.read().await;
        vms.get(id).cloned().ok_or_else(|| anyhow!("VM not found: {}", id))
    }
}

/// Append a tag (e.g. "Ollama installed") to a VM's description.
/// Skips if the tag already exists in the description.
async fn append_vm_description(state: &Arc<CoreState>, vm_id: &str, tag: &str) -> Result<()> {
    let vms = state.vms.read().await;
    let vm = vms.get(vm_id).ok_or_else(|| anyhow!("VM not found: {}", vm_id))?;

    let current = vm.description.read().await.clone();
    if let Some(ref desc) = current {
        if desc.contains(tag) {
            return Ok(()); // already present
        }
    }

    let new_desc = match current {
        Some(ref d) if !d.is_empty() => format!("{}, {}", d, tag),
        _ => tag.to_string(),
    };

    let mut record = state.config_store.get_vm(vm_id)?
        .ok_or_else(|| anyhow!("VM record not found: {}", vm_id))?;
    record.description = Some(new_desc.clone());
    state.config_store.update_vm(&record)?;

    *vm.description.write().await = Some(new_desc);
    debug!("VM {} description updated with: {}", vm_id, tag);
    Ok(())
}

// ── HostConnect: raw TCP tunnel to host localhost ──────────────

const CDP_PORT: u32 = 9222;

/// Tracks the auto-launched CDP browser process and its headless/headed mode.
/// Shared across concurrent HostConnect handlers for the same VM.
struct CdpBrowserHandle {
    child: Option<std::process::Child>,
    headless: bool,
    /// Pending idle-kill timer: aborted when a new connection arrives within the timeout.
    kill_timer: Option<tokio::task::AbortHandle>,
}

impl CdpBrowserHandle {
    fn new() -> Self {
        Self { child: None, headless: true, kill_timer: None }
    }

    /// Cancel any pending idle-kill timer.
    fn cancel_kill_timer(&mut self) {
        if let Some(h) = self.kill_timer.take() {
            h.abort();
        }
    }
}

impl Drop for CdpBrowserHandle {
    /// Kill headless Chrome when nilbox exits (NilBoxService is dropped).
    /// Headed Chrome is left running — the user launched it intentionally.
    fn drop(&mut self) {
        if self.headless {
            if let Some(mut child) = self.child.take() {
                let _ = child.kill();
            }
            kill_nilbox_cdp_chrome_by_profile(&cdp_profile_dir(true));
        }
    }
}

async fn handle_host_connect_stream(
    mut stream: crate::vsock::stream::VirtualStream,
    config_store: std::sync::Arc<ConfigStore>,
    cdp_handle: std::sync::Arc<std::sync::Mutex<CdpBrowserHandle>>,
) -> anyhow::Result<()> {
    use crate::vsock::VsockStream;

    let payload = stream.read().await?;
    if payload.len() < 4 {
        let _ = stream.close().await;
        return Err(anyhow!("Invalid HostConnect payload"));
    }
    let target_port = u32::from_be_bytes(payload[..4].try_into().unwrap());
    // mode byte: 0x00=Auto, 0x01=Headless, 0x02=Headed (default Auto for older VM agents)
    let cdp_mode = payload.get(4).copied().unwrap_or(0x00);
    debug!("HostConnect: tunneling to localhost:{} (mode=0x{:02X})", target_port, cdp_mode);

    let addr = format!("127.0.0.1:{}", target_port);

    if target_port == CDP_PORT {
        // Cancel any pending idle-kill timer — a new connection is arriving.
        cdp_handle.lock().unwrap().cancel_kill_timer();

        // Resolve effective headless flag
        let effective_headless = match config_store.get_cdp_open_mode().as_str() {
            "headed"   => false,
            "headless" => true,
            _ => cdp_mode != 0x02,
        };

        // Determine replacement hostname for CDP JSON response rewriting.
        // Must match what the VM client used so returned ws:// URLs are resolvable.
        let replacement_host = match cdp_mode {
            0x02 => "headed.cdp.nilbox",
            0x01 => "headless.cdp.nilbox",
            _    => "cdp.nilbox",
        };

        // Obtain a live TCP connection to Chrome (launches / restarts if necessary).
        let tcp = connect_cdp_tcp(&config_store, &cdp_handle, &addr, effective_headless).await?;

        // Forward CDP traffic; blocks until tunnel closes.
        let result = crate::gateway::cdp_rewriter::forward_cdp_connection(
            tcp, Box::new(stream), replacement_host,
        ).await;

        // After tunnel closes: schedule an idle-kill timer for headless auto-launched Chrome.
        // If a new connection arrives within 60 s the timer is cancelled (see above).
        // Headed Chrome is left running so the user can keep seeing the browser window.
        schedule_headless_idle_kill(&cdp_handle);

        return result;
    }

    // Non-CDP port: plain TCP tunnel
    match tokio::net::TcpStream::connect(&addr).await {
        Ok(tcp) => crate::gateway::forwarder::forward_connection(tcp, Box::new(stream)).await,
        Err(e) => Err(anyhow!("HostConnect: failed to connect to {}: {}", addr, e)),
    }
}

/// Obtain a live TCP connection to the CDP browser, launching or restarting as needed.
async fn connect_cdp_tcp(
    config_store: &ConfigStore,
    cdp_handle: &std::sync::Arc<std::sync::Mutex<CdpBrowserHandle>>,
    addr: &str,
    effective_headless: bool,
) -> anyhow::Result<tokio::net::TcpStream> {
    match tokio::net::TcpStream::connect(addr).await {
        Ok(_) => {
            let (running_headless, is_auto_launched) = {
                let h = cdp_handle.lock().unwrap();
                (h.headless, h.child.is_some())
            };
            let is_nilbox_chrome = is_auto_launched || kill_nilbox_cdp_chrome_check();

            if running_headless != effective_headless && is_nilbox_chrome {
                // Nilbox-owned browser is running in wrong mode — kill and restart.
                debug!(
                    "CDP mode switch: headless={} → headless={}, restarting nilbox Chrome",
                    running_headless, effective_headless
                );
                {
                    let mut h = cdp_handle.lock().unwrap();
                    h.cancel_kill_timer();
                    if let Some(mut child) = h.child.take() {
                        let _ = child.kill();
                    }
                }
                kill_nilbox_cdp_chrome_by_profile(&cdp_profile_dir(running_headless));
                // Wait for port to close (up to 3s)
                for _ in 0..10 {
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    if tokio::net::TcpStream::connect(addr).await.is_err() { break; }
                }
                let child = cdp_auto_launch(config_store, effective_headless)?;
                { let mut h = cdp_handle.lock().unwrap(); h.child = Some(child); h.headless = effective_headless; }
                for i in 0..10 {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    if let Ok(tcp) = tokio::net::TcpStream::connect(addr).await {
                        debug!("CDP browser restarted (headless={}) after {} retries", effective_headless, i + 1);
                        return Ok(tcp);
                    }
                }
                return Err(anyhow!("CDP browser failed to restart on port 9222"));
            } else {
                if running_headless != effective_headless {
                    warn!(
                        "CDP mode mismatch: requested headless={} but user-launched browser is running as headless={}. \
                         Close the browser to switch mode.",
                        effective_headless, running_headless
                    );
                }
                tokio::net::TcpStream::connect(addr).await
                    .map_err(|e| anyhow!("HostConnect: reconnect failed: {}", e))
            }
        }
        Err(e) => {
            debug!("CDP port 9222 not listening, auto-launching (headless={}): {}", effective_headless, e);
            let child = cdp_auto_launch(config_store, effective_headless)?;
            { let mut h = cdp_handle.lock().unwrap(); h.child = Some(child); h.headless = effective_headless; }
            for i in 0..10 {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                if let Ok(tcp) = tokio::net::TcpStream::connect(addr).await {
                    debug!("CDP browser ready after {} retries", i + 1);
                    return Ok(tcp);
                }
            }
            Err(anyhow!("CDP browser failed to start on port 9222"))
        }
    }
}

/// After a CDP tunnel closes, schedule an idle-kill timer for headless auto-launched Chrome.
/// The timer fires after 60 s of no new connections; headed Chrome is not touched.
fn schedule_headless_idle_kill(cdp_handle: &std::sync::Arc<std::sync::Mutex<CdpBrowserHandle>>) {
    let is_headless_child = {
        let h = cdp_handle.lock().unwrap();
        h.headless && h.child.is_some()
    };
    if !is_headless_child {
        return;
    }

    const IDLE_KILL_SECS: u64 = 120;
    debug!("CDP tunnel closed: headless Chrome will be killed in {}s if no reconnection", IDLE_KILL_SECS);

    let handle_clone = cdp_handle.clone();
    let task = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(IDLE_KILL_SECS)).await;
        // Re-check under lock — a mode switch or new launch may have replaced the child.
        let should_kill = {
            let h = handle_clone.lock().unwrap();
            h.headless && h.child.is_some()
        };
        if should_kill {
            debug!("CDP idle timeout: killing headless Chrome ({}s with no reconnection)", IDLE_KILL_SECS);
            {
                let mut h = handle_clone.lock().unwrap();
                if let Some(mut child) = h.child.take() {
                    let _ = child.kill();
                }
                h.kill_timer = None;
            }
            kill_nilbox_cdp_chrome_by_profile(&cdp_profile_dir(true));
        }
    });

    cdp_handle.lock().unwrap().kill_timer = Some(task.abort_handle());
}

/// CDP profile dirs used by nilbox auto-launched Chrome instances.
const CDP_PROFILE_HEADLESS: &str = "/tmp/nilbox-cdp-profile-headless";
const CDP_PROFILE_HEADED:   &str = "/tmp/nilbox-cdp-profile-headed";

fn cdp_profile_dir(headless: bool) -> String {
    #[cfg(target_os = "windows")]
    {
        let tmp = std::env::temp_dir();
        let name = if headless { "nilbox-cdp-profile-headless" } else { "nilbox-cdp-profile-headed" };
        tmp.join(name).to_string_lossy().to_string()
    }
    #[cfg(not(target_os = "windows"))]
    {
        if headless { CDP_PROFILE_HEADLESS.to_string() } else { CDP_PROFILE_HEADED.to_string() }
    }
}

/// Returns true if a Chrome process running with the given nilbox CDP profile is detected.
/// Used to detect nilbox-owned Chrome even after the child handle is lost (e.g. after restart).
fn kill_nilbox_cdp_chrome_check() -> bool {
    #[cfg(target_os = "macos")]
    {
        // Check either profile — any nilbox-owned Chrome is relevant.
        for profile in [CDP_PROFILE_HEADLESS, CDP_PROFILE_HEADED] {
            if std::process::Command::new("pgrep")
                .args(["-f", profile])
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
            {
                return true;
            }
        }
        false
    }
    #[cfg(target_os = "windows")]
    {
        // Use PowerShell Get-CimInstance to find Chrome processes with nilbox CDP profile.
        for headless in [true, false] {
            let profile = cdp_profile_dir(headless);
            let query = format!(
                "Get-CimInstance Win32_Process -Filter \"commandline like '%{}%'\" | Select-Object -ExpandProperty ProcessId",
                profile.replace('\\', "\\\\").replace('\'', "''")
            );
            let output = std::process::Command::new("powershell")
                .args(["-NoProfile", "-Command", &query])
                .output();
            if let Ok(out) = output {
                let stdout = String::from_utf8_lossy(&out.stdout);
                if stdout.lines().any(|line| line.trim().parse::<u32>().is_ok()) {
                    return true;
                }
            }
        }
        false
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        false
    }
}

/// Kill Chrome processes that were launched with the specified nilbox CDP profile dir.
/// Handles multi-process Chrome (GPU/network helpers may outlive the browser process).
fn kill_nilbox_cdp_chrome_by_profile(profile_dir: &str) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("pkill")
            .args(["-f", profile_dir])
            .status();
    }
    #[cfg(target_os = "windows")]
    {
        // Use PowerShell Get-CimInstance to find and taskkill Chrome processes with the profile dir.
        let escaped = profile_dir.replace('\\', "\\\\").replace('\'', "''");
        let query = format!(
            "Get-CimInstance Win32_Process -Filter \"commandline like '%{}%'\" | Select-Object -ExpandProperty ProcessId",
            escaped
        );
        let output = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &query])
            .output();
        if let Ok(out) = output {
            let stdout = String::from_utf8_lossy(&out.stdout);
            for line in stdout.lines() {
                if let Ok(pid) = line.trim().parse::<u32>() {
                    let _ = std::process::Command::new("taskkill")
                        .args(["/F", "/T", "/PID", &pid.to_string()])
                        .status();
                }
            }
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = profile_dir;
}


/// Launch the CDP browser and return the child process handle.
/// headless/headed 각각 별도 user-data-dir 사용 → Chrome profile lock 충돌 방지
/// (macOS에서 같은 profile dir를 공유하면 Chrome이 기존 인스턴스로 라우팅)
fn cdp_auto_launch(config_store: &ConfigStore, headless: bool) -> anyhow::Result<std::process::Child> {
    let browser = config_store.get_cdp_browser();
    let path = resolve_cdp_browser_path(&browser)?;

    // Use separate profile dirs per mode to prevent Chrome profile lock conflicts.
    // When headless and headed share the same dir, Chrome detects the lock and routes
    // the new launch to the existing instance (macOS single-instance behavior),
    // which causes the existing browser to behave unexpectedly.
    let user_data_dir = cdp_profile_dir(headless);

    debug!("CDP Auto-Launch: {} headless={} profile={}", browser, headless, user_data_dir);

    let mut args = vec![
        "--remote-debugging-port=9222",
        "--no-first-run",
        "--no-default-browser-check",
        // Isolation flags: prevent interference with user's existing Chrome instance
        "--disable-sync",
        "--disable-extensions",
        "--disable-background-networking",
        "--disable-background-mode",
        "--no-service-autorun",
        "--metrics-recording-only",
        "--disable-default-apps",
        "--disable-popup-blocking",
    ];

    // macOS: prevent keychain sharing with existing Chrome instance
    #[cfg(target_os = "macos")]
    args.push("--use-mock-keychain");

    if headless {
        args.push("--headless=new");
    }

    std::process::Command::new(&path)
        .arg(format!("--user-data-dir={}", user_data_dir))
        .args(&args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| anyhow!("Failed to launch CDP browser '{}': {}", path, e))
}

fn resolve_cdp_browser_path(browser: &str) -> anyhow::Result<String> {
    #[cfg(target_os = "macos")]
    {
        match browser {
            "edge" => Ok("/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge".to_string()),
            _ => Ok("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".to_string()),
        }
    }
    #[cfg(target_os = "linux")]
    {
        match browser {
            "edge" => Ok("microsoft-edge".to_string()),
            _ => Ok("google-chrome".to_string()),
        }
    }
    #[cfg(target_os = "windows")]
    {
        match browser {
            "edge" => Ok(r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe".to_string()),
            _ => Ok(r"C:\Program Files\Google\Chrome\Application\chrome.exe".to_string()),
        }
    }
}
