//! App installation handler — Zero Token in VM
//!
//! Fetches manifest + taskfile via outbound proxy, verifies SHA256,
//! and runs `task install` with stdout/stderr streaming over VSOCK.

use crate::vsock::stream::VirtualStream;
use crate::vsock::VsockStream;
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};
use base64::Engine as _;
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use tracing::{debug, error, warn};

const PROXY_BASE: &str = "http://127.0.0.1:18088";
const STORE_BASE_URL: &str = "https://store.nilbox.run";

// ── Encryption keys (must match nilbox-core/src/store/keys.rs) ───────────────

const NILBOX_ENC_KEY: [u8; 32] = [
    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08,
    0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10,
    0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18,
    0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f, 0x20,
];

#[cfg(feature = "dev-store")]
const DEV_ENC_KEY: [u8; 32] = [
    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08,
    0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10,
    0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18,
    0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f, 0x20,
];

fn get_enc_key(key_id: &str) -> Result<[u8; 32]> {
    match key_id {
        "nilbox-enc-2026" => Ok(NILBOX_ENC_KEY),
        #[cfg(feature = "dev-store")]
        "nilbox-enc-dev" => Ok(DEV_ENC_KEY),
        _ => Err(anyhow!("Unknown enc key id: {}", key_id)),
    }
}

/// Decrypt a v3 envelope, returning the inner v2 envelope JSON Value.
fn decrypt_v3(raw: &serde_json::Value) -> Result<serde_json::Value> {
    let key_id = raw["key_id"].as_str()
        .ok_or_else(|| anyhow!("Missing key_id in v3 envelope"))?;
    let nonce_b64 = raw["nonce"].as_str()
        .ok_or_else(|| anyhow!("Missing nonce in v3 envelope"))?;
    let ciphertext_b64 = raw["ciphertext"].as_str()
        .ok_or_else(|| anyhow!("Missing ciphertext in v3 envelope"))?;

    let nonce_bytes = base64::engine::general_purpose::STANDARD
        .decode(nonce_b64)
        .map_err(|e| anyhow!("Invalid base64 in v3 nonce: {}", e))?;
    let ciphertext = base64::engine::general_purpose::STANDARD
        .decode(ciphertext_b64)
        .map_err(|e| anyhow!("Invalid base64 in v3 ciphertext: {}", e))?;

    let key_bytes = get_enc_key(key_id)?;
    let cipher = Aes256Gcm::new_from_slice(&key_bytes)
        .map_err(|e| anyhow!("Cipher init failed: {}", e))?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let plaintext = cipher.decrypt(nonce, ciphertext.as_ref())
        .map_err(|_| anyhow!("Manifest decryption failed — ciphertext tampered or wrong key"))?;

    serde_json::from_slice(&plaintext)
        .map_err(|e| anyhow!("Decrypted v3 content is not valid JSON: {}", e))
}

/// Optional verification config passed from the host with the install command.
/// When present, the VM agent POSTs the install result to the store callback URL.
#[derive(Debug, Clone)]
pub struct VerifyConfig {
    pub verify_token: String,
    pub callback_url: String,
}

#[derive(Debug, Deserialize)]
pub struct SignedManifest {
    pub manifest_sha256: String,
    pub manifest: AppManifest,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppManifest {
    pub id: String,
    #[serde(rename = "type")]
    pub manifest_type: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    pub taskfile_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub taskfile_sha256: Option<String>,
    /// Minimum required free disk space in MB (VM partition check)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_disk: Option<u64>,
    /// Flattened extra fields for canonical hashing
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct OutputLine {
    uuid: String,
    line: String,
    is_stderr: bool,
}

#[derive(Debug, Serialize)]
struct DoneMessage {
    uuid: String,
    success: bool,
    exit_code: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// Check that the filesystem containing `path` has at least `min_mb` MB free.
#[cfg(target_os = "linux")]
fn check_free_disk_space(path: &str, min_mb: u64) -> Result<()> {
    use std::ffi::CString;
    let c_path = CString::new(path).map_err(|e| anyhow!("Invalid path: {}", e))?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let ret = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
    if ret != 0 {
        return Err(anyhow!("statvfs({}) failed: errno {}", path, std::io::Error::last_os_error()));
    }
    let block_size = if stat.f_frsize > 0 { stat.f_frsize } else { stat.f_bsize };
    let free_mb = (stat.f_bavail as u64 * block_size as u64) / (1024 * 1024);
    if free_mb < min_mb {
        return Err(anyhow!(
            "Insufficient disk space: {} MB required, {} MB available",
            min_mb, free_mb
        ));
    }
    debug!("VM disk check passed: {} MB free >= {} MB required", free_mb, min_mb);
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn check_free_disk_space(_path: &str, _min_mb: u64) -> Result<()> {
    Ok(())
}

/// Rewrite https:// URLs to http:// so requests go through the outbound proxy
/// as plain HTTP. The host proxy handles actual TLS to the upstream server.
pub fn to_proxy_url(url: &str) -> String {
    if url.starts_with("https://") {
        format!("http://{}", &url[8..])
    } else {
        url.to_string()
    }
}

/// Build an HTTP client configured to use the outbound proxy.
pub fn proxy_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(PROXY_BASE)?)
        .build()
        .map_err(|e| anyhow!("Failed to build HTTP client: {}", e))
}

/// Fetch manifest from store via outbound proxy.
/// Returns raw JSON value (for SHA256 verification) and typed struct (for field access).
pub async fn fetch_manifest(manifest_url: &str) -> Result<(serde_json::Value, SignedManifest)> {
    let client = proxy_client()?;
    let resp = client
        .get(to_proxy_url(manifest_url))
        .send()
        .await
        .map_err(|e| anyhow!("Failed to fetch manifest: {}", e))?;

    if !resp.status().is_success() {
        return Err(anyhow!("Manifest fetch failed with status {}", resp.status()));
    }

    let text = resp.text().await
        .map_err(|e| anyhow!("Failed to read manifest body: {}", e))?;
    let raw: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| anyhow!("Failed to parse manifest JSON: {}", e))?;

    // Unwrap v3 (encrypted) → v2, then extract payload
    let envelope = if raw.get("version").and_then(|v| v.as_u64()) == Some(3) {
        decrypt_v3(&raw)?
    } else {
        raw.clone()
    };

    let (manifest_sha256, manifest_value) = if envelope.get("version").and_then(|v| v.as_u64()) == Some(2) {
        let payload = envelope.get("signed_payload")
            .ok_or_else(|| anyhow!("Missing signed_payload in v2 envelope"))?;
        let sha256 = payload["manifest_sha256"].as_str()
            .ok_or_else(|| anyhow!("Missing manifest_sha256 in signed_payload"))?
            .to_string();
        (sha256, payload["manifest"].clone())
    } else {
        let sha256 = envelope["manifest_sha256"].as_str()
            .ok_or_else(|| anyhow!("Missing manifest_sha256"))?
            .to_string();
        (sha256, envelope["manifest"].clone())
    };

    let manifest: AppManifest = serde_json::from_value(manifest_value.clone())
        .map_err(|e| anyhow!("Failed to parse manifest fields: {}", e))?;
    let signed = SignedManifest { manifest_sha256, manifest };

    Ok((manifest_value, signed))
}

/// Verify manifest_sha256 against canonical JSON of the manifest body.
/// Canonical: sorted keys, compact separators, excluding `taskfile_content` field.
/// Uses raw serde_json::Value to preserve null fields that would be dropped
/// by skip_serializing_if on typed structs.
pub fn verify_manifest_sha256(manifest_value: &serde_json::Value, expected: &str) -> Result<()> {
    let mut value = manifest_value.clone();

    // Remove taskfile_content if present
    if let Some(obj) = value.as_object_mut() {
        obj.remove("taskfile_content");
    }

    // serde_json with BTreeMap produces sorted keys; to_string is compact
    let canonical = canonical_json(&value);
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    let hash = format!("{:x}", hasher.finalize());

    if hash != expected {
        return Err(anyhow!(
            "Manifest SHA256 mismatch: expected {}, got {}",
            expected, hash
        ));
    }

    debug!("Manifest SHA256 verified: {}", &hash[..16]);
    Ok(())
}

/// Produce canonical JSON: sorted keys, compact separators.
fn canonical_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by_key(|(k, _)| *k);
            let inner: Vec<String> = entries
                .into_iter()
                .map(|(k, v)| format!("{}:{}", serde_json::to_string(k).unwrap(), canonical_json(v)))
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        serde_json::Value::Array(arr) => {
            let inner: Vec<String> = arr.iter().map(canonical_json).collect();
            format!("[{}]", inner.join(","))
        }
        _ => serde_json::to_string(value).unwrap(),
    }
}

/// Fetch taskfile content and optionally verify its SHA256.
/// If `expected_sha256` is None, verification is skipped.
pub async fn fetch_and_verify_taskfile(url: &str, expected_sha256: Option<&str>) -> Result<String> {
    let client = proxy_client()?;
    let resp = client
        .get(to_proxy_url(url))
        .send()
        .await
        .map_err(|e| anyhow!("Failed to fetch taskfile: {}", e))?;

    if !resp.status().is_success() {
        return Err(anyhow!("Taskfile fetch failed with status {}", resp.status()));
    }

    let content = resp.text().await
        .map_err(|e| anyhow!("Failed to read taskfile body: {}", e))?;

    if let Some(expected) = expected_sha256 {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        let hash = format!("{:x}", hasher.finalize());

        if hash != expected {
            return Err(anyhow!(
                "Taskfile SHA256 mismatch: expected {}, got {}",
                expected, hash
            ));
        }

        debug!("Taskfile SHA256 verified: {}", &hash[..16]);
    } else {
        debug!("Taskfile fetched (no SHA256 to verify)");
    }

    Ok(content)
}

/// Write taskfile to disk and run `task install`, streaming output lines over VSOCK.
/// Returns (success, exit_code) — DoneMessage is sent by the caller.
async fn run_task_install(uuid: &str, taskfile_content: &str, stream: &mut VirtualStream) -> Result<(bool, i32)> {
    let work_dir = format!("/tmp/nilbox-app-{}", uuid);
    std::fs::create_dir_all(&work_dir)
        .map_err(|e| anyhow!("Failed to create work dir: {}", e))?;

    let taskfile_path = format!("{}/Taskfile.yml", work_dir);
    std::fs::write(&taskfile_path, taskfile_content)
        .map_err(|e| anyhow!("Failed to write Taskfile.yml: {}", e))?;

    debug!("Running `task install` in {}", work_dir);

    let mut child = Command::new("task")
        .arg("install")
        .current_dir(&work_dir)
        .env("PATH", "/usr/local/bin:/usr/bin:/bin:/usr/local/sbin:/usr/sbin:/sbin")
        .env("NODE_EXTRA_CA_CERTS", "/usr/local/share/ca-certificates/nilbox-inspect.crt")
        .env("NODE_TLS_REJECT_UNAUTHORIZED", "1")
        .env("SSL_CERT_FILE", "/etc/ssl/certs/ca-certificates.crt")
        .env("CURL_CA_BUNDLE", "/etc/ssl/certs/ca-certificates.crt")
        .env("REQUESTS_CA_BUNDLE", "/etc/ssl/certs/ca-certificates.crt")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow!("Failed to spawn `task install`: {}", e))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let uuid_owned = uuid.to_string();

    // Read stdout and stderr in blocking threads, send lines to channel
    let (line_tx, mut line_rx) = tokio::sync::mpsc::channel::<OutputLine>(256);

    if let Some(stdout) = stdout {
        let tx = line_tx.clone();
        let uid = uuid_owned.clone();
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                if let Ok(line) = line {
                    let _ = tx.blocking_send(OutputLine {
                        uuid: uid.clone(),
                        line,
                        is_stderr: false,
                    });
                }
            }
        });
    }

    if let Some(stderr) = stderr {
        let tx = line_tx.clone();
        let uid = uuid_owned.clone();
        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                if let Ok(line) = line {
                    let _ = tx.blocking_send(OutputLine {
                        uuid: uid.clone(),
                        line,
                        is_stderr: true,
                    });
                }
            }
        });
    }

    // Drop our copy so channel closes when both threads finish
    drop(line_tx);

    // Stream lines to host
    while let Some(output) = line_rx.recv().await {
        let json = serde_json::to_string(&output).unwrap();
        if let Err(e) = stream.write(json.as_bytes()).await {
            error!("Failed to write output line: {}", e);
            break;
        }
    }

    // Wait for process exit
    let status = tokio::task::spawn_blocking(move || child.wait())
        .await
        .map_err(|e| anyhow!("Join error: {}", e))?
        .map_err(|e| anyhow!("Wait error: {}", e))?;

    let exit_code = status.code().unwrap_or(-1);
    debug!("`task install` exited with code {}", exit_code);

    // Re-apply npm cafile after install — npm may have been installed by the taskfile,
    // so update_ca_certificates (which runs at VM init) might have missed it.
    let _ = tokio::process::Command::new("npm")
        .args(["config", "set", "--global", "cafile", "/etc/ssl/certs/ca-certificates.crt"])
        .output()
        .await;

    Ok((status.success(), exit_code))
}

/// Collect VM metadata for the verify callback.
fn collect_vm_info() -> serde_json::Value {
    let arch = std::env::consts::ARCH;
    let nilbox_version = env!("CARGO_PKG_VERSION");

    // Try to read OS name from /etc/os-release
    let os = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|content| {
            content
                .lines()
                .find(|l| l.starts_with("ID="))
                .map(|l| l.trim_start_matches("ID=").trim_matches('"').to_string())
        })
        .unwrap_or_else(|| "linux".to_string());

    serde_json::json!({
        "os": os,
        "arch": arch,
        "nilbox_version": nilbox_version,
    })
}

/// POST install result to store callback URL (best-effort, via outbound proxy).
async fn send_verify_callback(
    config: &VerifyConfig,
    success: bool,
    error_message: Option<&str>,
) {
    let client = match proxy_client() {
        Ok(c) => c,
        Err(e) => {
            warn!("Failed to build HTTP client for verify callback: {}", e);
            return;
        }
    };

    let body = serde_json::json!({
        "verify_token": config.verify_token,
        "result": if success { "success" } else { "failure" },
        "error_message": error_message,
        "vm_info": collect_vm_info(),
    });

    match tokio::time::timeout(
        std::time::Duration::from_secs(30),
        client
            .post(to_proxy_url(&config.callback_url))
            .header("Authorization", format!("Bearer {}", config.verify_token))
            .json(&body)
            .send(),
    )
    .await
    {
        Ok(Ok(resp)) => {
            debug!(
                "Verify callback sent: status={}, url={}",
                resp.status(),
                config.callback_url
            );
        }
        Ok(Err(e)) => {
            warn!("Verify callback request failed: {}", e);
        }
        Err(_) => {
            warn!("Verify callback timed out after 30s");
        }
    }
}

/// Inner install orchestration: fetch → verify → execute.
/// Returns (success, exit_code) on task completion, or Err on pre-task failure.
async fn do_install(uuid: &str, id: &str, stream: &mut VirtualStream) -> Result<(bool, i32)> {
    let manifest_url = format!("{}/apps/{}/manifest", STORE_BASE_URL, id);

    debug!("Fetching manifest for app '{}' from {}", id, manifest_url);
    let (manifest_value, signed) = fetch_manifest(&manifest_url).await?;

    verify_manifest_sha256(&manifest_value, &signed.manifest_sha256)?;

    if signed.manifest.manifest_type != "application" {
        return Err(anyhow!(
            "Invalid manifest type: expected 'application', got '{}'",
            signed.manifest.manifest_type
        ));
    }

    if let Some(min_mb) = signed.manifest.min_disk {
        if min_mb > 0 {
            check_free_disk_space("/tmp", min_mb)?;
        }
    }

    let taskfile_content = fetch_and_verify_taskfile(
        &signed.manifest.taskfile_url,
        signed.manifest.taskfile_sha256.as_deref(),
    )
    .await?;

    run_task_install(uuid, &taskfile_content, stream).await
}

/// Full install orchestration with DoneMessage + verify callback.
///
/// Always sends a DoneMessage over the VSOCK stream before returning.
/// If `verify_config` is set, POSTs the install result to the store callback URL
/// after the DoneMessage (best-effort, 30s timeout).
pub async fn handle_install_app(
    uuid: &str,
    id: &str,
    verify_config: Option<VerifyConfig>,
    stream: &mut VirtualStream,
) -> Result<()> {
    let result = do_install(uuid, id, stream).await;

    let (success, exit_code, error_message) = match &result {
        Ok((s, c)) => (*s, *c, None),
        Err(e) => (false, -1, Some(e.to_string())),
    };

    // Always send DoneMessage
    let done = DoneMessage {
        uuid: uuid.to_string(),
        success,
        exit_code,
        error: error_message.clone(),
    };
    let json = serde_json::to_string(&done).unwrap();
    let _ = stream.write(json.as_bytes()).await;

    // Send verify callback (best-effort)
    if let Some(ref vc) = verify_config {
        send_verify_callback(vc, success, error_message.as_deref()).await;
    }

    result.map(|_| ())
}
