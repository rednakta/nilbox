//! DNS forwarder — relays DNS queries over VSOCK to the host

use crate::outbound::proxy::SharedMultiplexer;
use crate::vsock::VsockStream;
use anyhow::{Result, anyhow};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tracing::{debug, error, warn};

pub struct DnsForwarder {
    mux_store: SharedMultiplexer,
}

impl DnsForwarder {
    pub fn new(mux_store: SharedMultiplexer) -> Self {
        Self { mux_store }
    }

    pub async fn run(&self) -> Result<()> {
        let socket = Arc::new(UdpSocket::bind("127.0.0.53:53").await?);
        debug!("DNS forwarder listening on 127.0.0.53:53");

        let mut buf = vec![0u8; 512];
        loop {
            let (len, src_addr) = socket.recv_from(&mut buf).await?;
            let query = buf[..len].to_vec();
            let mux_store = self.mux_store.clone();
            let sock = socket.clone();

            tokio::spawn(async move {
                match handle_dns_query(query.clone(), mux_store).await {
                    Ok(response) => {
                        if let Err(e) = sock.send_to(&response, src_addr).await {
                            error!("Failed to send DNS response to {}: {}", src_addr, e);
                        }
                    }
                    Err(e) => {
                        warn!("DNS query failed: {}, sending SERVFAIL to {}", e, src_addr);
                        if let Some(servfail) = build_servfail(&query) {
                            let _ = sock.send_to(&servfail, src_addr).await;
                        }
                    }
                }
            });
        }
    }
}

async fn handle_dns_query(query: Vec<u8>, mux_store: SharedMultiplexer) -> Result<Vec<u8>> {
    let mux = {
        let lock = mux_store.read().await;
        lock.clone()
    };

    let mux = mux.ok_or_else(|| anyhow!("No active VSOCK connection"))?;

    let mut stream = mux.create_dns_stream(bytes::Bytes::from(query)).await?;

    let response = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        stream.read(),
    )
    .await
    .map_err(|_| anyhow!("DNS response timeout"))??;

    let _ = stream.close().await;

    Ok(response.to_vec())
}

/// Build a minimal SERVFAIL response from the original query
fn build_servfail(query: &[u8]) -> Option<Vec<u8>> {
    if query.len() < 12 {
        return None;
    }
    let mut resp = vec![0u8; 12];
    resp[0] = query[0];
    resp[1] = query[1];
    resp[2] = 0x80; // QR=1, Opcode=0, AA=0, TC=0, RD=0
    resp[3] = 0x02; // RA=0, Z=0, RCODE=2 (SERVFAIL)
    resp[4] = query[4];
    resp[5] = query[5];
    Some(resp)
}
