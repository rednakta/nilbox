//! Apple Virtualization.framework VSOCK relay connector

use super::{VsockConnector, VsockListener, VsockStream};
use async_trait::async_trait;
use anyhow::{Result, anyhow};
use bytes::Bytes;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

pub struct AppleVirtConnector {
    relay_path: PathBuf,
    auth_token: [u8; 32],
}

impl AppleVirtConnector {
    pub fn new(relay_path: impl Into<PathBuf>, auth_token: [u8; 32]) -> Self {
        Self { relay_path: relay_path.into(), auth_token }
    }
}

#[async_trait]
impl VsockConnector for AppleVirtConnector {
    async fn listen(&self, _port: u32) -> Result<Box<dyn VsockListener>> {
        if self.relay_path.exists() {
            std::fs::remove_file(&self.relay_path)?;
        }
        tracing::debug!("Binding relay socket: {}", self.relay_path.display());
        let listener = UnixListener::bind(&self.relay_path)?;

        // Restrict socket file permissions to owner-only (0o600)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&self.relay_path, perms)?;
        }

        Ok(Box::new(AppleVirtListener {
            inner: listener,
            expected_token: self.auth_token,
        }))
    }

    async fn connect(&self, _cid: u32, _port: u32) -> Result<Box<dyn VsockStream>> {
        Err(anyhow!("AppleVirtConnector: outbound connect not supported"))
    }
}

struct AppleVirtListener {
    inner: UnixListener,
    expected_token: [u8; 32],
}

#[async_trait]
impl VsockListener for AppleVirtListener {
    async fn accept(&mut self) -> Result<Box<dyn VsockStream>> {
        let (mut stream, _addr) = self.inner.accept().await?;

        // Tune relay socket buffers to match the Swift VMM side.
        // Without this, macOS default ~8-16KB buffers cause write stalls
        // when the multiplexer pushes 32KB frames through the relay.
        {
            use std::os::unix::io::AsRawFd;
            let fd = stream.as_raw_fd();
            let snd: libc::c_int = 1_048_576; // 1MB
            let rcv: libc::c_int = 4_194_304; // 4MB
            unsafe {
                libc::setsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_SNDBUF,
                    &snd as *const _ as *const libc::c_void,
                    std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                );
                libc::setsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_RCVBUF,
                    &rcv as *const _ as *const libc::c_void,
                    std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                );
            }
        }

        // Verify 32-byte auth token before reading port header
        let mut token_buf = [0u8; 32];
        stream.read_exact(&mut token_buf).await?;
        if !constant_time_eq(&token_buf, &self.expected_token) {
            tracing::warn!("Relay auth token mismatch — rejecting connection");
            stream.shutdown().await.ok();
            return Err(anyhow!("Relay auth token mismatch"));
        }

        let mut port_bytes = [0u8; 4];
        stream.read_exact(&mut port_bytes).await?;
        let port = u32::from_be_bytes(port_bytes);
        tracing::debug!("Relay accepted authenticated connection for vsock port {}", port);
        Ok(Box::new(AppleVirtStream { inner: stream }))
    }
}

/// Constant-time comparison to avoid timing side-channels on token verification.
fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff: u8 = 0;
    for i in 0..32 {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

struct AppleVirtStream {
    inner: UnixStream,
}

#[async_trait]
impl VsockStream for AppleVirtStream {
    async fn read(&mut self) -> Result<Bytes> {
        let mut buf = [0u8; 65536];
        let n = self.inner.read(&mut buf).await?;
        if n == 0 {
            return Err(anyhow!("Connection closed"));
        }
        Ok(Bytes::copy_from_slice(&buf[..n]))
    }

    async fn write(&mut self, data: &[u8]) -> Result<()> {
        self.inner.write_all(data).await?;
        Ok(())
    }

    async fn close(&mut self) -> Result<()> {
        self.inner.shutdown().await?;
        Ok(())
    }
}
