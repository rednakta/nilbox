//! FUSE client module — mounts host shared directory at the path sent by host.

pub mod protocol;
pub mod dispatcher;
pub mod cache;
pub mod filesystem;

use dispatcher::RequestDispatcher;
use filesystem::HostFilesystem;

use crate::vsock::stream::VirtualStream;
use crate::vsock::VsockStream as _;
use anyhow::Result;
use std::sync::Arc;
use tracing::{debug, error, warn};

/// Handle an inbound FUSE stream initiated by the host.
/// Called from the inbound handler when a stream with port == FUSE_PORT arrives.
pub async fn handle_fuse_stream(mut stream: VirtualStream) -> Result<()> {
    let rt = tokio::runtime::Handle::current();

    // Read mount point handshake: u16_le(len) + path bytes
    let mount_point = {
        let data = stream.read().await.map_err(|e| {
            anyhow::anyhow!("Failed to read mount point handshake: {}", e)
        })?;

        if data.len() < 2 {
            return Err(anyhow::anyhow!("Mount point handshake too short: {} bytes", data.len()));
        }

        let path_len = u16::from_le_bytes([data[0], data[1]]) as usize;
        if data.len() < 2 + path_len {
            return Err(anyhow::anyhow!(
                "Mount point handshake incomplete: expected {} bytes, got {}",
                2 + path_len,
                data.len()
            ));
        }

        let path = std::str::from_utf8(&data[2..2 + path_len])
            .map_err(|e| anyhow::anyhow!("Mount point is not valid UTF-8: {}", e))?
            .to_string();

        if path.is_empty() {
            warn!("Empty mount point received, falling back to /mnt/host");
            "/mnt/host".to_string()
        } else {
            path
        }
    };

    debug!("FUSE: received mount point from host: {}", mount_point);

    if let Err(e) = std::fs::create_dir_all(&mount_point) {
        error!("Failed to create mount point {}: {}", mount_point, e);
        return Err(e.into());
    }

    let writer = stream.writer();
    let dispatcher = Arc::new(RequestDispatcher::new(writer));
    let _reader_handle = dispatcher.clone().start_reader(stream);

    let mut fs = HostFilesystem::new(dispatcher.clone(), rt.clone());
    fs.setup_notification_handler().await;

    debug!("Mounting FUSE filesystem at {}", mount_point);

    tokio::task::spawn_blocking(move || {
        let options = &[
            fuser::MountOption::FSName("nilbox-host".to_string()),
            fuser::MountOption::AllowOther,
        ];
        if let Err(e) = fuser::mount2(fs, &mount_point, options) {
            error!("FUSE mount failed: {}", e);
        }
        debug!("FUSE filesystem unmounted");
    });

    Ok(())
}
