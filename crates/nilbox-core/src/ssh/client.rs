//! SSH client using russh — connects over VirtualStream via DuplexStream adapter

use anyhow::{bail, Context, Result};
use russh::keys::*;
use russh::*;
use std::sync::Arc;
use tokio::io::DuplexStream;
use tracing::debug;

/// Handler for russh client events.
struct SshHandler;

impl client::Handler for SshHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        // VM is locally controlled — skip host key verification
        Ok(true)
    }
}

/// SSH client wrapping a russh Handle + optional active shell channel.
pub struct SshClient {
    handle: client::Handle<SshHandler>,
    channel: Option<Channel<client::Msg>>,
}

/// A raw SSH channel for external management (e.g., shell session read loops).
pub type SshChannel = Channel<client::Msg>;

impl SshClient {
    /// Perform SSH handshake + public-key authentication over the given stream.
    pub async fn connect(
        stream: DuplexStream,
        private_key: Arc<PrivateKey>,
    ) -> Result<Self> {
        let config = Arc::new(client::Config {
            inactivity_timeout: None,
            ..<_>::default()
        });

        let mut handle = client::connect_stream(config, stream, SshHandler)
            .await
            .context("SSH handshake failed")?;

        let auth_result = handle
            .authenticate_publickey(
                "nilbox",
                PrivateKeyWithHashAlg::new(
                    private_key,
                    handle.best_supported_rsa_hash().await
                        .context("Failed to negotiate RSA hash")?
                        .flatten(),
                ),
            )
            .await
            .context("SSH authentication request failed")?;

        if !auth_result.success() {
            bail!("SSH public-key authentication failed");
        }

        debug!("SSH connection established and authenticated");
        Ok(Self { handle, channel: None })
    }

    /// Open a new interactive shell channel with PTY, storing it internally.
    pub async fn open_shell(
        &mut self,
        cols: u32,
        rows: u32,
    ) -> Result<u64> {
        let channel = self.handle.channel_open_session().await
            .context("Failed to open SSH session channel")?;

        channel
            .request_pty(
                true,
                "xterm-256color",
                cols,
                rows,
                0,
                0,
                &[],
            )
            .await
            .context("Failed to request PTY")?;

        // Login shell sources /etc/profile.d/nilbox-proxy.sh (REQUESTS_CA_BUNDLE etc.)
        channel
            .exec(true, "bash --login")
            .await
            .context("Failed to start login shell")?;

        let session_id: u64 = u32::from(channel.id()).into();
        debug!("SSH shell channel opened ({}x{}), session_id={}", cols, rows, session_id);
        self.channel = Some(channel);
        Ok(session_id)
    }

    /// Open a PTY exec channel to run a command interactively (e.g. nilbox-install).
    /// Returns the raw channel for external management.
    pub async fn open_install_channel(
        &mut self,
        cols: u32,
        rows: u32,
        cmd: &str,
    ) -> Result<SshChannel> {
        let channel = self.handle.channel_open_session().await
            .context("Failed to open SSH session channel")?;

        channel
            .request_pty(true, "xterm-256color", cols, rows, 0, 0, &[])
            .await
            .context("Failed to request PTY")?;

        channel
            .exec(true, cmd)
            .await
            .context("Failed to exec install command")?;

        debug!("SSH install channel opened ({}x{}), cmd={}", cols, rows, cmd);
        Ok(channel)
    }

    /// Open a new interactive shell channel with PTY, returning the raw channel
    /// for external management (background read loop).
    pub async fn open_shell_channel(
        &mut self,
        cols: u32,
        rows: u32,
    ) -> Result<SshChannel> {
        let channel = self.handle.channel_open_session().await
            .context("Failed to open SSH session channel")?;

        channel
            .request_pty(true, "xterm-256color", cols, rows, 0, 0, &[])
            .await
            .context("Failed to request PTY")?;

        // Login shell sources /etc/profile.d/nilbox-proxy.sh (REQUESTS_CA_BUNDLE etc.)
        channel
            .exec(true, "bash --login")
            .await
            .context("Failed to start login shell")?;

        debug!("SSH shell channel opened ({}x{})", cols, rows);
        Ok(channel)
    }

    /// Write data to the active shell channel.
    pub async fn write_data(&self, data: &[u8]) -> Result<()> {
        let channel = self.channel.as_ref()
            .ok_or_else(|| anyhow::anyhow!("No shell channel active"))?;
        channel.data(&data[..]).await
            .context("Failed to write to shell channel")?;
        Ok(())
    }

    /// Resize the active shell PTY.
    pub async fn resize_pty(&self, cols: u32, rows: u32) -> Result<()> {
        let channel = self.channel.as_ref()
            .ok_or_else(|| anyhow::anyhow!("No shell channel active"))?;
        channel.window_change(cols, rows, 0, 0).await
            .context("Failed to resize PTY")?;
        Ok(())
    }

    /// Close the active shell channel.
    pub async fn close_shell(&mut self) -> Result<()> {
        if let Some(channel) = self.channel.take() {
            channel.close().await.context("Failed to close shell channel")?;
        }
        Ok(())
    }

    // exec_command() removed — all non-interactive VM operations now use
    // ControlClient (Control Port 9402) instead of SSH exec channels.
    // This eliminates arbitrary shell command execution capability.

    /// Check if the SSH session is still alive.
    pub fn is_connected(&self) -> bool {
        !self.handle.is_closed()
    }

    /// Disconnect gracefully.
    pub async fn disconnect(&self) -> Result<()> {
        self.handle
            .disconnect(Disconnect::ByApplication, "", "")
            .await
            .context("Failed to disconnect SSH")?;
        Ok(())
    }
}
