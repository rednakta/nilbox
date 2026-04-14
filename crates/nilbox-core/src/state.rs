//! CoreState — central application state (Tauri-independent)
//!
//! Holds multi-VM instances, keystore, proxy, gateway, config,
//! and placeholders for Phase 4 modules (store, mcp_bridge, monitoring, audit, recovery).

use crate::audit::AuditLog;
use crate::config_store::{AdminUrlRecord, ConfigStore};
use crate::gateway::Gateway;
use crate::keystore::KeyStore;
use crate::mcp_bridge::McpBridgeManager;
use crate::monitoring::MonitoringCollector;
use crate::proxy::api_key_gate::ApiKeyGate;
use crate::proxy::auth_router::AuthRouter;
use crate::proxy::domain_gate::DomainGate;
use crate::proxy::token_mismatch_gate::TokenMismatchGate;
use crate::proxy::oauth_script_engine::OAuthScriptEngine;
use crate::proxy::oauth_token_vault::OAuthTokenVault;
use crate::proxy::llm_detector::LlmProviderMatcher;
use crate::recovery::RecoveryManager;
use crate::ssh;
use crate::ssh_gateway::SshGateway;
use crate::store::StoreManager;
use crate::store::auth::StoreAuth;
use crate::store::client::StoreClient;
use crate::file_proxy::FileProxy;
use crate::vm_platform::{VmPlatform, VmStatus};
use crate::vsock::stream::StreamMultiplexer;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{OnceCell, RwLock};
use tokio_util::sync::CancellationToken;
use russh::keys::PrivateKey;
use serde::{Serialize, Deserialize};

/// Unique VM instance identifier.
pub type VmId = String;

/// Per-VM instance state.
pub struct VmInstance {
    pub id: VmId,
    pub name: RwLock<String>,
    pub description: RwLock<Option<String>>,
    pub last_boot_at: RwLock<Option<String>>,
    pub created_at: String,
    pub memory_mb: RwLock<u32>,
    pub cpus: RwLock<u32>,
    pub base_os: Option<String>,
    pub base_os_version: Option<String>,
    pub target_platform: Option<String>,
    pub admin_urls: RwLock<Vec<AdminUrlRecord>>,
    pub platform: RwLock<Box<dyn VmPlatform>>,
    pub multiplexer: RwLock<Option<Arc<StreamMultiplexer>>>,
    pub ssh_client: RwLock<Option<ssh::client::SshClient>>,
    pub file_proxies: RwLock<HashMap<i64, Arc<FileProxy>>>,
    /// Cancellation token for the metrics collection background task.
    /// Wrapped in RwLock so it can be replaced on VM restart.
    pub metrics_cancel: RwLock<CancellationToken>,
    /// Disk image path (from VmRecord) — used to derive vm_dir for UI.
    pub disk_image: String,
}

impl VmInstance {
    pub async fn status(&self) -> VmStatus {
        self.platform.read().await.status_async().await
    }
}

/// Summary info returned to UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmInfo {
    pub id: VmId,
    pub name: String,
    pub status: VmStatus,
    pub ssh_ready: bool,
    pub description: Option<String>,
    pub last_boot_at: Option<String>,
    pub created_at: String,
    pub memory_mb: u32,
    pub cpus: u32,
    pub base_os: Option<String>,
    pub base_os_version: Option<String>,
    pub target_platform: Option<String>,
    pub admin_urls: Vec<AdminUrlRecord>,
    /// VM storage directory (parent of disk image).
    pub vm_dir: Option<String>,
}

/// Central application state — no Tauri dependency.
pub struct CoreState {
    /// Multi-VM instances
    pub vms: RwLock<HashMap<VmId, Arc<VmInstance>>>,
    /// Currently selected VM
    pub active_vm: RwLock<Option<VmId>>,
    /// Secure key store
    pub keystore: Arc<dyn KeyStore>,
    /// Port-forwarding gateway
    pub gateway: Arc<Gateway>,
    /// Persistent configuration (SQLite)
    pub config_store: Arc<ConfigStore>,
    /// App data directory
    pub app_data_dir: PathBuf,
    /// SSH keypair — initialized lazily on first VM start (after window is visible)
    /// to avoid triggering the macOS Keychain prompt before the window appears.
    pub ssh_keys: Arc<OnceCell<(Arc<PrivateKey>, String)>>,
    // Phase 4 modules
    /// API key gate (auto-detect auth headers + prompt user for missing keys)
    pub api_key_gate: Arc<ApiKeyGate>,
    /// Auth router (domain → delegator routing for vendor-specific auth)
    pub auth_router: Arc<AuthRouter>,
    /// App/MCP store catalog + installed items
    pub store: Arc<StoreManager>,
    /// Store authentication (polling login + JWT)
    pub store_auth: Arc<StoreAuth>,
    /// Authenticated store HTTP client (pinned TLS + signed downloads)
    pub store_client: Arc<StoreClient>,
    /// MCP bridge server management
    pub mcp_bridge: Arc<McpBridgeManager>,
    /// VM metrics collection
    pub monitoring: Arc<MonitoringCollector>,
    /// Audit log
    pub audit_log: Arc<AuditLog>,
    /// VM crash recovery
    pub recovery: Arc<RecoveryManager>,
    /// SSH gateway (TCP → VSOCK port 22)
    pub ssh_gateway: Arc<SshGateway>,
    /// Domain access gate (runtime allow/deny for HTTPS CONNECT)
    pub domain_gate: Arc<DomainGate>,
    /// Token mismatch gate (warn when request doesn't use mapped token)
    pub token_mismatch_gate: Arc<TokenMismatchGate>,
    /// Shared OAuth script engine — reloadable after store update
    pub oauth_engine: Arc<RwLock<Arc<OAuthScriptEngine>>>,
    /// OAuth token vault — stores real tokens, issues dummy tokens to VM
    pub oauth_vault: Arc<OAuthTokenVault>,
    /// LLM provider matcher — shared across proxy tasks, reloadable after manifest update
    pub llm_matcher: Arc<LlmProviderMatcher>,
    /// Absolute path to the bundled QEMU binary (Linux/Windows only).
    /// None → fall back to system PATH (dev environment or manual install).
    #[cfg(not(target_os = "macos"))]
    pub qemu_binary_path: Option<PathBuf>,
}

impl CoreState {
    /// Get the active VM instance.
    pub async fn active_vm_instance(&self) -> Option<Arc<VmInstance>> {
        let active_id = self.active_vm.read().await;
        if let Some(id) = active_id.as_ref() {
            let vms = self.vms.read().await;
            vms.get(id).cloned()
        } else {
            None
        }
    }
}
