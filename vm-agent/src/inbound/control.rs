//! CONTROL_PORT handler — receives JSON commands from the Host
//!
//! Supported actions: inject_ssh_key, install_app, get_fs_info,
//! get_system_metrics, write_file, update_ca_certificates, expand_partition,
//! notify_env_changed, update_mcp_config.

use crate::app_install::{self, VerifyConfig};
use crate::vsock::stream::VirtualStream;
use crate::vsock::VsockStream;
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use tracing::{debug, error};
#[cfg(target_os = "linux")]
use libc;

pub mod file_ops;
pub mod system_ops;

// ── Command / Response types ────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(tag = "action")]
enum ControlCommand {
    // Existing
    #[serde(rename = "inject_ssh_key")]
    InjectSshKey { pubkey: String },
    #[serde(rename = "install_app")]
    InstallApp {
        uuid: String,
        id: String,
        #[serde(default)]
        verify_token: Option<String>,
        #[serde(default)]
        store_callback_url: Option<String>,
    },

    // New — system operations
    #[serde(rename = "get_fs_info")]
    GetFsInfo { mount_point: String },
    #[serde(rename = "get_system_metrics")]
    GetSystemMetrics {},
    #[serde(rename = "expand_partition")]
    ExpandPartition { root_device: String },

    // New — file operations
    #[serde(rename = "write_file")]
    WriteFile {
        path: String,
        content_b64: String,
        #[serde(default)]
        mode: Option<String>,
        #[serde(default)]
        owner: Option<String>,
    },
    #[serde(rename = "update_ca_certificates")]
    UpdateCaCertificates {},
    #[serde(rename = "notify_env_changed")]
    NotifyEnvChanged {},
    #[serde(rename = "ensure_dir")]
    EnsureDir { path: String },
    #[serde(rename = "update_mcp_config")]
    UpdateMcpConfig { config_json: String },
}

#[derive(Serialize)]
pub struct ControlResponse {
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ControlResponse {
    pub fn ok(data: Option<serde_json::Value>) -> Self {
        Self { status: "ok", data, error: None }
    }
    pub fn err(msg: String) -> Self {
        Self { status: "error", data: None, error: Some(msg) }
    }
}

// ── Dispatch ────────────────────────────────────────────────────────────

pub async fn handle_control_stream(mut stream: VirtualStream) -> Result<()> {
    let payload = stream.read().await?;
    let text = String::from_utf8_lossy(&payload);
    debug!("Control command received: {}", text);

    let cmd: ControlCommand = match serde_json::from_str(&text) {
        Ok(c) => c,
        Err(e) => {
            let resp = ControlResponse::err(format!("Invalid JSON: {}", e));
            let _ = stream.write(serde_json::to_string(&resp).unwrap().as_bytes()).await;
            let _ = stream.close().await;
            return Err(anyhow!("Invalid control command JSON: {}", e));
        }
    };

    match cmd {
        ControlCommand::InjectSshKey { pubkey } => {
            let resp = match handle_inject_ssh_key(&pubkey).await {
                Ok(_) => ControlResponse::ok(None),
                Err(e) => ControlResponse::err(e.to_string()),
            };
            let _ = stream.write(serde_json::to_string(&resp).unwrap().as_bytes()).await;
            let _ = stream.close().await;
            Ok(())
        }

        ControlCommand::InstallApp { uuid, id, verify_token, store_callback_url } => {
            debug!("App install requested: uuid={}, id={}", uuid, id);

            let verify_config = match (verify_token, store_callback_url) {
                (Some(token), Some(url)) => {
                    debug!("Install verification enabled: callback_url={}", url);
                    Some(VerifyConfig { verify_token: token, callback_url: url })
                }
                _ => None,
            };

            // Send accepted response immediately (install_app keeps legacy format for compatibility)
            let accepted = serde_json::json!({
                "action": "install_app",
                "uuid": &uuid,
                "status": "accepted",
            });
            let _ = stream.write(accepted.to_string().as_bytes()).await;

            let uuid_clone = uuid.clone();
            let id_clone = id.clone();
            tokio::spawn(async move {
                if let Err(e) = app_install::handle_install_app(&uuid_clone, &id_clone, verify_config, &mut stream).await {
                    error!("App install failed: {}", e);
                }
                let _ = stream.close().await;
            });
            Ok(())
        }

        ControlCommand::GetFsInfo { mount_point } => {
            let resp = match system_ops::handle_get_fs_info(&mount_point) {
                Ok(data) => ControlResponse::ok(Some(data)),
                Err(e) => ControlResponse::err(e.to_string()),
            };
            let _ = stream.write(serde_json::to_string(&resp).unwrap().as_bytes()).await;
            let _ = stream.close().await;
            Ok(())
        }

        ControlCommand::GetSystemMetrics {} => {
            let resp = match system_ops::handle_get_system_metrics() {
                Ok(data) => ControlResponse::ok(Some(data)),
                Err(e) => ControlResponse::err(e.to_string()),
            };
            let _ = stream.write(serde_json::to_string(&resp).unwrap().as_bytes()).await;
            let _ = stream.close().await;
            Ok(())
        }

        ControlCommand::ExpandPartition { root_device } => {
            let resp = match system_ops::handle_expand_partition(&root_device).await {
                Ok(output) => ControlResponse::ok(Some(serde_json::json!({ "output": output }))),
                Err(e) => ControlResponse::err(e.to_string()),
            };
            let _ = stream.write(serde_json::to_string(&resp).unwrap().as_bytes()).await;
            let _ = stream.close().await;
            Ok(())
        }

        ControlCommand::WriteFile { path, content_b64, mode, owner } => {
            let resp = match file_ops::handle_write_file(&path, &content_b64, mode.as_deref(), owner.as_deref()) {
                Ok(_) => ControlResponse::ok(None),
                Err(e) => ControlResponse::err(e.to_string()),
            };
            let _ = stream.write(serde_json::to_string(&resp).unwrap().as_bytes()).await;
            let _ = stream.close().await;
            Ok(())
        }

        ControlCommand::UpdateCaCertificates {} => {
            let resp = match file_ops::handle_update_ca_certificates().await {
                Ok(output) => ControlResponse::ok(Some(serde_json::json!({ "output": output }))),
                Err(e) => ControlResponse::err(e.to_string()),
            };
            let _ = stream.write(serde_json::to_string(&resp).unwrap().as_bytes()).await;
            let _ = stream.close().await;
            Ok(())
        }

        ControlCommand::NotifyEnvChanged {} => {
            // Currently a no-op — active shells re-source env on next prompt.
            let resp = ControlResponse::ok(None);
            let _ = stream.write(serde_json::to_string(&resp).unwrap().as_bytes()).await;
            let _ = stream.close().await;
            Ok(())
        }

        ControlCommand::EnsureDir { path } => {
            let resp = if !path.starts_with('/') || path.contains("..") {
                ControlResponse::err(format!("Invalid path: {}", path))
            } else {
                match std::fs::create_dir_all(&path) {
                    Ok(_) => ControlResponse::ok(None),
                    Err(e) => ControlResponse::err(format!("Failed to create {}: {}", path, e)),
                }
            };
            let _ = stream.write(serde_json::to_string(&resp).unwrap().as_bytes()).await;
            let _ = stream.close().await;
            Ok(())
        }

        ControlCommand::UpdateMcpConfig { config_json } => {
            let resp = match file_ops::handle_update_mcp_config(&config_json).await {
                Ok(_) => ControlResponse::ok(None),
                Err(e) => ControlResponse::err(e.to_string()),
            };
            let _ = stream.write(serde_json::to_string(&resp).unwrap().as_bytes()).await;
            let _ = stream.close().await;
            Ok(())
        }
    }
}

// ── SSH key injection ───────────────────────────────────────────────────

async fn handle_inject_ssh_key(pubkey: &str) -> Result<()> {
    if pubkey.is_empty() {
        return Err(anyhow!("Empty public key"));
    }

    let ssh_dir = std::path::Path::new("/home/nilbox/.ssh");
    let authorized_keys = ssh_dir.join("authorized_keys");

    std::fs::create_dir_all(ssh_dir)
        .map_err(|e| anyhow!("Failed to create /home/nilbox/.ssh: {}", e))?;

    #[cfg(target_os = "linux")]
    {
        let (uid, gid) = resolve_user_uid_gid("nilbox")
            .map_err(|e| anyhow!("Failed to resolve nilbox uid/gid: {}", e))?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(ssh_dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| anyhow!("Failed to set .ssh permissions: {}", e))?;
        chown_path(ssh_dir, uid, gid)?;
    }

    std::fs::write(&authorized_keys, format!("{}\n", pubkey.trim()))
        .map_err(|e| anyhow!("Failed to write authorized_keys: {}", e))?;

    #[cfg(target_os = "linux")]
    {
        let (uid, gid) = resolve_user_uid_gid("nilbox")
            .map_err(|e| anyhow!("Failed to resolve nilbox uid/gid: {}", e))?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&authorized_keys, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| anyhow!("Failed to set authorized_keys permissions: {}", e))?;
        chown_path(&authorized_keys, uid, gid)?;
    }

    debug!("SSH public key injected to {:?}", authorized_keys);
    Ok(())
}

// ── Utility functions (used by file_ops too) ────────────────────────────

#[cfg(target_os = "linux")]
pub fn resolve_user_uid_gid(username: &str) -> Result<(u32, u32)> {
    use std::ffi::CString;
    let name = CString::new(username).map_err(|e| anyhow!("{}", e))?;
    let pw = unsafe { libc::getpwnam(name.as_ptr()) };
    if pw.is_null() {
        return Err(anyhow!("User '{}' not found", username));
    }
    let uid = unsafe { (*pw).pw_uid };
    let gid = unsafe { (*pw).pw_gid };
    Ok((uid, gid))
}

#[cfg(target_os = "linux")]
pub fn chown_path(path: &std::path::Path, uid: u32, gid: u32) -> Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|e| anyhow!("Invalid path: {}", e))?;
    let ret = unsafe { libc::chown(c_path.as_ptr(), uid, gid) };
    if ret != 0 {
        return Err(anyhow!("chown {:?} failed: errno={}", path, unsafe { *libc::__errno_location() }));
    }
    Ok(())
}
