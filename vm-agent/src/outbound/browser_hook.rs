//! Handler for __nilbox__ control requests (e.g., open-url for browser delegation)
//!
//! VM 내부의 xdg-open 후킹 스크립트가 호출하는 `/__nilbox__/open-url?url=...` 요청을
//! VSOCK ReverseRequest로 호스트에 전달하여 호스트 브라우저에서 URL을 연다.

use crate::outbound::proxy::SharedMultiplexer;
use crate::utils::pump;
use crate::vsock::VsockStream;
use anyhow::{Result, anyhow};
use bytes::{Bytes, BytesMut};
use tokio::net::TcpStream;
use tracing::debug;

/// Check if an HTTP request path starts with `/__nilbox__/`.
pub fn is_nilbox_control_request(headers: &[u8]) -> bool {
    let header_str = match std::str::from_utf8(headers) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let request_line = match header_str.lines().next() {
        Some(l) => l,
        None => return false,
    };
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        return false;
    }
    let uri = parts[1];
    // Direct request: GET /__nilbox__/...
    // Proxy request: GET http://127.0.0.1:18088/__nilbox__/...
    uri.starts_with("/__nilbox__/") || uri.contains("/__nilbox__/")
}

/// Forward a `/__nilbox__/` control request to the host via VSOCK ReverseRequest.
pub async fn handle_nilbox_request(
    socket: TcpStream,
    headers: Bytes,
    body: BytesMut,
    mux_store: SharedMultiplexer,
) -> Result<()> {
    debug!("Forwarding __nilbox__ control request to host via VSOCK");

    let mux = {
        let lock = mux_store.read().await;
        lock.clone()
    };
    let mux = mux.ok_or_else(|| anyhow!("No active VSOCK connection"))?;

    let mut vsock_stream = mux.create_reverse_stream(headers).await?;
    if !body.is_empty() {
        vsock_stream.write(&body).await?;
    }

    pump(socket, Box::new(vsock_stream)).await
}
