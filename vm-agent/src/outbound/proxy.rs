//! Outbound proxy agent (VM -> Host HTTP forwarding)

use crate::vsock::stream::StreamMultiplexer;
use crate::utils::{pump, pump_tcp};
use crate::vsock::VsockStream;
use anyhow::{Result, anyhow};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::RwLock;
use tracing::{debug, warn, error};
use bytes::BytesMut;

#[cfg(target_os = "linux")]
const NILBOX_HOST_SENTINEL: std::net::Ipv4Addr = std::net::Ipv4Addr::new(169, 254, 254, 1); // Auto
#[cfg(target_os = "linux")]
const NILBOX_HEADLESS_SENTINEL: std::net::Ipv4Addr = std::net::Ipv4Addr::new(169, 254, 254, 2); // Headless
#[cfg(target_os = "linux")]
const NILBOX_HEADED_SENTINEL: std::net::Ipv4Addr = std::net::Ipv4Addr::new(169, 254, 254, 3); // Headed

pub type SharedMultiplexer = Arc<RwLock<Option<Arc<StreamMultiplexer>>>>;

pub struct OutboundProxy {
    mux_store: SharedMultiplexer,
}

impl OutboundProxy {
    pub fn new(mux_store: SharedMultiplexer) -> Self {
        Self { mux_store }
    }

    pub async fn run(&self) -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:18088").await?;
        debug!("Outbound Proxy listening on 127.0.0.1:18088");

        loop {
            let (socket, addr) = listener.accept().await?;
            let mux_store = self.mux_store.clone();

            tokio::spawn(async move {
                if let Err(e) = handle_connection(socket, mux_store).await {
                    error!("Outbound connection error: {}", e);
                }
            });
        }
    }
}

async fn handle_connection(socket: TcpStream, mux_store: SharedMultiplexer) -> Result<()> {
    let addr = socket.peer_addr().ok();
    // Get original destination before borrowing socket for reads.
    // For iptables REDIRECT connections this returns the real target;
    // for explicit proxy connections it returns 127.0.0.1:18088 or fails.
    #[cfg(target_os = "linux")]
    let original_dst = match get_original_dst(&socket) {
        Ok(dst) => {
            debug!("Connection from {:?}, original_dst={}", addr, dst);
            Some(dst)
        }
        Err(e) => {
            warn!("SO_ORIGINAL_DST failed for {:?}: {}, falling back to HTTP parse", addr, e);
            None
        }
    };

    // Note: original_dst=127.0.0.1:18088 is normal for explicit proxy
    // connections (HTTP_PROXY). Self-referencing is only blocked in the
    // localhost bypass path below where the HTTP target matches our port.

    // Sentinel IP detection: cdp.nilbox variants → raw TCP tunnel to host
    // 169.254.254.1 = Auto, 169.254.254.2 = Headless, 169.254.254.3 = Headed
    #[cfg(target_os = "linux")]
    if let Some(ref dst) = original_dst {
        let mode: Option<u8> = match dst.ip() {
            ip if ip == std::net::IpAddr::V4(NILBOX_HOST_SENTINEL)    => Some(0x00),
            ip if ip == std::net::IpAddr::V4(NILBOX_HEADLESS_SENTINEL) => Some(0x01),
            ip if ip == std::net::IpAddr::V4(NILBOX_HEADED_SENTINEL)   => Some(0x02),
            _ => None,
        };
        if let Some(m) = mode {
            let port = dst.port();
            debug!("Sentinel IP detected: {}:{} → HostConnect tunnel (mode=0x{:02X})", dst.ip(), port, m);
            return handle_host_connect(socket, mux_store, port, m).await;
        }
    }

    let mut socket = socket;
    let mut buffer = BytesMut::with_capacity(8192);

    // First read to determine connection type
    let n = socket.read_buf(&mut buffer).await?;
    if n == 0 {
        return Ok(());
    }

    // Detect TLS ClientHello (transparent HTTPS via iptables REDIRECT)
    #[cfg(target_os = "linux")]
    if buffer[0] == 0x16 {
        debug!("TLS ClientHello detected from {:?}, original_dst={:?}", addr, original_dst);
        if let Some(dst) = original_dst {
            return handle_transparent_tls(socket, buffer, mux_store, dst).await;
        }
        return Err(anyhow!("TLS connection but SO_ORIGINAL_DST unavailable"));
    }

    debug!("HTTP proxy request from {:?}", addr);

    // Existing HTTP proxy flow: read until headers complete
    loop {
        if let Some(pos) = find_subsequence(&buffer, b"\r\n\r\n") {
            let header_len = pos + 4;

            let headers = &buffer[..header_len];

            // Check for __nilbox__ control requests — always forward to host via VSOCK
            if super::browser_hook::is_nilbox_control_request(headers) {
                let body = buffer.split_off(header_len);
                let headers_payload = buffer.freeze();
                return super::browser_hook::handle_nilbox_request(socket, headers_payload, body, mux_store).await;
            }

            // Check if target is localhost — bypass VSOCK and connect directly
            // Also handle nilbox-cdp sentinel hosts via HostConnect tunnel
            if let Some(target) = extract_target(headers) {
                if is_localhost(&target.host) {
                    // Reject self-referencing loop: destination is our own listen port
                    if target.port == 18088 {
                        warn!("Rejecting self-referencing proxy loop to {}:{}", target.host, target.port);
                        return Err(anyhow!("Self-referencing proxy loop detected"));
                    }
                    debug!("Localhost bypass: {}:{} (connect={})", target.host, target.port, target.is_connect);
                    let body = buffer.split_off(header_len);
                    let headers_buf = buffer;
                    return handle_localhost_direct(socket, target, headers_buf, body).await;
                }

                // cdp.nilbox via http_proxy: route as HostConnect tunnel instead of VSOCK reverse proxy
                #[cfg(target_os = "linux")]
                if let Some(mode) = nilbox_cdp_mode(&target.host) {
                    let port = target.port;
                    let is_connect = target.is_connect;
                    debug!("cdp.nilbox HTTP proxy: {}:{} mode=0x{:02X} connect={}", target.host, port, mode, is_connect);
                    let body = buffer.split_off(header_len);
                    let headers_buf = buffer;
                    return handle_nilbox_cdp_proxy(socket, headers_buf, body, mux_store, port, mode, is_connect).await;
                }
            }

            let initial_payload = buffer.split_to(header_len).freeze();

            let mux = {
                let lock = mux_store.read().await;
                lock.clone()
            };

            if let Some(mux) = mux {
                let mut vsock_stream = mux.create_reverse_stream(initial_payload).await?;
                if !buffer.is_empty() {
                    vsock_stream.write(&buffer).await?;
                }
                return pump(socket, Box::new(vsock_stream)).await;
            } else {
                return Err(anyhow!("No active VSOCK connection"));
            }
        }

        let n = socket.read_buf(&mut buffer).await?;
        if n == 0 {
            return Err(anyhow!("Connection closed before headers complete"));
        }
        if buffer.len() > 16384 {
            return Err(anyhow!("Headers too large"));
        }
    }
}

/// Handle a transparent HTTPS connection redirected by iptables.
/// Synthesize a CONNECT request to the host reverse proxy, then pipe TLS data.
#[cfg(target_os = "linux")]
async fn handle_transparent_tls(

    socket: TcpStream,
    initial_data: BytesMut,
    mux_store: SharedMultiplexer,
    original_dst: std::net::SocketAddr,
) -> Result<()> {
    let sni = extract_sni(&initial_data);
    debug!("SNI extraction result: {:?}, original_dst={}", sni, original_dst);
    let host = sni.unwrap_or_else(|| original_dst.ip().to_string());
    let port = original_dst.port();

    debug!("Transparent HTTPS: sending CONNECT {}:{}", host, port);

    let connect_header = format!(
        "CONNECT {}:{} HTTP/1.1\r\nHost: {}:{}\r\n\r\n",
        host, port, host, port
    );

    let mux = {
        let lock = mux_store.read().await;
        lock.clone()
    };
    let mux = mux.ok_or_else(|| anyhow!("No active VSOCK connection"))?;

    let mut vsock_stream = mux.create_reverse_stream(bytes::Bytes::from(connect_header)).await?;

    // Read the tunnel establishment response from host
    let response = vsock_stream.read().await?;
    let response_str = String::from_utf8_lossy(&response);
    debug!("CONNECT response: {}", response_str.lines().next().unwrap_or("(empty)"));
    if !response_str.contains("200") {
        return Err(anyhow!("CONNECT rejected: {}", response_str.lines().next().unwrap_or("")));
    }

    debug!("CONNECT tunnel established for {}:{}", host, port);

    // Forward the buffered TLS ClientHello
    vsock_stream.write(&initial_data).await?;

    // Bidirectional pipe
    pump(socket, Box::new(vsock_stream)).await
}

/// Extract SNI hostname from a TLS ClientHello message.
fn extract_sni(data: &[u8]) -> Option<String> {
    if data.len() < 5 || data[0] != 0x16 {
        return None;
    }
    let record_len = u16::from_be_bytes([data[3], data[4]]) as usize;
    if data.len() < 5 + record_len { return None; }

    let hs = &data[5..];
    if hs.is_empty() || hs[0] != 0x01 { return None; } // ClientHello
    if hs.len() < 4 { return None; }
    let hs_len = u32::from_be_bytes([0, hs[1], hs[2], hs[3]]) as usize;
    if hs.len() < 4 + hs_len { return None; }

    let ch = &hs[4..4 + hs_len];
    if ch.len() < 34 { return None; } // version(2) + random(32)

    let mut pos = 34;

    // Session ID
    if pos >= ch.len() { return None; }
    let sid_len = ch[pos] as usize;
    pos += 1 + sid_len;

    // Cipher suites
    if pos + 2 > ch.len() { return None; }
    let cs_len = u16::from_be_bytes([ch[pos], ch[pos + 1]]) as usize;
    pos += 2 + cs_len;

    // Compression methods
    if pos >= ch.len() { return None; }
    let cm_len = ch[pos] as usize;
    pos += 1 + cm_len;

    // Extensions
    if pos + 2 > ch.len() { return None; }
    let ext_total = u16::from_be_bytes([ch[pos], ch[pos + 1]]) as usize;
    pos += 2;
    let ext_end = pos + ext_total;

    while pos + 4 <= ext_end && pos + 4 <= ch.len() {
        let ext_type = u16::from_be_bytes([ch[pos], ch[pos + 1]]);
        let ext_len = u16::from_be_bytes([ch[pos + 2], ch[pos + 3]]) as usize;
        pos += 4;

        if ext_type == 0x0000 {
            // SNI extension
            if pos + ext_len > ch.len() || ext_len < 5 { return None; }
            let sni = &ch[pos..pos + ext_len];
            // sni_list_len(2) + name_type(1) + name_len(2) + name
            let name_type = sni[2];
            if name_type != 0 { return None; } // host_name
            let name_len = u16::from_be_bytes([sni[3], sni[4]]) as usize;
            if sni.len() < 5 + name_len { return None; }
            return std::str::from_utf8(&sni[5..5 + name_len]).ok().map(|s| s.to_string());
        }

        pos += ext_len;
    }

    None
}

/// Get the original destination of a connection redirected by iptables REDIRECT.
#[cfg(target_os = "linux")]
fn get_original_dst(socket: &TcpStream) -> Result<std::net::SocketAddr> {
    use std::os::unix::io::AsRawFd;

    const SO_ORIGINAL_DST: libc::c_int = 80;

    let fd = socket.as_raw_fd();
    let mut addr: libc::sockaddr_in = unsafe { std::mem::zeroed() };
    let mut len: libc::socklen_t = std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;

    let ret = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_IP,
            SO_ORIGINAL_DST,
            &mut addr as *mut _ as *mut libc::c_void,
            &mut len,
        )
    };

    if ret != 0 {
        return Err(anyhow!("getsockopt SO_ORIGINAL_DST failed: {}", std::io::Error::last_os_error()));
    }

    let ip = std::net::Ipv4Addr::from(u32::from_be(addr.sin_addr.s_addr));
    let port = u16::from_be(addr.sin_port);
    Ok(std::net::SocketAddr::new(std::net::IpAddr::V4(ip), port))
}

struct ProxyTarget {
    host: String,
    port: u16,
    is_connect: bool,
}

/// Parse the HTTP request line to extract the target host and port.
fn extract_target(headers: &[u8]) -> Option<ProxyTarget> {
    let header_str = std::str::from_utf8(headers).ok()?;
    let request_line = header_str.lines().next()?;
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }

    let method = parts[0];
    let uri = parts[1];

    if method.eq_ignore_ascii_case("CONNECT") {
        // CONNECT host:port HTTP/1.x
        let (host, port) = parse_host_port(uri, 443)?;
        return Some(ProxyTarget { host, port, is_connect: true });
    }

    // Plain HTTP: GET http://host:port/path HTTP/1.x
    if let Some(rest) = uri.strip_prefix("http://") {
        let (authority, _path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        let (host, port) = parse_host_port(authority, 80)?;
        return Some(ProxyTarget { host, port, is_connect: false });
    }

    // Fallback: check Host header
    for line in header_str.lines().skip(1) {
        if let Some(value) = line.strip_prefix("Host:").or_else(|| line.strip_prefix("host:")) {
            let value = value.trim();
            let (host, port) = parse_host_port(value, 80)?;
            return Some(ProxyTarget { host, port, is_connect: false });
        }
    }

    None
}

/// Parse "host:port" or "host" with a default port.
fn parse_host_port(s: &str, default_port: u16) -> Option<(String, u16)> {
    // Handle IPv6 bracket notation: [::1]:port
    if s.starts_with('[') {
        if let Some(bracket_end) = s.find(']') {
            let host = &s[1..bracket_end];
            let port = if s.len() > bracket_end + 1 && s.as_bytes()[bracket_end + 1] == b':' {
                s[bracket_end + 2..].parse().ok()?
            } else {
                default_port
            };
            return Some((host.to_string(), port));
        }
        return None;
    }

    match s.rfind(':') {
        Some(i) => {
            if let Ok(port) = s[i + 1..].parse::<u16>() {
                Some((s[..i].to_string(), port))
            } else {
                Some((s.to_string(), default_port))
            }
        }
        None => Some((s.to_string(), default_port)),
    }
}

fn is_localhost(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "::1" | "[::1]")
}

async fn handle_localhost_direct(
    mut client: TcpStream,
    target: ProxyTarget,
    headers_buf: BytesMut,
    body: BytesMut,
) -> Result<()> {
    let addr = format!("127.0.0.1:{}", target.port);
    let mut upstream = TcpStream::connect(&addr).await
        .map_err(|e| anyhow!("Failed to connect to {}: {}", addr, e))?;

    if target.is_connect {
        // CONNECT tunnel: send 200 back to client, then bidirectional copy
        client.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await?;
        if !body.is_empty() {
            upstream.write_all(&body).await?;
        }
        pump_tcp(client, upstream).await
    } else {
        // Plain HTTP: rewrite absolute URI to relative path, forward headers + body to upstream
        let rewritten = rewrite_request_uri(&headers_buf);
        upstream.write_all(&rewritten).await?;
        if !body.is_empty() {
            upstream.write_all(&body).await?;
        }
        pump_tcp(client, upstream).await
    }
}

/// Rewrite "GET http://host:port/path HTTP/1.x" → "GET /path HTTP/1.x"
/// for forwarding to a local upstream that expects relative URIs.
fn rewrite_request_uri(header_bytes: &[u8]) -> Vec<u8> {
    let header_str = match std::str::from_utf8(header_bytes) {
        Ok(s) => s,
        Err(_) => return header_bytes.to_vec(),
    };

    if let Some(first_line_end) = header_str.find("\r\n") {
        let request_line = &header_str[..first_line_end];
        let parts: Vec<&str> = request_line.splitn(3, ' ').collect();
        if parts.len() == 3 {
            let uri = parts[1];
            if let Some(rest) = uri.strip_prefix("http://") {
                let path = match rest.find('/') {
                    Some(i) => &rest[i..],
                    None => "/",
                };
                let new_line = format!("{} {} {}", parts[0], path, parts[2]);
                let mut result = new_line.into_bytes();
                result.extend_from_slice(header_str[first_line_end..].as_bytes());
                return result;
            }
        }
    }
    header_bytes.to_vec()
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(target_os = "linux")]
async fn handle_host_connect(socket: TcpStream, mux_store: SharedMultiplexer, port: u16, mode: u8) -> Result<()> {
    let mux_guard = mux_store.read().await;
    let mux = mux_guard.as_ref().ok_or_else(|| anyhow!("No multiplexer available for HostConnect"))?;
    let vsock_stream = mux.create_host_connect_stream(port as u32, mode).await?;
    drop(mux_guard);
    pump(socket, Box::new(vsock_stream)).await
}

/// Map cdp.nilbox hostname variants to HostConnect mode bytes.
/// Returns None if the host is not a cdp.nilbox sentinel.
fn nilbox_cdp_mode(host: &str) -> Option<u8> {
    match host {
        "cdp.nilbox"          => Some(0x00), // Auto
        "headless.cdp.nilbox" => Some(0x01), // Headless
        "headed.cdp.nilbox"   => Some(0x02), // Headed
        _ => None,
    }
}

/// Handle a cdp.nilbox request arriving via the http_proxy env var path.
///
/// When `http_proxy=http://127.0.0.1:18088` is set, curl/apps connect directly
/// to 18088 without going through DNS + iptables REDIRECT, so SO_ORIGINAL_DST
/// returns 127.0.0.1 and the sentinel IP check is skipped.  We detect the
/// cdp.nilbox hostname here and route via a HostConnect VSOCK tunnel instead.
#[cfg(target_os = "linux")]
async fn handle_nilbox_cdp_proxy(
    mut socket: TcpStream,
    headers_buf: BytesMut,
    body: BytesMut,
    mux_store: SharedMultiplexer,
    port: u16,
    mode: u8,
    is_connect: bool,
) -> Result<()> {
    let mux_guard = mux_store.read().await;
    let mux = mux_guard.as_ref().ok_or_else(|| anyhow!("No multiplexer available for HostConnect"))?;
    let mut vsock_stream = mux.create_host_connect_stream(port as u32, mode).await?;
    drop(mux_guard);

    if is_connect {
        // CONNECT tunnel: send 200 to client then pipe raw bytes
        socket.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await?;
        if !body.is_empty() {
            vsock_stream.write(&body).await?;
        }
    } else {
        // Plain HTTP: rewrite absolute URI to relative path before forwarding
        let rewritten = rewrite_request_uri(&headers_buf);
        vsock_stream.write(&rewritten).await?;
        if !body.is_empty() {
            vsock_stream.write(&body).await?;
        }
    }

    pump(socket, Box::new(vsock_stream)).await
}
