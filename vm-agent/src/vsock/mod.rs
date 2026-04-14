//! VSOCK communication module (vm-agent)

pub mod protocol;
pub mod stream;

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "linux")]
pub mod virtio_serial;

use async_trait::async_trait;
use anyhow::Result;
use bytes::Bytes;

pub const CONTROL_PORT: u32 = 9402;
pub const OUTBOUND_PORT: u32 = 18088;

#[async_trait]
pub trait VsockConnector: Send + Sync {
    async fn connect(&self, cid: u32, port: u32) -> Result<Box<dyn VsockStream>>;
    async fn listen(&self, port: u32) -> Result<Box<dyn VsockListener>>;
}

#[async_trait]
pub trait VsockListener: Send + Sync {
    async fn accept(&mut self) -> Result<Box<dyn VsockStream>>;
}

#[async_trait]
pub trait VsockStream: Send + Sync {
    async fn read(&mut self) -> Result<Bytes>;
    async fn write(&mut self, data: &[u8]) -> Result<()>;
    async fn close(&mut self) -> Result<()>;
}
