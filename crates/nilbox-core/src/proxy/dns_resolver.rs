//! Host-side DNS resolver — resolves DNS queries from the VM using system DNS

use crate::vsock::stream::VirtualStream;
use crate::vsock::VsockStream;
use crate::vsock::protocol::Frame;
use anyhow::{Result, anyhow};
use tokio::net::UdpSocket;
use tracing::{warn, debug};

/// Handle a DNS request from the VM: read query, forward to system DNS, return response.
pub async fn handle_dns(mut stream: VirtualStream) -> Result<()> {
    let query = stream.read().await?;
    let qname = extract_qname(&query);
    debug!("DNS request: {} bytes, qname={}, stream {}", query.len(), qname, stream.stream_id);

    // Synthetic resolution: cdp.nilbox variants → sentinel IPs
    // cdp.nilbox          → 169.254.254.1 (Auto — uses Settings)
    // headless.cdp.nilbox → 169.254.254.2 (Headless override)
    // headed.cdp.nilbox   → 169.254.254.3 (Headed override)
    let synthetic_ip: Option<[u8; 4]> = if qname == "cdp.nilbox" || qname == "cdp.nilbox." {
        Some([169, 254, 254, 1])
    } else if qname == "headless.cdp.nilbox" || qname == "headless.cdp.nilbox." {
        Some([169, 254, 254, 2])
    } else if qname == "headed.cdp.nilbox" || qname == "headed.cdp.nilbox." {
        Some([169, 254, 254, 3])
    } else {
        None
    };
    if let Some(ip) = synthetic_ip {
        debug!("DNS: synthetic {} -> {}.{}.{}.{}", qname, ip[0], ip[1], ip[2], ip[3]);
        let response = build_a_record(&query, ip);
        let frame = Frame::dns_response(stream.stream_id, response);
        stream.send_frame(frame).await?;
        let _ = stream.close().await;
        return Ok(());
    }

    let response = match resolve_dns(&query).await {
        Ok(resp) => {
            debug!("DNS response: {} bytes for qname={}, stream {}", resp.len(), qname, stream.stream_id);
            resp
        }
        Err(e) => {
            warn!("DNS resolution failed for qname={}: {}", qname, e);
            build_servfail(&query).unwrap_or_default()
        }
    };

    let frame = Frame::dns_response(stream.stream_id, response);
    stream.send_frame(frame).await?;
    let _ = stream.close().await;
    Ok(())
}

async fn resolve_dns(query: &[u8]) -> Result<Vec<u8>> {
    let dns_server = get_system_dns().await;
    // debug!("Forwarding DNS query to {}", dns_server);
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    socket.send_to(query, &dns_server).await?;

    let mut buf = vec![0u8; 4096];
    let (len, _) = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        socket.recv_from(&mut buf),
    )
    .await
    .map_err(|_| anyhow!("DNS upstream timeout from {}", dns_server))??;

    Ok(buf[..len].to_vec())
}

async fn get_system_dns() -> String {
    // On macOS, 127.0.0.1 in resolv.conf is mDNSResponder (the real system resolver)
    // — it's safe and correct to use. Only skip 127.0.0.53 (systemd-resolved stub on Linux).
    if let Ok(content) = tokio::fs::read_to_string("/etc/resolv.conf").await {
        for line in content.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("nameserver") {
                let server = rest.trim();
                if server == "127.0.0.53" {
                    // systemd-resolved stub — skip (may have no upstream in our zero-NIC VM)
                    continue;
                }
                if !server.is_empty() {
                    return format!("{}:53", server);
                }
            }
        }
    }

    "8.8.8.8:53".to_string()
}

/// Extract the QNAME from a DNS query for logging
fn extract_qname(query: &[u8]) -> String {
    if query.len() < 13 {
        return "<invalid>".to_string();
    }
    let mut pos = 12; // skip header
    let mut parts = Vec::new();
    while pos < query.len() {
        let len = query[pos] as usize;
        if len == 0 {
            break;
        }
        pos += 1;
        if pos + len > query.len() {
            return "<truncated>".to_string();
        }
        if let Ok(label) = std::str::from_utf8(&query[pos..pos + len]) {
            parts.push(label.to_string());
        } else {
            return "<non-utf8>".to_string();
        }
        pos += len;
    }
    if parts.is_empty() {
        "<empty>".to_string()
    } else {
        parts.join(".")
    }
}

/// Build a synthetic DNS A record response
fn build_a_record(query: &[u8], ip: [u8; 4]) -> Vec<u8> {
    if query.len() < 12 {
        return Vec::new();
    }
    // Find end of question section (skip header + qname + qtype + qclass)
    let mut pos = 12;
    while pos < query.len() && query[pos] != 0 {
        pos += 1 + query[pos] as usize;
    }
    pos += 1 + 4; // null terminator + qtype(2) + qclass(2)
    let question_section = &query[12..pos.min(query.len())];

    let mut resp = Vec::with_capacity(pos + 16);
    // Header
    resp.push(query[0]); // Transaction ID
    resp.push(query[1]);
    resp.extend_from_slice(&[0x81, 0x80]); // Flags: QR=1, RD=1, RA=1
    resp.extend_from_slice(&[0x00, 0x01]); // QDCOUNT=1
    resp.extend_from_slice(&[0x00, 0x01]); // ANCOUNT=1
    resp.extend_from_slice(&[0x00, 0x00]); // NSCOUNT=0
    resp.extend_from_slice(&[0x00, 0x00]); // ARCOUNT=0
    // Question section (copy from query)
    resp.extend_from_slice(question_section);
    // Answer section
    resp.extend_from_slice(&[0xC0, 0x0C]); // Name pointer to offset 12
    resp.extend_from_slice(&[0x00, 0x01]); // Type A
    resp.extend_from_slice(&[0x00, 0x01]); // Class IN
    resp.extend_from_slice(&[0x00, 0x00, 0x00, 0x3C]); // TTL=60s
    resp.extend_from_slice(&[0x00, 0x04]); // RDLENGTH=4
    resp.extend_from_slice(&ip);
    resp
}

fn build_servfail(query: &[u8]) -> Option<Vec<u8>> {
    if query.len() < 12 {
        return None;
    }
    let mut resp = vec![0u8; 12];
    resp[0] = query[0];
    resp[1] = query[1];
    resp[2] = 0x80;
    resp[3] = 0x02;
    resp[4] = query[4];
    resp[5] = query[5];
    Some(resp)
}
