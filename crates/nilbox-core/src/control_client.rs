//! Typed client for sending JSON commands to the vm-agent Control Port (9402).
//!
//! Each method opens a VSOCK stream to CONTROL_PORT, sends a JSON command,
//! reads the response with a timeout, and returns a typed result.
//! This replaces all SSH `exec_command()` usage for non-interactive operations.

use crate::vsock::stream::StreamMultiplexer;
use crate::vsock::VsockStream;

use std::sync::Arc;
use std::time::Duration;
use anyhow::{Result, anyhow};
use serde::Deserialize;

const CONTROL_PORT: u32 = 9402;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Response envelope from the vm-agent control handler.
#[derive(Deserialize)]
struct ControlResponse {
    status: String,
    #[serde(default)]
    data: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<String>,
}

/// Typed client for vm-agent Control Port commands.
pub struct ControlClient {
    multiplexer: Arc<StreamMultiplexer>,
}

impl ControlClient {
    pub fn new(multiplexer: Arc<StreamMultiplexer>) -> Self {
        Self { multiplexer }
    }

    /// Send a JSON command and receive a typed response.
    async fn send_command(&self, cmd: serde_json::Value) -> Result<ControlResponse> {
        let mut stream = self.multiplexer.create_stream(CONTROL_PORT).await
            .map_err(|e| anyhow!("Failed to open control stream: {}", e))?;

        stream.write(cmd.to_string().as_bytes()).await
            .map_err(|e| anyhow!("Failed to write control command: {}", e))?;

        let data = tokio::time::timeout(DEFAULT_TIMEOUT, stream.read()).await
            .map_err(|_| anyhow!("Control command timed out ({}s)", DEFAULT_TIMEOUT.as_secs()))?
            .map_err(|e| anyhow!("Failed to read control response: {}", e))?;

        let _ = stream.close().await;

        let resp: ControlResponse = serde_json::from_slice(&data)
            .map_err(|e| {
                let raw = String::from_utf8_lossy(&data);
                // Backwards compatibility: old vm-agent returns plain "ok" or "error: ..."
                if raw.starts_with("ok") {
                    return anyhow!("__legacy_ok__");
                }
                if raw.starts_with("error:") {
                    return anyhow!("{}", raw);
                }
                anyhow!("Invalid control response JSON: {} (raw: {})", e, raw)
            })?;

        if resp.status == "error" {
            return Err(anyhow!("{}", resp.error.unwrap_or_else(|| "Unknown error".to_string())));
        }

        Ok(resp)
    }

    // ── Filesystem info ─────────────────────────────────────────────────

    /// Get filesystem info for a mount point inside the VM.
    pub async fn get_fs_info(&self, mount_point: &str) -> Result<VmFsInfoRaw> {
        let cmd = serde_json::json!({
            "action": "get_fs_info",
            "mount_point": mount_point,
        });

        let resp = self.send_command(cmd).await?;
        let data = resp.data.ok_or_else(|| anyhow!("Missing data in get_fs_info response"))?;

        Ok(VmFsInfoRaw {
            device: data["device"].as_str().unwrap_or("unknown").to_string(),
            total_mb: data["total_mb"].as_u64().unwrap_or(0),
            used_mb: data["used_mb"].as_u64().unwrap_or(0),
            avail_mb: data["avail_mb"].as_u64().unwrap_or(0),
            use_pct: data["use_pct"].as_u64().unwrap_or(0),
        })
    }

    // ── System metrics ──────────────────────────────────────────────────

    /// Get raw /proc/stat + /proc/meminfo + /proc/net/dev combined text.
    pub async fn get_system_metrics(&self) -> Result<Vec<u8>> {
        let cmd = serde_json::json!({ "action": "get_system_metrics" });

        let resp = self.send_command(cmd).await?;
        let data = resp.data.ok_or_else(|| anyhow!("Missing data in get_system_metrics response"))?;

        let raw = data["raw"].as_str().unwrap_or("");
        Ok(raw.as_bytes().to_vec())
    }

    // ── Write file ──────────────────────────────────────────────────────

    /// Write content to an allowlisted path inside the VM.
    pub async fn write_file(
        &self,
        path: &str,
        content: &[u8],
        mode: &str,
        owner: &str,
    ) -> Result<()> {
        use base64::{Engine as _, engine::general_purpose::STANDARD};

        let cmd = serde_json::json!({
            "action": "write_file",
            "path": path,
            "content_b64": STANDARD.encode(content),
            "mode": mode,
            "owner": owner,
        });

        self.send_command(cmd).await?;
        Ok(())
    }

    // ── Update CA certificates ──────────────────────────────────────────

    /// Run the system CA certificate update chain inside the VM.
    pub async fn update_ca_certificates(&self) -> Result<String> {
        let cmd = serde_json::json!({ "action": "update_ca_certificates" });

        let resp = self.send_command(cmd).await?;
        let output = resp.data
            .and_then(|d| d["output"].as_str().map(|s| s.to_string()))
            .unwrap_or_default();
        Ok(output)
    }

    // ── Expand partition ────────────────────────────────────────────────

    /// Run growpart + resize2fs inside the VM for the given root device.
    pub async fn expand_partition(&self, root_device: &str) -> Result<String> {
        let cmd = serde_json::json!({
            "action": "expand_partition",
            "root_device": root_device,
        });

        let resp = self.send_command(cmd).await?;
        let output = resp.data
            .and_then(|d| d["output"].as_str().map(|s| s.to_string()))
            .unwrap_or_default();
        Ok(output)
    }

    // ── Notify env changed ──────────────────────────────────────────────

    /// Notify the vm-agent that environment variables have been updated.
    pub async fn notify_env_changed(&self) -> Result<()> {
        let cmd = serde_json::json!({ "action": "notify_env_changed" });
        self.send_command(cmd).await?;
        Ok(())
    }

    // ── Ensure directory ───────────────────────────────────────────────

    /// Create a directory (and parents) inside the VM.
    pub async fn ensure_dir(&self, path: &str) -> Result<()> {
        let cmd = serde_json::json!({
            "action": "ensure_dir",
            "path": path,
        });
        self.send_command(cmd).await?;
        Ok(())
    }

    // ── Update MCP config ──────────────────────────────────────────────

    /// Write MCP server configuration to the VM and signal mcp-stdio-proxy
    /// to reload. `config` is serialized to JSON and written to
    /// `/etc/nilbox/mcp-servers.json` inside the VM.
    pub async fn update_mcp_config(&self, config: &serde_json::Value) -> Result<()> {
        let cmd = serde_json::json!({
            "action": "update_mcp_config",
            "config_json": config.to_string(),
        });
        self.send_command(cmd).await?;
        Ok(())
    }
}

/// Filesystem info returned by `get_fs_info`.
#[derive(Debug)]
pub struct VmFsInfoRaw {
    pub device: String,
    pub total_mb: u64,
    pub used_mb: u64,
    pub avail_mb: u64,
    pub use_pct: u64,
}
