//! VmPlatform trait — OS-abstracted VM management

pub mod qemu;

#[cfg(target_os = "macos")]
pub mod apple;

use async_trait::async_trait;
use anyhow::Result;
use serde::{Serialize, Deserialize};
use std::path::PathBuf;

/// VM configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmConfig {
    pub disk_image: PathBuf,
    pub kernel: Option<PathBuf>,
    pub initrd: Option<PathBuf>,
    pub append: Option<String>,
    pub memory_mb: u32,
    pub cpus: u32,
}

impl Default for VmConfig {
    fn default() -> Self {
        Self {
            disk_image: PathBuf::new(),
            kernel: None,
            initrd: None,
            append: None,
            memory_mb: 512,
            cpus: 2,
        }
    }
}

/// VM lifecycle status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VmStatus {
    Stopped,
    Starting,
    Running,
    Stopping,
    Error,
}

/// Platform-agnostic VM management trait
#[async_trait]
pub trait VmPlatform: Send + Sync {
    /// Create the VM with the given config (may be called multiple times to reconfigure)
    async fn create(&mut self, config: VmConfig) -> Result<()>;

    /// Start the VM
    async fn start(&mut self) -> Result<()>;

    /// Stop the VM
    async fn stop(&mut self) -> Result<()>;

    /// Current VM status (async, preferred)
    async fn status_async(&self) -> VmStatus;

    /// Current VM status (sync fallback)
    fn status(&self) -> VmStatus;

    /// VSOCK relay/socket path if applicable
    fn vsock_socket_path(&self) -> Option<PathBuf>;

    /// Downcast to concrete type for platform-specific operations
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}
