//! QemuVmPlatform — wraps QEMU process to implement VmPlatform

use super::{VmConfig, VmPlatform, VmStatus};
use async_trait::async_trait;
use anyhow::{Result, anyhow};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::env;
use tokio::process::Command;
use tokio::sync::{RwLock, oneshot};
use tokio::io::{AsyncBufReadExt, BufReader};
use tracing::{debug, error, warn};

/// Default TCP port for QEMU socket chardev on Windows.
#[cfg(target_os = "windows")]
pub const QEMU_VSOCK_PORT: u16 = 19522;

pub struct QemuVmPlatform {
    config: Option<VmConfig>,
    stop_tx: Arc<RwLock<Option<oneshot::Sender<()>>>>,
    vm_status: Arc<RwLock<VmStatus>>,
    /// Absolute path to the bundled QEMU binary. None → fall back to system PATH.
    binary_path: Option<PathBuf>,
    #[cfg(target_os = "windows")]
    vsock_port: u16,
    #[cfg(target_os = "windows")]
    fw_cfg_token_hex: Option<String>,
    #[cfg(target_os = "linux")]
    fw_cfg_token_hex: Option<String>,
}

impl QemuVmPlatform {
    pub fn new() -> Self {
        Self {
            config: None,
            stop_tx: Arc::new(RwLock::new(None)),
            vm_status: Arc::new(RwLock::new(VmStatus::Stopped)),
            binary_path: None,
            #[cfg(target_os = "windows")]
            vsock_port: QEMU_VSOCK_PORT,
            #[cfg(target_os = "windows")]
            fw_cfg_token_hex: None,
            #[cfg(target_os = "linux")]
            fw_cfg_token_hex: None,
        }
    }

    /// Create with a pre-resolved absolute path to the QEMU binary (bundled sidecar).
    pub fn with_binary_path(path: PathBuf) -> Self {
        Self {
            config: None,
            stop_tx: Arc::new(RwLock::new(None)),
            vm_status: Arc::new(RwLock::new(VmStatus::Stopped)),
            binary_path: Some(path),
            #[cfg(target_os = "windows")]
            vsock_port: QEMU_VSOCK_PORT,
            #[cfg(target_os = "windows")]
            fw_cfg_token_hex: None,
            #[cfg(target_os = "linux")]
            fw_cfg_token_hex: None,
        }
    }

    #[cfg(target_os = "windows")]
    pub fn vsock_port(&self) -> u16 {
        self.vsock_port
    }

    #[cfg(target_os = "windows")]
    pub fn inject_fw_cfg_token(&mut self, token_hex: &str) {
        self.fw_cfg_token_hex = Some(token_hex.to_string());
        debug!("fw_cfg auth token staged for QEMU start");
    }

    #[cfg(target_os = "linux")]
    pub fn inject_fw_cfg_token(&mut self, token_hex: &str) {
        self.fw_cfg_token_hex = Some(token_hex.to_string());
        debug!("fw_cfg auth token staged for QEMU start");
    }

    fn build_args(config: &VmConfig) -> Vec<String> {
        let mut args = Vec::new();
        let arch = env::consts::ARCH;

        match (env::consts::OS, arch) {
            ("macos", "aarch64") => {
                args.extend(["-machine".into(), "virt,accel=hvf,highmem=on".into(),
                              "-cpu".into(), "host".into()]);
            }
            ("macos", "x86_64") => {
                args.extend(["-machine".into(), "q35,accel=hvf".into(),
                              "-cpu".into(), "host".into()]);
            }
            ("linux", _) => {
                args.extend(["-machine".into(), "pc,accel=kvm".into(),
                              "-cpu".into(), "host".into()]);
            }
            ("windows", _) => {
                args.extend([
                    "-accel".into(), "whpx,kernel-irqchip=off".into(),
                    "-cpu".into(), "qemu64".into(),
                    "-machine".into(), "q35".into(),
                ]);
            }
            _ => {}
        }

        args.extend(["-m".into(), format!("{}M", config.memory_mb),
                     "-smp".into(), config.cpus.to_string()]);

        args.extend(["-display".into(), "none".into(),
                     "-serial".into(), "stdio".into()]);

        if let Some(kernel) = &config.kernel {
            args.extend(["-kernel".into(), kernel.display().to_string()]);
            if let Some(initrd) = &config.initrd {
                args.extend(["-initrd".into(), initrd.display().to_string()]);
            }
            if let Some(append) = &config.append {
                // Linux uses -serial stdio (ttyS0), so replace hvc0 with ttyS0
                #[cfg(target_os = "linux")]
                let append = append.replace("console=hvc0", "console=ttyS0");
                #[cfg(target_os = "windows")]
                let append = append.clone();
                #[cfg(not(any(target_os = "linux", target_os = "windows")))]
                let append = append.clone();
                args.extend(["-append".into(), append]);
            }
        }

        if config.disk_image.exists() {
            #[cfg(target_os = "linux")]
            let drive_fmt = format!("file={},format=raw,if=virtio", config.disk_image.display());

            #[cfg(target_os = "windows")]
            let drive_fmt = format!("file={},format=raw,if=virtio", config.disk_image.display());

            #[cfg(not(any(target_os = "linux", target_os = "windows")))]
            let drive_fmt = format!("file={},if=virtio", config.disk_image.display());

            args.extend(["-drive".into(), drive_fmt]);
        }

        // NIC: Linux uses user-mode for debug, others disable
        #[cfg(target_os = "linux")]
        args.extend(["-nic".into(), "user".into()]);
        #[cfg(not(target_os = "linux"))]
        args.extend(["-nic".into(), "none".into()]);

        #[cfg(target_os = "linux")]
        args.extend(["-device".into(), "vhost-vsock-pci,guest-cid=3".into()]);

        #[cfg(target_os = "windows")]
        args.extend([
            "-device".into(), "virtio-serial-pci".into(),
            "-chardev".into(), format!(
                "socket,id=vsock0,host=127.0.0.1,port={}",
                QEMU_VSOCK_PORT
            ),
            "-device".into(), "virtserialport,chardev=vsock0,name=run.nilbox.vsock".into(),
        ]);

        args
    }

    /// Return binary name for the current architecture (no path — for PATH fallback).
    fn qemu_binary_name() -> &'static str {
        match env::consts::ARCH {
            "aarch64" => "qemu-system-aarch64",
            _ => "qemu-system-x86_64",
        }
    }

    /// Resolve the QEMU binary path.
    /// 1. Bundled path set via `with_binary_path()` (Tauri externalBin sidecar)
    /// 2. System PATH fallback
    fn resolve_binary(&self) -> PathBuf {
        if let Some(path) = &self.binary_path {
            return path.clone();
        }
        PathBuf::from(Self::qemu_binary_name())
    }
}

#[async_trait]
impl VmPlatform for QemuVmPlatform {
    async fn create(&mut self, config: VmConfig) -> Result<()> {
        self.config = Some(config);
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let config = self.config.as_ref().ok_or(anyhow!("No VM config set"))?;

        {
            let status = self.vm_status.read().await;
            if *status != VmStatus::Stopped && *status != VmStatus::Error {
                return Err(anyhow!("VM is not in stopped state"));
            }
        }

        *self.vm_status.write().await = VmStatus::Starting;

        #[cfg_attr(target_os = "macos", allow(unused_mut))]
        let mut args = Self::build_args(config);
        let binary = self.resolve_binary();

        // On Windows, BIOS/ROM files are bundled next to the QEMU binary
        #[cfg(target_os = "windows")]
        if let Some(dir) = binary.parent() {
            args.insert(0, dir.to_string_lossy().to_string());
            args.insert(0, "-L".to_string());
        }

        // On Linux, prepend -L <binary_dir> for bundled BIOS/ROM files
        // Only when using a bundled binary (not system PATH fallback).
        #[cfg(target_os = "linux")]
        if self.binary_path.is_some() {
            if let Some(dir) = binary.parent() {
                let dir_str = dir.to_string_lossy();
                if !dir_str.is_empty() {
                    args.insert(0, dir_str.to_string());
                    args.insert(0, "-L".to_string());
                }
            }
        }

        #[cfg(target_os = "windows")]
        let _fw_cfg_cleanup: Option<PathBuf> = if let Some(ref token_hex) = self.fw_cfg_token_hex {
            let token_path = std::env::temp_dir().join(format!("nilbox-auth-{}.hex", &token_hex[..8]));
            std::fs::write(&token_path, token_hex)
                .map_err(|e| anyhow!("Failed to create fw_cfg token file: {}", e))?;
            args.extend([
                "-fw_cfg".into(),
                format!("name=opt/nilbox/auth_token,file={}", token_path.display()),
            ]);
            debug!("fw_cfg token file created: {}", token_path.display());
            Some(token_path)
        } else {
            None
        };

        #[cfg(target_os = "linux")]
        let _fw_cfg_cleanup: Option<PathBuf> = if let Some(ref token_hex) = self.fw_cfg_token_hex {
            let token_path = std::env::temp_dir().join(format!("nilbox-auth-{}.hex", &token_hex[..8]));
            std::fs::write(&token_path, token_hex)
                .map_err(|e| anyhow!("Failed to create fw_cfg token file: {}", e))?;
            args.extend([
                "-fw_cfg".into(),
                format!("name=opt/nilbox/auth_token,file={}", token_path.display()),
            ]);
            debug!("fw_cfg token file created: {}", token_path.display());
            Some(token_path)
        } else {
            None
        };

        debug!("Starting QEMU: {} {:?}", binary.display(), args);

        let mut cmd = Command::new(&binary);
        cmd.args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Linux: -serial stdio needs stdin open (piped), others use null
        #[cfg(target_os = "linux")]
        cmd.stdin(Stdio::piped());
        #[cfg(not(target_os = "linux"))]
        cmd.stdin(Stdio::null());

        #[cfg(target_os = "windows")]
        {
            // Suppress the console window that Windows allocates for console-subsystem processes
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);

            // DLLs are placed in lib/ subdirectory next to qemu exe by Tauri installer
            let binary_dir = binary.parent()
                .unwrap_or(std::path::Path::new("."));
            let lib_dir = binary_dir.join("lib");
            let path_env = format!(
                "{};{};{}",
                lib_dir.display(),
                binary_dir.display(),
                std::env::var("PATH").unwrap_or_default()
            );
            cmd.env("PATH", path_env);
        }

        let child_res = cmd.spawn();

        let mut child = match child_res {
            Ok(c) => {
                debug!("QEMU spawned, pid: {:?}", c.id());
                c
            }
            Err(e) => {
                *self.vm_status.write().await = VmStatus::Error;
                return Err(anyhow!("Failed to spawn QEMU: {}", e));
            }
        };

        // QEMU reads fw_cfg files during early init — defer cleanup to avoid race
        #[cfg(target_os = "windows")]
        if let Some(path) = _fw_cfg_cleanup {
            tokio::spawn(async move {
                tokio::time::sleep(tokio::time::Duration::from_secs(20)).await;
                if let Err(e) = std::fs::remove_file(&path) {
                    warn!("Failed to clean up fw_cfg token file: {}", e);
                }
            });
        }

        #[cfg(target_os = "linux")]
        if let Some(path) = _fw_cfg_cleanup {
            tokio::spawn(async move {
                tokio::time::sleep(tokio::time::Duration::from_secs(20)).await;
                if let Err(e) = std::fs::remove_file(&path) {
                    warn!("Failed to clean up fw_cfg token file: {}", e);
                }
            });
        }

        if let Some(stdout) = child.stdout.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    debug!("[QEMU] {}", line);
                }
            });
        }
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    debug!("[QEMU stderr] {}", line);
                }
            });
        }

        let (tx, rx) = oneshot::channel();
        let status_clone = self.vm_status.clone();

        tokio::spawn(async move {
            tokio::select! {
                res = child.wait() => {
                    let mut status = status_clone.write().await;
                    match res {
                        Ok(exit) if exit.success() => *status = VmStatus::Stopped,
                        Ok(exit) => {
                            error!("QEMU exited: {}", exit);
                            *status = VmStatus::Error;
                        }
                        Err(e) => {
                            error!("QEMU wait error: {}", e);
                            *status = VmStatus::Error;
                        }
                    }
                }
                _ = rx => {
                    debug!("Stopping QEMU...");
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    *status_clone.write().await = VmStatus::Stopped;
                    debug!("QEMU stopped");
                }
            }
        });

        *self.stop_tx.write().await = Some(tx);
        *self.vm_status.write().await = VmStatus::Running;
        debug!("QEMU started successfully");
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

        let mut tx_guard = self.stop_tx.write().await;
        if let Some(tx) = tx_guard.take() {
            if tx.send(()).is_err() {
                warn!("QEMU monitor already finished");
                *self.vm_status.write().await = VmStatus::Stopped;
            }
        } else {
            *self.vm_status.write().await = VmStatus::Stopped;
        }

        Ok(())
    }

    async fn status_async(&self) -> VmStatus {
        *self.vm_status.read().await
    }

    fn status(&self) -> VmStatus {
        VmStatus::Stopped
    }

    fn vsock_socket_path(&self) -> Option<PathBuf> {
        #[cfg(target_os = "windows")]
        { return Some(PathBuf::from(format!("tcp:127.0.0.1:{}", self.vsock_port))); }
        #[cfg(not(target_os = "windows"))]
        { None }
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}
