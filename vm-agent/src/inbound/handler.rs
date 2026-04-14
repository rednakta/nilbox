//! Inbound stream handler (Host -> VM service or control)

use crate::vsock::stream::VirtualStream;
use crate::vsock::{VsockStream, CONTROL_PORT};
use crate::utils::pump;
use crate::inbound::control;
use anyhow::{Result, anyhow};
use bytes::Buf;
use tokio::net::TcpStream;
use tracing::{debug, error, warn};

pub async fn handle_inbound_stream(mut stream: VirtualStream) -> Result<()> {
    // Read Connect payload (4 bytes: target port)
    let payload = match stream.read().await {
        Ok(p) => p,
        Err(e) => {
            error!("Failed to read Connect payload: {}", e);
            return Err(e);
        }
    };

    if payload.len() < 4 {
        warn!("Invalid Connect payload length: {}", payload.len());
        stream.close().await?;
        return Err(anyhow!("Invalid Connect payload"));
    }

    let mut buf = payload;
    let target_port = buf.get_u32();

    debug!("Inbound connection request to port {}", target_port);

    // Route control commands to the control handler
    if target_port == CONTROL_PORT {
        debug!("Routing to control handler");
        return control::handle_control_stream(stream).await;
    }

    // Route FUSE requests to the FUSE filesystem handler
    #[cfg(feature = "with-fuse")]
    if crate::fuse::protocol::is_fuse_port(target_port) {
        debug!("Routing to FUSE handler");
        return crate::fuse::handle_fuse_stream(stream).await;
    }

    // Forward to local TCP service (port 22 for sshd, etc.)
    let target_addr = format!("127.0.0.1:{}", target_port);
    match TcpStream::connect(&target_addr).await {
        Ok(tcp_stream) => pump(tcp_stream, Box::new(stream)).await,
        Err(e) => {
            error!("Failed to connect to local service {}: {}", target_addr, e);
            stream.close().await?;
            Ok(())
        }
    }
}
