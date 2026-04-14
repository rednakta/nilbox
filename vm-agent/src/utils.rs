//! Utility functions

use crate::vsock::VsockStream;
use tokio::net::TcpStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, error};
use anyhow::Result;

/// Bidirectional copy between two TcpStreams (for localhost bypass).
pub async fn pump_tcp(a: TcpStream, b: TcpStream) -> Result<()> {
    let (mut a_read, mut a_write) = a.into_split();
    let (mut b_read, mut b_write) = b.into_split();

    let a_to_b = tokio::io::copy(&mut a_read, &mut b_write);
    let b_to_a = tokio::io::copy(&mut b_read, &mut a_write);

    tokio::select! {
        r = a_to_b => { r?; }
        r = b_to_a => { r?; }
    }
    Ok(())
}

pub async fn pump(mut tcp: TcpStream, mut vsock: Box<dyn VsockStream>) -> Result<()> {
    let mut buf = [0u8; 65536];

    loop {
        tokio::select! {
            read_result = tcp.read(&mut buf) => {
                match read_result {
                    Ok(0) => {
                        debug!("TCP connection closed by peer");
                        if let Err(e) = vsock.close().await {
                            debug!("Failed to send FIN to VSOCK: {}", e);
                        }
                        break;
                    }
                    Ok(n) => {
                        if let Err(e) = vsock.write(&buf[..n]).await {
                            error!("VSOCK write error: {}", e);
                            break;
                        }
                    }
                    Err(e) => {
                        error!("TCP read error: {}", e);
                        let _ = vsock.close().await;
                        break;
                    }
                }
            }
            read_result = vsock.read() => {
                match read_result {
                    Ok(data) => {
                        if let Err(e) = tcp.write_all(&data).await {
                            error!("TCP write error: {}", e);
                            break;
                        }
                    }
                    Err(e) => {
                        debug!("VSOCK stream ended: {}", e);
                        let _ = tcp.shutdown().await;
                        break;
                    }
                }
            }
        }
    }
    Ok(())
}
