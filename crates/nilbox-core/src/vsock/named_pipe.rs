//! Windows TCP socket connector for QEMU virtio-serial communication
//!
//! The host binds a TCP listener *before* QEMU starts, and QEMU connects
//! to it as a client (`-chardev socket,...` without `server`).
//! This guarantees the chardev is connected before the guest boots,
//! so the vm-agent never sees EBUSY on `/dev/vport0p1`.

use super::{VsockConnector, VsockListener, VsockStream};
use async_trait::async_trait;
use anyhow::{Result, anyhow};
use bytes::Bytes;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Connector that listens for QEMU's TCP chardev connection.
///
/// Host binds to `127.0.0.1:PORT` first, then QEMU starts with
/// `-chardev socket,host=127.0.0.1,port=PORT` (client mode) and connects.
pub struct WindowsSocketConnector {
    port: u16,
}

impl WindowsSocketConnector {
    pub fn new(port: u16) -> Self {
        Self { port }
    }
}

#[async_trait]
impl VsockConnector for WindowsSocketConnector {
    async fn listen(&self, _port: u32) -> Result<Box<dyn VsockListener>> {
        let addr = format!("127.0.0.1:{}", self.port);
        let listener = TcpListener::bind(&addr).await
            .map_err(|e| anyhow!("Failed to bind TCP listener on {}: {}", addr, e))?;
        tracing::debug!("TCP listener ready on {} — waiting for QEMU chardev client", addr);
        Ok(Box::new(WindowsSocketListener { listener }))
    }

    async fn connect(&self, _cid: u32, _port: u32) -> Result<Box<dyn VsockStream>> {
        Err(anyhow!("WindowsSocketConnector: outbound connect not supported"))
    }
}

struct WindowsSocketListener {
    listener: TcpListener,
}

#[async_trait]
impl VsockListener for WindowsSocketListener {
    async fn accept(&mut self) -> Result<Box<dyn VsockStream>> {
        let (stream, addr) = self.listener.accept().await
            .map_err(|e| anyhow!("TCP accept failed: {}", e))?;
        stream.set_nodelay(true)?;
        tracing::debug!("QEMU chardev connected from {}", addr);
        Ok(Box::new(WindowsSocketStream { inner: stream }))
    }
}

struct WindowsSocketStream {
    inner: TcpStream,
}

#[async_trait]
impl VsockStream for WindowsSocketStream {
    async fn read(&mut self) -> Result<Bytes> {
        let mut buf = [0u8; 8192];
        let n = self.inner.read(&mut buf).await?;
        if n == 0 {
            return Err(anyhow!("Connection closed"));
        }
        Ok(Bytes::copy_from_slice(&buf[..n]))
    }

    async fn write(&mut self, data: &[u8]) -> Result<()> {
        self.inner.write_all(data).await?;
        self.inner.flush().await?;
        Ok(())
    }

    async fn close(&mut self) -> Result<()> {
        self.inner.shutdown().await?;
        Ok(())
    }
}
