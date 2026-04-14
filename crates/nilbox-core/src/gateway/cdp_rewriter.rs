//! CDP Response Rewriter — rewrites Chrome DevTools Protocol discovery responses
//! to replace 127.0.0.1/localhost with cdp.nilbox so VM clients can connect
//! through the VSOCK tunnel without client-side URL fixups.

use crate::vsock::VsockStream;
use tokio::net::TcpStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{warn, debug, error};
use anyhow::Result;

const MAX_BODY_SIZE: usize = 1_048_576; // 1 MB

/// CDP-aware forwarder: intercepts HTTP request/response cycles for CDP discovery
/// endpoints (/json/*), rewrites hostnames in JSON responses, then switches to
/// raw bidirectional tunnel on WebSocket upgrade or non-discovery paths.
///
/// `replacement_host`: hostname to substitute for 127.0.0.1/localhost in responses.
/// Should match the hostname the VM client used to connect
/// (cdp.nilbox / headless.cdp.nilbox / headed.cdp.nilbox).
pub async fn forward_cdp_connection(
    mut tcp: TcpStream,
    mut vsock: Box<dyn VsockStream>,
    replacement_host: &str,
) -> Result<()> {
    let mut leftover_tcp: Vec<u8> = Vec::new();
    let mut leftover_vsock: Vec<u8> = Vec::new();

    loop {
        // === Phase 1: Read HTTP request from VM ===
        let request_bytes = match read_vsock_data(&mut vsock, &mut leftover_vsock).await {
            Ok(data) if data.is_empty() => break,
            Ok(data) => data,
            Err(_) => break,
        };

        // Check if this looks like an HTTP request
        let is_http = request_bytes.starts_with(b"GET ")
            || request_bytes.starts_with(b"POST ")
            || request_bytes.starts_with(b"PUT ")
            || request_bytes.starts_with(b"HEAD ");

        if !is_http {
            // Not HTTP — switch to raw tunnel immediately
            tcp.write_all(&request_bytes).await?;
            return raw_tunnel(tcp, vsock, leftover_tcp, leftover_vsock).await;
        }

        let is_cdp_discovery = is_cdp_discovery_path(&request_bytes);

        // Rewrite Host header: cdp.nilbox → 127.0.0.1 (Chrome rejects non-localhost Host)
        let request_bytes = rewrite_request_host(&request_bytes);

        // Forward request to Chrome
        tcp.write_all(&request_bytes).await?;

        if !is_cdp_discovery {
            // Non-discovery path (likely WebSocket upgrade) — go raw
            return raw_tunnel(tcp, vsock, leftover_tcp, leftover_vsock).await;
        }

        // === Phase 2: Read HTTP response from Chrome ===
        let (header_bytes, body, extra) = match read_http_response(&mut tcp, &mut leftover_tcp).await {
            Ok(v) => v,
            Err(e) => {
                warn!("CDP rewriter: failed to read HTTP response: {}", e);
                return raw_tunnel(tcp, vsock, leftover_tcp, leftover_vsock).await;
            }
        };
        leftover_tcp = extra;

        let status = parse_status_code(&header_bytes);

        if status == Some(101) {
            // WebSocket upgrade — forward as-is, then raw tunnel
            vsock.write(&header_bytes).await?;
            if !body.is_empty() {
                vsock.write(&body).await?;
            }
            return raw_tunnel(tcp, vsock, leftover_tcp, leftover_vsock).await;
        }

        // Try to rewrite JSON body
        if status == Some(200) && body.len() <= MAX_BODY_SIZE {
            if let Some(rewritten) = rewrite_cdp_json(&body, replacement_host) {
                debug!("CDP rewriter: rewrote {} -> {} bytes", body.len(), rewritten.len());
                let new_headers = update_content_length(&header_bytes, rewritten.len());
                vsock.write(&new_headers).await?;
                vsock.write(&rewritten).await?;
                continue; // loop back for next request (keep-alive)
            }
        }

        // Passthrough — no rewriting needed or possible
        vsock.write(&header_bytes).await?;
        if !body.is_empty() {
            vsock.write(&body).await?;
        }
    }

    Ok(())
}

/// Rewrite Host header from any cdp.nilbox variant to 127.0.0.1 so Chrome accepts the request.
/// Handles: cdp.nilbox, headless.cdp.nilbox, headed.cdp.nilbox
fn rewrite_request_host(request: &[u8]) -> Vec<u8> {
    let s = match std::str::from_utf8(request) {
        Ok(s) => s,
        Err(_) => return request.to_vec(),
    };
    // Replace longer variants first to avoid partial match on "cdp.nilbox" substring
    let result = s
        .replace("Host: headless.cdp.nilbox", "Host: 127.0.0.1")
        .replace("host: headless.cdp.nilbox", "host: 127.0.0.1")
        .replace("Host: headed.cdp.nilbox", "Host: 127.0.0.1")
        .replace("host: headed.cdp.nilbox", "host: 127.0.0.1")
        .replace("Host: cdp.nilbox", "Host: 127.0.0.1")
        .replace("host: cdp.nilbox", "host: 127.0.0.1");
    result.into_bytes()
}

/// Check if the HTTP request path matches a CDP discovery endpoint
fn is_cdp_discovery_path(request: &[u8]) -> bool {
    let request_str = match std::str::from_utf8(request) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let first_line = match request_str.lines().next() {
        Some(l) => l,
        None => return false,
    };
    // "GET /json/version HTTP/1.1"
    let parts: Vec<&str> = first_line.split_whitespace().collect();
    if parts.len() < 2 {
        return false;
    }
    let path = parts[1];
    path == "/json"
        || path == "/json/"
        || path.starts_with("/json/version")
        || path.starts_with("/json/list")
        || path.starts_with("/json/new")
        || path.starts_with("/json/protocol")
}

/// Parse HTTP status code from response header
fn parse_status_code(headers: &[u8]) -> Option<u16> {
    let s = std::str::from_utf8(headers).ok()?;
    let first_line = s.lines().next()?;
    // "HTTP/1.1 200 OK"
    let parts: Vec<&str> = first_line.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }
    parts[1].parse().ok()
}

/// Rewrite CDP JSON discovery response: replace 127.0.0.1/localhost with `replacement_host`
fn rewrite_cdp_json(body: &[u8], replacement_host: &str) -> Option<Vec<u8>> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    let rewritten = rewrite_json_value(value, replacement_host);
    serde_json::to_vec(&rewritten).ok()
}

fn rewrite_json_value(value: serde_json::Value, replacement_host: &str) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let new_map: serde_json::Map<String, serde_json::Value> = map
                .into_iter()
                .map(|(k, v)| {
                    let new_v = if k == "webSocketDebuggerUrl" || k == "devtoolsFrontendUrl" {
                        if let serde_json::Value::String(s) = &v {
                            serde_json::Value::String(
                                s.replace("127.0.0.1", replacement_host)
                                    .replace("localhost", replacement_host),
                            )
                        } else {
                            rewrite_json_value(v, replacement_host)
                        }
                    } else {
                        rewrite_json_value(v, replacement_host)
                    };
                    (k, new_v)
                })
                .collect();
            serde_json::Value::Object(new_map)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.into_iter().map(|v| rewrite_json_value(v, replacement_host)).collect())
        }
        other => other,
    }
}

/// Update Content-Length header in HTTP response
fn update_content_length(headers: &[u8], new_length: usize) -> Vec<u8> {
    let s = match std::str::from_utf8(headers) {
        Ok(s) => s,
        Err(_) => return headers.to_vec(),
    };

    let mut result = String::with_capacity(s.len());
    let mut found = false;
    for line in s.split("\r\n") {
        if !result.is_empty() {
            result.push_str("\r\n");
        }
        if line.to_lowercase().starts_with("content-length:") {
            result.push_str(&format!("Content-Length: {}", new_length));
            found = true;
        } else {
            result.push_str(line);
        }
    }
    if !found {
        // Insert before final \r\n\r\n
        if let Some(pos) = result.rfind("\r\n\r\n") {
            result.insert_str(pos, &format!("\r\nContent-Length: {}", new_length));
        }
    }
    result.into_bytes()
}

/// Read data from VsockStream, using leftover buffer first
async fn read_vsock_data(vsock: &mut Box<dyn VsockStream>, leftover: &mut Vec<u8>) -> Result<Vec<u8>> {
    if !leftover.is_empty() {
        return Ok(std::mem::take(leftover));
    }
    let data = vsock.read().await?;
    Ok(data.to_vec())
}

/// Read a complete HTTP response (headers + body) from TCP stream.
/// Returns (headers_with_crlf, body, leftover_bytes).
async fn read_http_response(
    tcp: &mut TcpStream,
    leftover: &mut Vec<u8>,
) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    let mut buf = std::mem::take(leftover);
    let mut read_buf = [0u8; 65536];

    // Read until we find \r\n\r\n (end of headers)
    let header_end = loop {
        if let Some(pos) = find_header_end(&buf) {
            break pos;
        }
        let n = tcp.read(&mut read_buf).await?;
        if n == 0 {
            return Err(anyhow::anyhow!("Connection closed before headers complete"));
        }
        buf.extend_from_slice(&read_buf[..n]);
    };

    let headers_len = header_end + 4; // include \r\n\r\n
    let headers = buf[..headers_len].to_vec();
    let after_headers = buf[headers_len..].to_vec();

    // Parse Content-Length
    let content_length = parse_content_length(&headers);

    let body = if let Some(cl) = content_length {
        // Read exact body
        let mut body = after_headers;
        while body.len() < cl {
            let n = tcp.read(&mut read_buf).await?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&read_buf[..n]);
        }
        let extra = if body.len() > cl {
            body[cl..].to_vec()
        } else {
            Vec::new()
        };
        let body_final = body[..cl.min(body.len())].to_vec();
        return Ok((headers, body_final, extra));
    } else {
        // No Content-Length — return what we have (likely chunked or connection-close)
        after_headers
    };

    Ok((headers, body, Vec::new()))
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn parse_content_length(headers: &[u8]) -> Option<usize> {
    let s = std::str::from_utf8(headers).ok()?;
    for line in s.split("\r\n") {
        if let Some(rest) = line.strip_prefix("Content-Length:").or_else(|| line.strip_prefix("content-length:")) {
            return rest.trim().parse().ok();
        }
    }
    None
}

/// Bidirectional raw tunnel (no inspection)
async fn raw_tunnel(
    mut tcp: TcpStream,
    mut vsock: Box<dyn VsockStream>,
    leftover_tcp: Vec<u8>,
    leftover_vsock: Vec<u8>,
) -> Result<()> {
    // Flush leftover data
    if !leftover_tcp.is_empty() {
        vsock.write(&leftover_tcp).await?;
    }
    if !leftover_vsock.is_empty() {
        tcp.write_all(&leftover_vsock).await?;
    }

    let mut buf = [0u8; 65536];
    loop {
        tokio::select! {
            read_result = tcp.read(&mut buf) => {
                match read_result {
                    Ok(0) => {
                        debug!("CDP tunnel: TCP closed");
                        let _ = vsock.close().await;
                        break;
                    }
                    Ok(n) => {
                        if let Err(e) = vsock.write(&buf[..n]).await {
                            error!("CDP tunnel: VSOCK write error: {}", e);
                            break;
                        }
                    }
                    Err(e) => {
                        error!("CDP tunnel: TCP read error: {}", e);
                        let _ = vsock.close().await;
                        break;
                    }
                }
            }
            read_result = vsock.read() => {
                match read_result {
                    Ok(data) => {
                        if let Err(e) = tcp.write_all(&data).await {
                            error!("CDP tunnel: TCP write error: {}", e);
                            break;
                        }
                    }
                    Err(e) => {
                        debug!("CDP tunnel: VSOCK ended: {}", e);
                        let _ = tcp.shutdown().await;
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}
