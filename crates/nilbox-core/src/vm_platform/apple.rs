//! AppleVmPlatform — Apple Virtualization.framework via nilbox-vmm subprocess
//!
//! Spawns the `nilbox-vmm` Swift executable which runs VZVirtualMachine.
//! Communication via JSON lines on stdin/stdout.

use super::{VmConfig, VmPlatform, VmStatus};
use async_trait::async_trait;
use anyhow::{Result, anyhow};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::RwLock;
use tracing::{debug, error, warn};

pub struct AppleVmPlatform {
    config: Option<VmConfig>,
    relay_socket_path: PathBuf,
    relay_token: [u8; 32],
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    vm_status: Arc<RwLock<VmStatus>>,
}

impl AppleVmPlatform {
    pub fn new(relay_token: [u8; 32]) -> Self {
        Self {
            config: None,
            relay_socket_path: PathBuf::from("/tmp/nilbox-vsock-relay.sock"),
            relay_token,
            child: None,
            stdin: None,
            vm_status: Arc::new(RwLock::new(VmStatus::Stopped)),
        }
    }

    /// Set the relay authentication token (called before each start).
    pub fn set_relay_token(&mut self, token: [u8; 32]) {
        self.relay_token = token;
    }

    /// Inject agent auth token and kernel tuning into VmConfig.append for kernel cmdline.
    /// This is a runtime-only modification — the DB record is not changed.
    pub fn inject_cmdline_token(&mut self, token_hex: &str) {
        if let Some(ref mut config) = self.config {
            let base = config.append.clone().unwrap_or_default();
            // Strip previous auth token and watchdog params to prevent accumulation on restart
            let cleaned: String = base
                .split_whitespace()
                .filter(|p| {
                    !p.starts_with("nilbox.auth_token=")
                        && !p.starts_with("nmi_watchdog=")
                        && !p.starts_with("nohz=")
                        && !p.starts_with("idle=")
                        && !p.starts_with("maxcpus=")
                        && !p.starts_with("irqaffinity=")
                        && *p != "nosoftlockup"
                        && *p != "threadirqs"
                })
                .collect::<Vec<_>>()
                .join(" ");
            // Apple Virtualization.framework kernel tuning parameters:
            //   nmi_watchdog=0  — disable NMI watchdog (false hard-lockup detection)
            //   nosoftlockup    — disable soft-lockup detector
            //   threadirqs      — run hardirq handlers as preemptible kernel threads,
            //                     preventing indefinite CPU lockup during virtio bursts
            //   maxcpus=N       — match config.cpus; previously forced to 1 due to Apple VZ
            //                     IPI delivery issues, now configurable with relay socket
            //                     buffer tuning (1MB/4MB) mitigating virtio ring pressure.
            config.append = Some(format!(
                "{} nilbox.auth_token={} nmi_watchdog=0 nosoftlockup threadirqs maxcpus={}",
                cleaned.trim(), token_hex, config.cpus,
            ));
            debug!("Injected agent auth token + watchdog tuning into kernel cmdline");
        }
    }

    /// Map Rust arch name to Swift build directory name
    fn swift_arch_name() -> &'static str {
        match std::env::consts::ARCH {
            "aarch64" => "arm64",
            "x86_64" => "x86_64",
            other => other,
        }
    }

    /// Verify that a binary supports the current host architecture using `lipo -archs`
    fn binary_supports_current_arch(path: &std::path::Path) -> bool {
        let arch = Self::swift_arch_name();
        match std::process::Command::new("lipo")
            .args(["-archs", &path.to_string_lossy()])
            .output()
        {
            Ok(output) => {
                let archs = String::from_utf8_lossy(&output.stdout);
                archs.split_whitespace().any(|a| a == arch)
            }
            Err(_) => {
                // lipo not available — assume compatible
                true
            }
        }
    }

    fn vmm_binary_path() -> Result<PathBuf> {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../nilbox-vmm/.build");
        let arch = Self::swift_arch_name();

        // 1. Architecture-specific Swift build directory
        let arch_path = base.join(format!("{}-apple-macosx/release/nilbox-vmm", arch));
        if arch_path.exists() {
            debug!("Using arch-specific VMM binary: {:?}", arch_path);
            return Ok(arch_path);
        }

        // 2. Generic release path (may be universal or single-arch)
        let generic_path = base.join("release/nilbox-vmm");
        if generic_path.exists() {
            if Self::binary_supports_current_arch(&generic_path) {
                debug!("Using generic VMM binary: {:?}", generic_path);
                return Ok(generic_path);
            }
            warn!(
                "VMM binary at {:?} does not support current architecture ({})",
                generic_path, arch
            );
        }

        // 3. Bundle: next to the app executable
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let bundle_path = dir.join("nilbox-vmm");
                if bundle_path.exists() {
                    if Self::binary_supports_current_arch(&bundle_path) {
                        debug!("Using bundled VMM binary: {:?}", bundle_path);
                        return Ok(bundle_path);
                    }
                    warn!(
                        "Bundled VMM binary at {:?} does not support current architecture ({})",
                        bundle_path, arch
                    );
                }
            }
        }

        Err(anyhow!(
            "nilbox-vmm binary not found for architecture '{}'. \
             Build it with: cd nilbox-vmm && make release-{}",
            arch, arch
        ))
    }
}

#[async_trait]
impl VmPlatform for AppleVmPlatform {
    async fn create(&mut self, config: VmConfig) -> Result<()> {
        self.config = Some(config);
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let config = self.config.as_ref().ok_or_else(|| anyhow!("No VM config set"))?;

        {
            let status = self.vm_status.read().await;
            if *status != VmStatus::Stopped && *status != VmStatus::Error {
                return Err(anyhow!("VM is not in stopped state"));
            }
        }

        *self.vm_status.write().await = VmStatus::Starting;

        let vmm_path = Self::vmm_binary_path()?;
        debug!("Spawning VMM: {:?} (arch: {})", vmm_path, Self::swift_arch_name());

        let relay_token_hex: String = self.relay_token.iter().map(|b| format!("{:02x}", b)).collect();
        let start_cmd = serde_json::json!({
            "cmd": "start",
            "config": {
                "disk_image": config.disk_image.to_string_lossy(),
                "kernel": config.kernel.as_ref().map(|p| p.to_string_lossy().to_string()),
                "initrd": config.initrd.as_ref().map(|p| p.to_string_lossy().to_string()),
                "append": config.append.clone(),
                "memory_mb": config.memory_mb,
                "cpus": config.cpus,
                "relay_socket": self.relay_socket_path.to_string_lossy(),
                "relay_token": relay_token_hex,
            }
        });
        let mut start_line = serde_json::to_string(&start_cmd)?;
        debug!("VMM start command: {}", start_line);
        start_line.push('\n');

        let mut child = Command::new(&vmm_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| anyhow!("Failed to spawn VMM {:?}: {}", vmm_path, e))?;

        // Check that the process didn't exit immediately (e.g. architecture mismatch)
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        if let Some(exit_status) = child.try_wait()? {
            return Err(anyhow!(
                "VMM process exited immediately with status {}. \
                 This may indicate an architecture mismatch — \
                 current host is '{}'. Rebuild with: cd nilbox-vmm && make release-{}",
                exit_status,
                Self::swift_arch_name(),
                Self::swift_arch_name(),
            ));
        }

        let mut stdin = child.stdin.take().ok_or_else(|| anyhow!("No VMM stdin"))?;

        // Read stdout: wait for "ready" event before sending start command
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("No VMM stdout"))?;
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<()>();
        let status_clone = self.vm_status.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            let mut ready_tx = Some(ready_tx);
            while let Ok(Some(line)) = lines.next_line().await {
                debug!("[VMM] {}", line);
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&line) {
                    match val.get("event").and_then(|e| e.as_str()) {
                        Some("ready") => {
                            if let Some(tx) = ready_tx.take() {
                                let _ = tx.send(());
                            }
                        }
                        Some("started") => {
                            *status_clone.write().await = VmStatus::Running;
                            debug!("VM running (Apple Virtualization.framework)");
                        }
                        Some("stopped") => {
                            *status_clone.write().await = VmStatus::Stopped;
                            debug!("VM stopped");
                        }
                        Some("error") => {
                            let msg = val
                                .get("message")
                                .and_then(|m| m.as_str())
                                .unwrap_or("unknown error");
                            error!("VMM error: {}", msg);
                            *status_clone.write().await = VmStatus::Error;
                        }
                        _ => {}
                    }
                }
            }
            *status_clone.write().await = VmStatus::Stopped;
        });

        // Wait for "ready" event with timeout
        match tokio::time::timeout(
            tokio::time::Duration::from_secs(15),
            ready_rx,
        ).await {
            Ok(Ok(())) => {
                debug!("VMM process ready, sending start command");
            }
            Ok(Err(_)) => {
                return Err(anyhow!(
                    "VMM process stdout closed before sending ready event. \
                     Architecture mismatch? Host is '{}'.",
                    Self::swift_arch_name(),
                ));
            }
            Err(_) => {
                return Err(anyhow!(
                    "VMM process did not become ready within 15 seconds. \
                     Architecture mismatch? Host is '{}'.",
                    Self::swift_arch_name(),
                ));
            }
        }

        stdin.write_all(start_line.as_bytes()).await?;
        stdin.flush().await?;
        self.stdin = Some(stdin);

        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    debug!("[VMM stderr] {}", line);
                }
            });
        }

        self.child = Some(child);
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        {
            let status = self.vm_status.read().await;
            if *status == VmStatus::Stopped {
                return Ok(());
            }
        }

        *self.vm_status.write().await = VmStatus::Stopping;

        // Send graceful stop command
        if let Some(stdin) = &mut self.stdin {
            let stop_cmd = serde_json::json!({"cmd": "stop"});
            let mut stop_line = serde_json::to_string(&stop_cmd)?;
            stop_line.push('\n');
            let _ = stdin.write_all(stop_line.as_bytes()).await;
            let _ = stdin.flush().await;
        }

        // Drop stdin to signal EOF (VMM reads stdin closure as shutdown)
        self.stdin.take();

        // Wait for child to exit, force kill after timeout
        if let Some(ref mut child) = self.child {
            match tokio::time::timeout(
                tokio::time::Duration::from_secs(10),
                child.wait(),
            ).await {
                Ok(Ok(status)) => {
                    debug!("VMM exited: {}", status);
                }
                Ok(Err(e)) => {
                    warn!("VMM wait error: {}", e);
                }
                Err(_) => {
                    warn!("VMM did not exit within 10s, killing");
                    let _ = child.kill().await;
                }
            }
        }
        self.child.take();

        *self.vm_status.write().await = VmStatus::Stopped;
        debug!("VM stopped");
        Ok(())
    }

    async fn status_async(&self) -> VmStatus {
        *self.vm_status.read().await
    }

    fn status(&self) -> VmStatus {
        VmStatus::Stopped
    }

    fn vsock_socket_path(&self) -> Option<PathBuf> {
        Some(self.relay_socket_path.clone())
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}
