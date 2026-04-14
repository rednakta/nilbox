//! Async adapter: bridges VirtualStream to tokio AsyncRead+AsyncWrite
//! via a tokio::io::duplex() pair.

use super::stream::VirtualStream;
use super::VsockStream;
use tokio::io::{self, AsyncReadExt, AsyncWriteExt, DuplexStream};
use tracing::{error, trace};

/// Wrap a VirtualStream into a tokio DuplexStream suitable for russh's
/// `connect_stream()` (which requires `AsyncRead + AsyncWrite + Unpin`).
pub fn wrap_virtual_stream(vs: VirtualStream) -> DuplexStream {
    let (client_half, bridge_half) = io::duplex(65536);
    let (bridge_read, bridge_write) = io::split(bridge_half);

    let writer = vs.writer();
    let stream_id = vs.stream_id;

    // VirtualStream.read() -> bridge write (VM output -> SSH client)
    tokio::spawn(async move {
        let mut bridge_write = bridge_write;
        let mut vs = vs;
        let mut total_bytes: u64 = 0;
        trace!("[ADAPTER-DBG] stream={} read loop started (VS→bridge)", stream_id);
        loop {
            match vs.read().await {
                Ok(data) => {
                    total_bytes += data.len() as u64;
                    trace!("[ADAPTER-DBG] stream={} VS→bridge {} bytes (total={})",
                        stream_id, data.len(), total_bytes);
                    if let Err(e) = bridge_write.write_all(&data).await {
                        error!("[ADAPTER-DBG] stream={} bridge write error: {} (total={})",
                            stream_id, e, total_bytes);
                        break;
                    }
                }
                Err(e) => {
                    trace!("[ADAPTER-DBG] stream={} VirtualStream read ended: {} (total_read={})",
                        stream_id, e, total_bytes);
                    break;
                }
            }
        }
        let _ = bridge_write.shutdown().await;
        trace!("[ADAPTER-DBG] stream={} read loop ended (VS→bridge, total={})", stream_id, total_bytes);
    });

    // bridge read -> VirtualStream.write() (SSH client -> VM)
    let ws_id = stream_id;
    tokio::spawn(async move {
        let mut bridge_read = bridge_read;
        let mut buf = [0u8; 8192];
        let mut total_bytes: u64 = 0;
        trace!("[ADAPTER-DBG] stream={} write loop started (bridge→VS)", ws_id);
        loop {
            match bridge_read.read(&mut buf).await {
                Ok(0) => {
                    trace!("[ADAPTER-DBG] stream={} bridge read EOF (total_written={})", ws_id, total_bytes);
                    break;
                }
                Ok(n) => {
                    total_bytes += n as u64;
                    trace!("[ADAPTER-DBG] stream={} bridge→VS {} bytes (total={})", ws_id, n, total_bytes);
                    if let Err(e) = writer.write(&buf[..n]).await {
                        error!("[ADAPTER-DBG] stream={} VirtualStream write error: {} (total_written={})",
                            ws_id, e, total_bytes);
                        break;
                    }
                }
                Err(e) => {
                    error!("[ADAPTER-DBG] stream={} bridge read error: {} (total_written={})",
                        ws_id, e, total_bytes);
                    break;
                }
            }
        }
        let _ = writer.close().await;
        trace!("[ADAPTER-DBG] stream={} write loop ended (bridge→VS, total={})", ws_id, total_bytes);
    });

    client_half
}
