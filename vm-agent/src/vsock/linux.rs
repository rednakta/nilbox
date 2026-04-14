//! Linux VSOCK implementation

use super::{VsockConnector, VsockStream as VsockStreamTrait, VsockListener as VsockListenerTrait};
use async_trait::async_trait;
use anyhow::{Result, anyhow};
use bytes::Bytes;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_vsock::{VsockStream as LinuxVsockStream, VsockListener as LinuxVsockListener, VsockAddr};

pub struct LinuxVsockConnector;

impl LinuxVsockConnector {
    pub fn new() -> Self { Self }
}

#[async_trait]
impl VsockConnector for LinuxVsockConnector {
    async fn connect(&self, cid: u32, port: u32) -> Result<Box<dyn VsockStreamTrait>> {
        let addr = VsockAddr::new(cid, port);
        let stream = LinuxVsockStream::connect(addr).await?;
        Ok(Box::new(LinuxVsockWrapper { inner: stream }))
    }

    async fn listen(&self, port: u32) -> Result<Box<dyn VsockListenerTrait>> {
        let addr = VsockAddr::new(libc::VMADDR_CID_ANY, port);
        let listener = LinuxVsockListener::bind(addr)?;
        Ok(Box::new(LinuxListenerWrapper { inner: listener }))
    }
}

struct LinuxVsockWrapper { inner: LinuxVsockStream }

#[async_trait]
impl VsockStreamTrait for LinuxVsockWrapper {
    async fn read(&mut self) -> Result<Bytes> {
        let mut buf = [0u8; 8192];
        let n = self.inner.read(&mut buf).await?;
        if n == 0 { return Err(anyhow!("Connection closed")); }
        Ok(Bytes::copy_from_slice(&buf[..n]))
    }
    async fn write(&mut self, data: &[u8]) -> Result<()> {
        self.inner.write_all(data).await?;
        self.inner.flush().await?;
        Ok(())
    }
    async fn close(&mut self) -> Result<()> {
        self.inner.shutdown(std::net::Shutdown::Both)?;
        Ok(())
    }
}

struct LinuxListenerWrapper { inner: LinuxVsockListener }

#[async_trait]
impl VsockListenerTrait for LinuxListenerWrapper {
    async fn accept(&mut self) -> Result<Box<dyn VsockStreamTrait>> {
        let (stream, _addr) = self.inner.accept().await?;
        Ok(Box::new(LinuxVsockWrapper { inner: stream }))
    }
}
