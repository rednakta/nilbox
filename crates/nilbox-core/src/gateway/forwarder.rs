//! Bidirectional forwarder

use crate::vsock::VsockStream;
use tokio::net::TcpStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{error, debug};
use anyhow::Result;

pub async fn forward_connection(
    mut tcp_stream: TcpStream,
    mut vsock_stream: Box<dyn VsockStream>,
) -> Result<()> {
    let mut buf = [0u8; 65536];

    loop {
        tokio::select! {
            read_result = tcp_stream.read(&mut buf) => {
                match read_result {
                    Ok(0) => {
                        debug!("TCP connection closed by client");
                        if let Err(e) = vsock_stream.close().await {
                            debug!("Failed to send FIN to VSOCK: {}", e);
                        }
                        break;
                    }
                    Ok(n) => {
                        if let Err(e) = vsock_stream.write(&buf[..n]).await {
                            error!("Failed to write to VSOCK: {}", e);
                            break;
                        }
                    }
                    Err(e) => {
                        error!("TCP read error: {}", e);
                        let _ = vsock_stream.close().await;
                        break;
                    }
                }
            }
            read_result = vsock_stream.read() => {
                match read_result {
                    Ok(data) => {
                        if let Err(e) = tcp_stream.write_all(&data).await {
                            error!("Failed to write to TCP: {}", e);
                            break;
                        }
                    }
                    Err(e) => {
                        debug!("VSOCK stream ended: {}", e);
                        let _ = tcp_stream.shutdown().await;
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}
