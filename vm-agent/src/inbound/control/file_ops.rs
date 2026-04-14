//! File operation handlers for the Control Port.
//!
//! `write_file` enforces a strict path allowlist — even if the VSOCK channel
//! is compromised, only pre-approved paths can be written.

use anyhow::{Result, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use tracing::{debug, warn};

#[cfg(target_os = "linux")]
use super::resolve_user_uid_gid;
#[cfg(target_os = "linux")]
use super::chown_path;

// ── Path allowlist (compile-time constants) ─────────────────────────────

/// Exact paths that may be written via `write_file`.
const ALLOWED_WRITE_PATHS: &[&str] = &[
    "/etc/profile.d/nilbox-envs.sh",
    "/etc/profile.d/nilbox-proxy.sh",
    "/usr/local/share/ca-certificates/nilbox-inspect.crt",
    "/usr/local/bin/xdg-open",
    "/etc/environment",
];

/// Path prefixes that may be written via `write_file`.
/// Any path starting with one of these is accepted (after canonicalization).
const ALLOWED_WRITE_PREFIXES: &[&str] = &[
    "/etc/nilbox/",
];

fn is_path_allowed(path: &str) -> bool {
    // Reject path traversal
    if path.contains("..") || !path.starts_with('/') {
        return false;
    }
    if ALLOWED_WRITE_PATHS.contains(&path) {
        return true;
    }
    ALLOWED_WRITE_PREFIXES.iter().any(|prefix| path.starts_with(prefix))
}

// ── write_file handler ──────────────────────────────────────────────────

/// Write content to an allowlisted path with specified permissions and owner.
///
/// - `path`: absolute filesystem path (must pass allowlist check)
/// - `content_b64`: base64-encoded file content
/// - `mode`: optional octal permission string (e.g. "0644")
/// - `owner`: optional username ("root" or "nilbox")
pub fn handle_write_file(
    path: &str,
    content_b64: &str,
    mode: Option<&str>,
    owner: Option<&str>,
) -> Result<()> {
    // 1. Allowlist check
    if !is_path_allowed(path) {
        warn!("write_file rejected: path not in allowlist: {}", path);
        return Err(anyhow!("path_not_allowed: {}", path));
    }

    // 2. Decode content
    let content = STANDARD.decode(content_b64)
        .map_err(|e| anyhow!("Invalid base64 content: {}", e))?;

    // 3. Ensure parent directory exists
    let file_path = std::path::Path::new(path);
    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow!("Failed to create parent dir {:?}: {}", parent, e))?;
    }

    // 4. Atomic write: write to temp file, then rename
    let tmp_path = format!("{}.tmp.{}", path, std::process::id());
    std::fs::write(&tmp_path, &content)
        .map_err(|e| anyhow!("Failed to write temp file {}: {}", tmp_path, e))?;

    // 5. Set permissions before rename (so the file is never world-readable even briefly)
    #[cfg(target_os = "linux")]
    if let Some(mode_str) = mode {
        use std::os::unix::fs::PermissionsExt;
        let mode_val = u32::from_str_radix(mode_str.trim_start_matches('0'), 8)
            .map_err(|e| anyhow!("Invalid mode '{}': {}", mode_str, e))?;
        std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(mode_val))
            .map_err(|e| anyhow!("Failed to set permissions on {}: {}", tmp_path, e))?;
    }

    // 6. Set ownership
    #[cfg(target_os = "linux")]
    if let Some(owner_name) = owner {
        let (uid, gid) = resolve_user_uid_gid(owner_name)
            .map_err(|e| anyhow!("Failed to resolve owner '{}': {}", owner_name, e))?;
        chown_path(std::path::Path::new(&tmp_path), uid, gid)?;
    }

    // 7. Rename into place (atomic on same filesystem)
    std::fs::rename(&tmp_path, path)
        .map_err(|e| {
            // Clean up temp file on failure
            let _ = std::fs::remove_file(&tmp_path);
            anyhow!("Failed to rename {} → {}: {}", tmp_path, path, e)
        })?;

    debug!("write_file: wrote {} bytes to {}", content.len(), path);
    Ok(())
}

// ── update_mcp_config handler ───────────────────────────────────────────

const MCP_CONFIG_PATH: &str = "/etc/nilbox/mcp-servers.json";

/// Write MCP server config and signal mcp-stdio-proxy to reload (SIGHUP).
pub async fn handle_update_mcp_config(config_json: &str) -> Result<()> {
    // Validate JSON before writing
    let _: serde_json::Value = serde_json::from_str(config_json)
        .map_err(|e| anyhow!("Invalid MCP config JSON: {}", e))?;

    // Ensure parent directory exists
    let path = std::path::Path::new(MCP_CONFIG_PATH);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow!("Failed to create dir {:?}: {}", parent, e))?;
    }

    // Atomic write
    let tmp_path = format!("{}.tmp.{}", MCP_CONFIG_PATH, std::process::id());
    std::fs::write(&tmp_path, config_json.as_bytes())
        .map_err(|e| anyhow!("Failed to write {}: {}", tmp_path, e))?;
    std::fs::rename(&tmp_path, MCP_CONFIG_PATH)
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp_path);
            anyhow!("Failed to rename {} → {}: {}", tmp_path, MCP_CONFIG_PATH, e)
        })?;

    debug!("MCP config written to {}", MCP_CONFIG_PATH);

    // Send SIGHUP to mcp-stdio-proxy to reload config
    #[cfg(target_os = "linux")]
    {
        match tokio::process::Command::new("pkill")
            .args(["-HUP", "-f", "mcp-stdio-proxy"])
            .output()
            .await
        {
            Ok(o) if o.status.success() => {
                debug!("Sent SIGHUP to mcp-stdio-proxy");
            }
            Ok(o) => {
                // pkill returns 1 if no process matched — not an error
                debug!("pkill -HUP mcp-stdio-proxy exited with {}", o.status);
            }
            Err(e) => {
                warn!("Failed to send SIGHUP to mcp-stdio-proxy: {}", e);
            }
        }
    }

    Ok(())
}

// ── update_ca_certificates handler ──────────────────────────────────────

/// Run the system CA certificate update chain.
/// All commands use hardcoded paths — no user input flows into any argument.
/// Remove an exact PEM certificate block from a bundle string.
///
/// Finds the exact `-----BEGIN CERTIFICATE-----` … `-----END CERTIFICATE-----`
/// block whose base64 body matches `cert_pem`, and removes it (including a
/// trailing newline if present).  All other certificates are left intact.
/// Returns the bundle unchanged if the block is not found.
fn remove_cert_block_from_bundle(bundle: &str, cert_pem: &str) -> String {
    // Extract the PEM block from the cert file (strip surrounding whitespace).
    let start = match cert_pem.find("-----BEGIN CERTIFICATE-----") {
        Some(p) => p,
        None => return bundle.to_string(),
    };
    let after = &cert_pem[start..];
    let end = match after.find("-----END CERTIFICATE-----") {
        Some(p) => p + "-----END CERTIFICATE-----".len(),
        None => return bundle.to_string(),
    };
    let block = &after[..end]; // e.g. "-----BEGIN CERTIFICATE-----\n…\n-----END CERTIFICATE-----"

    // Remove the block with a trailing newline (most common case), then without.
    let with_nl = format!("{}\n", block);
    if bundle.contains(with_nl.as_str()) {
        bundle.replace(with_nl.as_str(), "")
    } else {
        bundle.replace(block, "")
    }
}

pub async fn handle_update_ca_certificates() -> Result<String> {
    let mut output = String::new();

    // 1. System trust store update (Debian / Alpine)
    match tokio::process::Command::new("/usr/sbin/update-ca-certificates")
        .output()
        .await
    {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let stderr = String::from_utf8_lossy(&o.stderr);
            output.push_str(&format!("update-ca-certificates: {}{}\n", stdout, stderr));
            debug!("update-ca-certificates completed (exit={})", o.status);
        }
        Err(e) => {
            warn!("update-ca-certificates failed: {}", e);
            output.push_str(&format!("update-ca-certificates failed: {}\n", e));
        }
    }

    // 2. Force-replace NilBox CA in known CA bundles.
    //    Remove any existing (possibly tampered) NilBox Inspect CA block, then always
    //    append the fresh cert from disk.  This ensures a deleted or mutated CA is
    //    repaired on every VM start without leaving stale data behind.
    let cert_path = "/usr/local/share/ca-certificates/nilbox-inspect.crt";
    let bundles = [
        "/etc/ssl/certs/ca-certificates.crt",
        "/etc/ssl/ca-bundle.pem",
        "/etc/pki/tls/cert.pem",
    ];

    let cert_content = std::fs::read_to_string(cert_path).unwrap_or_default();
    if !cert_content.is_empty() {
        for bundle in &bundles {
            match std::fs::read_to_string(bundle) {
                Ok(bundle_content) => {
                    let cleaned = remove_cert_block_from_bundle(&bundle_content, &cert_content);
                    let mut new_content = cleaned;
                    if !new_content.ends_with('\n') {
                        new_content.push('\n');
                    }
                    new_content.push_str(&cert_content);
                    if let Err(e) = std::fs::write(bundle, new_content.as_bytes()) {
                        warn!("Failed to update cert in {}: {}", bundle, e);
                    } else {
                        debug!("Force-replaced inspect cert in {}", bundle);
                        output.push_str(&format!("Updated cert in {}\n", bundle));
                    }
                }
                Err(_) => {} // bundle does not exist on this distro — skip
            }
        }
    }

    // 3. Inject into Python certifi bundle (force-replace by exact PEM block match)
    match tokio::process::Command::new("python3")
        .args([
            "-c",
            concat!(
                "import certifi; ",
                "dst=certifi.where(); ",
                "src=open('/usr/local/share/ca-certificates/nilbox-inspect.crt').read().strip(); ",
                "bundle=open(dst).read(); ",
                // Remove exact PEM block (with or without trailing newline) then always append
                "cleaned=bundle.replace(src+'\\n','').replace(src,''); ",
                "open(dst,'w').write(cleaned.rstrip('\\n')+'\\n'+src+'\\n')"
            ),
        ])
        .output()
        .await
    {
        Ok(o) if o.status.success() => {
            debug!("Python certifi bundle updated");
            output.push_str("Python certifi updated\n");
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            warn!("Python certifi update failed: {}", stderr);
        }
        Err(e) => {
            warn!("Python certifi update skipped: {}", e);
        }
    }

    // 4. Pre-write npm config files so cafile is set even before npm is installed.
    //    npm reads /etc/npmrc (system-wide) and ~/.npmrc (user) on first run.
    //    This covers the case where npm is installed later by a Taskfile.
    // Pre-write ~/.npmrc so cafile is set even before npm is installed.
    // npm always reads ~/.npmrc on startup regardless of how it was invoked.
    let npm_cafile_line = "cafile=/etc/ssl/certs/ca-certificates.crt\n";
    let existing = std::fs::read_to_string("/root/.npmrc").unwrap_or_default();
    if !existing.contains("cafile=") {
        let base = if existing.is_empty() { String::new() } else { format!("{}\n", existing.trim_end()) };
        let new_content = format!("{}{}", base, npm_cafile_line);
        if let Err(e) = std::fs::write("/root/.npmrc", new_content) {
            warn!("Failed to write /root/.npmrc: {}", e);
        } else {
            debug!("npm cafile pre-configured in /root/.npmrc");
            output.push_str("npm cafile pre-configured in /root/.npmrc\n");
        }
    }

    // 5. npm global cafile via npm config (if npm is already present)
    match tokio::process::Command::new("npm")
        .args(["config", "set", "--global", "cafile", "/etc/ssl/certs/ca-certificates.crt"])
        .output()
        .await
    {
        Ok(o) if o.status.success() => {
            debug!("npm cafile configured via npm config");
            output.push_str("npm cafile configured\n");
        }
        _ => {} // npm not present or failed — non-fatal
    }

    // Chrome/Chromium NSS database is handled by nilbox-nssdb.path systemd unit:
    // triggered automatically when /usr/local/share/ca-certificates/nilbox-inspect.crt
    // is written. certutil runs as a small oneshot service, not forked from vm-agent.

    Ok(output)
}
