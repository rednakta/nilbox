//! VSOCK communication module

pub mod protocol;
pub mod stream;
pub mod async_adapter;

#[cfg(target_os = "macos")]
pub mod apple;

#[cfg(target_os = "windows")]
pub mod named_pipe;

#[cfg(target_os = "linux")]
pub mod linux_vsock;

use async_trait::async_trait;
use anyhow::Result;
use bytes::Bytes;

pub const HOST_CID: u32 = 2;
pub const GUEST_CID: u32 = 3;
pub const OUTBOUND_PORT: u32 = 18088;

#[async_trait]
pub trait VsockConnector: Send + Sync {
    async fn connect(&self, cid: u32, port: u32) -> Result<Box<dyn VsockStream>>;
    async fn listen(&self, port: u32) -> Result<Box<dyn VsockListener>>;
}

#[async_trait]
pub trait VsockStream: Send + Sync {
    async fn read(&mut self) -> Result<Bytes>;
    async fn write(&mut self, data: &[u8]) -> Result<()>;
    async fn close(&mut self) -> Result<()>;
}

#[async_trait]
pub trait VsockListener: Send + Sync {
    async fn accept(&mut self) -> Result<Box<dyn VsockStream>>;
}
