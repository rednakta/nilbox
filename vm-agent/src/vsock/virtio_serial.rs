//! VirtIO Serial connector (for QEMU fallback — Windows host)
//!
//! Virtio-serial is a single character device that only allows one
//! open fd at a time (EBUSY otherwise).  This module encapsulates
//! that exclusivity: after accept() returns a stream, subsequent
//! accept() calls block until the stream is dropped (fd released).
//!
//! Uses separate file descriptors for read and write to avoid
//! tokio::fs::File internal lseek() on non-seekable character devices.

use super::{VsockConnector, VsockStream as VsockStreamTrait, VsockListener as VsockListenerTrait};
use async_trait::async_trait;
use anyhow::{Result, anyhow};
use bytes::Bytes;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Notify;
use tracing::{debug, warn};

pub struct VirtioSerialConnector {
    device_path: PathBuf,
}

impl VirtioSerialConnector {
    pub fn new() -> Self {
        Self { device_path: PathBuf::from("/dev/vport0p1") }
    }

    pub fn with_path(path: PathBuf) -> Self {
        Self { device_path: path }
    }
}

#[async_trait]
impl VsockConnector for VirtioSerialConnector {
    async fn connect(&self, _cid: u32, _port: u32) -> Result<Box<dyn VsockStreamTrait>> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.device_path)?;
        Ok(Box::new(VirtioSerialStream::new(file, None)?))
    }

    async fn listen(&self, _port: u32) -> Result<Box<dyn VsockListenerTrait>> {
        Ok(Box::new(VirtioSerialListener {
            device_path: self.device_path.clone(),
            release: Arc::new(Notify::new()),
            has_connection: false,
        }))
    }
}

/// Guard that notifies the listener when dropped (fd released).
struct ReleaseGuard(Arc<Notify>);

impl Drop for ReleaseGuard {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

/// Stream backed by a virtio-serial character device.
///
/// Uses two separate file descriptors (via `dup()`) for read and write.
/// This avoids `tokio::fs::File`'s internal `lseek()` when interleaving
/// reads and writes, which fails with ESPIPE on non-seekable devices.
///
/// Field order matters: `reader` and `writer` are dropped (fd closed)
/// before `_release`, which then notifies the listener.
struct VirtioSerialStream {
    reader: tokio::fs::File,
    writer: tokio::fs::File,
    _release: Option<ReleaseGuard>,
}

impl VirtioSerialStream {
    fn new(file: std::fs::File, release: Option<Arc<Notify>>) -> Result<Self> {
        let write_file = file.try_clone()
            .map_err(|e| anyhow!("Failed to dup virtio-serial fd: {}", e))?;
        Ok(Self {
            reader: tokio::fs::File::from_std(file),
            writer: tokio::fs::File::from_std(write_file),
            _release: release.map(ReleaseGuard),
        })
    }
}

#[async_trait]
impl VsockStreamTrait for VirtioSerialStream {
    async fn read(&mut self) -> Result<Bytes> {
        let mut buf = [0u8; 8192];
        let n = self.reader.read(&mut buf).await?;
        if n == 0 { return Err(anyhow!("Connection closed")); }
        Ok(Bytes::copy_from_slice(&buf[..n]))
    }
    async fn write(&mut self, data: &[u8]) -> Result<()> {
        self.writer.write_all(data).await?;
        self.writer.flush().await?;
        Ok(())
    }
    async fn close(&mut self) -> Result<()> {
        self.writer.sync_all().await?;
        Ok(())
    }
}

struct VirtioSerialListener {
    device_path: PathBuf,
    release: Arc<Notify>,
    has_connection: bool,
}

#[async_trait]
impl VsockListenerTrait for VirtioSerialListener {
    async fn accept(&mut self) -> Result<Box<dyn VsockStreamTrait>> {
        // If a previous stream is still open, wait for it to be dropped.
        // Virtio-serial allows only one open fd — attempting to open again
        // returns EBUSY.  Block here instead of busy-looping.
        if self.has_connection {
            debug!("Waiting for previous virtio-serial connection to close...");
            self.release.notified().await;
            self.has_connection = false;
            debug!("Previous connection released, re-opening device");
        }

        loop {
            match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&self.device_path)
            {
                Ok(file) => {
                    debug!("virtio-serial device opened: {}", self.device_path.display());
                    self.has_connection = true;
                    return Ok(Box::new(
                        VirtioSerialStream::new(file, Some(self.release.clone()))?
                    ));
                }
                Err(e) => {
                    let raw = e.raw_os_error();
                    if raw == Some(libc::EBUSY) || raw == Some(libc::EAGAIN) {
                        warn!("virtio-serial busy, retrying in 2s...");
                        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                    } else {
                        return Err(e.into());
                    }
                }
            }
        }
    }
}
