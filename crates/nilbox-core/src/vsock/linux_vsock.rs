//! Linux AF_VSOCK connector for QEMU vhost-vsock-pci communication
//!
//! QEMU runs with `-device vhost-vsock-pci,guest-cid=3`.
//! The guest vm-agent listens on VSOCK port 1024.
//! This connector actively connects from host to guest via AF_VSOCK.

use super::{VsockConnector, VsockListener, VsockStream, GUEST_CID};
use async_trait::async_trait;
use anyhow::{Result, anyhow};
use bytes::Bytes;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_vsock::{VsockStream as TokioVsockStream, VsockAddr};

const CONNECT_TIMEOUT_SECS: u64 = 60;
const CONNECT_RETRY_INTERVAL_MS: u64 = 500;
const VM_AGENT_PORT: u32 = 1024;

/// Host-side AF_VSOCK connector that connects to the guest's vm-agent.
pub struct LinuxVhostConnector;

impl LinuxVhostConnector {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl VsockConnector for LinuxVhostConnector {
    async fn listen(&self, _port: u32) -> Result<Box<dyn VsockListener>> {
        tracing::debug!(
            "LinuxVhostConnector: will connect to guest CID {} port {}",
            GUEST_CID, VM_AGENT_PORT
        );
        Ok(Box::new(LinuxVhostListener))
    }

    async fn connect(&self, cid: u32, port: u32) -> Result<Box<dyn VsockStream>> {
        let addr = VsockAddr::new(cid, port);
        let stream = TokioVsockStream::connect(addr).await?;
        Ok(Box::new(LinuxVhostStream { inner: stream }))
    }
}

/// Pseudo-listener that retries AF_VSOCK connect until the guest vm-agent is ready.
struct LinuxVhostListener;

#[async_trait]
impl VsockListener for LinuxVhostListener {
    async fn accept(&mut self) -> Result<Box<dyn VsockStream>> {
        let addr = VsockAddr::new(GUEST_CID, VM_AGENT_PORT);
        let deadline = tokio::time::Instant::now()
            + tokio::time::Duration::from_secs(CONNECT_TIMEOUT_SECS);

        tracing::debug!(
            "Connecting to guest via AF_VSOCK (CID={}, port={}, timeout={}s)...",
            GUEST_CID, VM_AGENT_PORT, CONNECT_TIMEOUT_SECS
        );

        loop {
            match TokioVsockStream::connect(addr).await {
                Ok(stream) => {
                    tracing::debug!(
                        "Connected to guest via AF_VSOCK (CID={}, port={})",
                        GUEST_CID, VM_AGENT_PORT
                    );
                    return Ok(Box::new(LinuxVhostStream { inner: stream }));
                }
                Err(e) => {
                    if tokio::time::Instant::now() >= deadline {
                        tracing::error!(
                            "Timed out after {}s connecting to guest via AF_VSOCK: {}",
                            CONNECT_TIMEOUT_SECS, e
                        );
                        return Err(anyhow!(
                            "Timed out after {}s connecting to guest via AF_VSOCK (CID={}, port={}): {}",
                            CONNECT_TIMEOUT_SECS, GUEST_CID, VM_AGENT_PORT, e
                        ));
                    }
                    tracing::debug!(
                        "Guest AF_VSOCK not ready, retrying in {}ms: {}",
                        CONNECT_RETRY_INTERVAL_MS, e
                    );
                    tokio::time::sleep(tokio::time::Duration::from_millis(
                        CONNECT_RETRY_INTERVAL_MS,
                    ))
                    .await;
                }
            }
        }
    }
}

struct LinuxVhostStream {
    inner: TokioVsockStream,
}

#[async_trait]
impl VsockStream for LinuxVhostStream {
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
        self.inner.shutdown(std::net::Shutdown::Both)?;
        Ok(())
    }
}
