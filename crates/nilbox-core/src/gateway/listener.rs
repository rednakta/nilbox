//! TCP listener

use anyhow::Result;
use std::net::SocketAddr;
use tokio::net::TcpListener;

pub async fn create_listener(port: u16) -> Result<TcpListener> {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let listener = TcpListener::bind(addr).await?;
    tracing::debug!("Listening on port {}", port);
    Ok(listener)
}
