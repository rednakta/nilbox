//! SSH Gateway — external SSH client → VSOCK → VM sshd

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio::net::TcpListener;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use anyhow::{Result, anyhow};
use tracing::{debug, error};

use crate::vsock::VsockStream;
use crate::vsock::stream::StreamMultiplexer;

struct GatewayEntry {
    host_port: u16,
    handle: JoinHandle<()>,
}

pub struct SshGateway {
    entries: RwLock<HashMap<String, GatewayEntry>>,
}

impl SshGateway {
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
        }
    }

    /// Enable SSH gateway for a VM on a specific host port.
    pub async fn enable(
        &self,
        vm_id: &str,
        host_port: u16,
        multiplexer: Arc<StreamMultiplexer>,
    ) -> Result<()> {
        // Stop existing gateway for this VM
        self.disable(vm_id).await;

        let addr = format!("127.0.0.1:{}", host_port);
        let listener = TcpListener::bind(&addr).await
            .map_err(|e| anyhow!("Failed to bind SSH gateway on {}: {}", addr, e))?;

        debug!("SSH Gateway listening on {} for VM {}", addr, vm_id);

        let vm_id_owned = vm_id.to_string();
        let handle = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((tcp_stream, peer)) => {
                        debug!("SSH Gateway: connection from {} for VM {}", peer, vm_id_owned);
                        let mux = multiplexer.clone();
                        tokio::spawn(async move {
                            match mux.create_stream(22).await {
                                Ok(mut vsock_stream) => {
                                    let (mut tcp_read, mut tcp_write) = tcp_stream.into_split();
                                    let writer = vsock_stream.writer();

                                    let to_vm = tokio::spawn(async move {
                                        let mut buf = [0u8; 8192];
                                        loop {
                                            match tcp_read.read(&mut buf).await {
                                                Ok(0) => break,
                                                Ok(n) => {
                                                    if writer.write(&buf[..n]).await.is_err() {
                                                        break;
                                                    }
                                                }
                                                Err(_) => break,
                                            }
                                        }
                                        let _ = writer.close().await;
                                    });

                                    let from_vm = tokio::spawn(async move {
                                        loop {
                                            match vsock_stream.read().await {
                                                Ok(data) => {
                                                    if tcp_write.write_all(&data).await.is_err() {
                                                        break;
                                                    }
                                                }
                                                Err(_) => break,
                                            }
                                        }
                                        let _ = tcp_write.shutdown().await;
                                    });

                                    let _ = tokio::join!(to_vm, from_vm);
                                }
                                Err(e) => error!("SSH Gateway: VSOCK stream error: {}", e),
                            }
                        });
                    }
                    Err(e) => {
                        error!("SSH Gateway accept error: {}", e);
                        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    }
                }
            }
        });

        self.entries.write().await.insert(vm_id.to_string(), GatewayEntry {
            host_port,
            handle,
        });

        Ok(())
    }

    /// Disable SSH gateway for a VM.
    pub async fn disable(&self, vm_id: &str) {
        let mut entries = self.entries.write().await;
        if let Some(entry) = entries.remove(vm_id) {
            entry.handle.abort();
            debug!("SSH Gateway disabled for VM {}", vm_id);
        }
    }

    /// Get the host port for a VM's SSH gateway, if enabled.
    pub async fn status(&self, vm_id: &str) -> Option<u16> {
        let entries = self.entries.read().await;
        entries.get(vm_id).map(|e| e.host_port)
    }
}

impl Default for SshGateway {
    fn default() -> Self {
        Self::new()
    }
}
