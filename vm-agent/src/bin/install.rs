//! nilbox-install — interactive app installer for NilBox VM
//!
//! Usage: nilbox-install <base64-manifest-url>
//!
//! Decodes the manifest URL, fetches + verifies the manifest and taskfile,
//! then runs `task install` with inherited stdio (PTY) so the user gets a
//! real interactive terminal. Exit code mirrors `task`'s exit code.
//!
//! Invoked by the SSH shell wrapper:
//!   /bin/sh -c 'nilbox-install URL_B64 && exec /bin/sh || exec /bin/sh'

use anyhow::{anyhow, Context, Result};
use base64::{Engine as _, engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD}};
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::process;

const PROXY_BASE: &str = "http://127.0.0.1:18088";

// ── Store keys (must match nilbox-core/src/store/keys.rs) ────────────────────

/// Production Ed25519 public key — matches NILBOX_STORE_PUB_KEYID in nilbox-core.
const NILBOX_STORE_PUB_KEYID: [u8; 32] = [
    0x05, 0x46, 0xd5, 0x92, 0x95, 0x63, 0x09, 0xc3,
    0x1f, 0x6a, 0x38, 0x7f, 0x6d, 0xac, 0x81, 0x04,
    0xe1, 0xcb, 0x79, 0x9a, 0x40, 0x1a, 0xfa, 0x7e,
    0x57, 0x70, 0x5c, 0xf6, 0xcb, 0xce, 0xb8, 0x89
];

#[cfg(feature = "dev-store")]
const DEV_STORE_KEY: [u8; 32] = [
    0x3b, 0x6a, 0x27, 0xbc, 0xce, 0xb6, 0xa4, 0x2d,
    0x62, 0xa3, 0xa8, 0xd0, 0x2a, 0x6f, 0x0d, 0x73,
    0x65, 0x32, 0x15, 0x77, 0x1d, 0xe2, 0x43, 0xa6,
    0x3a, 0xc0, 0x48, 0xa1, 0x8b, 0x59, 0xda, 0x29,
];

fn get_store_public_key(key_id: &str) -> Result<VerifyingKey> {
    let bytes: &[u8; 32] = match key_id {
        "nilbox-store-2026" => &NILBOX_STORE_PUB_KEYID,
        #[cfg(feature = "dev-store")]
        "nilbox-store-dev" => &DEV_STORE_KEY,
        _ => return Err(anyhow!("Unknown store public key id: {}", key_id)),
    };
    VerifyingKey::from_bytes(bytes)
        .map_err(|e| anyhow!("Invalid store public key for '{}': {}", key_id, e))
}

// ── Encryption keys (must match nilbox-core/src/store/keys.rs) ───────────────

/// Production encryption key — matches STORE_ENC_KEY in nilbox-store and NILBOX_ENC_KEY in nilbox-core.
const NILBOX_ENC_KEY: [u8; 32] = [
    0xc7, 0xc4, 0xac, 0xba, 0x95, 0x2b, 0x33, 0x21,
    0x20, 0x30, 0xa9, 0x44, 0xcb, 0x6d, 0xf0, 0xff,
    0xd4, 0x54, 0x81, 0x83, 0x18, 0x31, 0x20, 0xe1,
    0x83, 0x29, 0x81, 0x8d, 0x5c, 0x0b, 0xc6, 0x2f
];

/// Dev encryption key — matches DEV_ENC_KEY in nilbox-core.
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

    let nonce_bytes = B64.decode(nonce_b64).context("Invalid base64 in v3 nonce")?;
    let ciphertext = B64.decode(ciphertext_b64).context("Invalid base64 in v3 ciphertext")?;

    let key_bytes = get_enc_key(key_id)?;
    let cipher = Aes256Gcm::new_from_slice(&key_bytes)
        .map_err(|e| anyhow!("Cipher init failed: {}", e))?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let plaintext = cipher.decrypt(nonce, ciphertext.as_ref())
        .map_err(|_| anyhow!("Manifest decryption failed — ciphertext tampered or wrong key"))?;

    serde_json::from_slice(&plaintext).context("Decrypted v3 content is not valid JSON")
}

// ── Manifest types ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct SignedManifest {
    manifest_sha256: String,
    manifest: AppManifest,
}

#[derive(Debug, Deserialize, Serialize)]
struct AppManifest {
    #[serde(rename = "type")]
    manifest_type: String,
    #[serde(default)]
    version: String,
    taskfile_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    taskfile_sha256: Option<String>,
    #[serde(flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Rewrite https:// → http:// so requests go through the outbound proxy.
fn to_proxy_url(url: &str) -> String {
    if url.starts_with("https://") {
        format!("http://{}", &url[8..])
    } else {
        url.to_string()
    }
}

fn proxy_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(PROXY_BASE)?)
        .build()
        .map_err(|e| anyhow!("Failed to build HTTP client: {}", e))
}

async fn fetch_manifest(manifest_url: &str) -> Result<(serde_json::Value, SignedManifest)> {
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

    // Only v3 (encrypted) manifests are accepted — v1/v2 are rejected for security
    let version = raw.get("version").and_then(|v| v.as_u64());
    if version != Some(3) {
        return Err(anyhow!(
            "Manifest version {} is not supported. Only encrypted v3 manifests are allowed.",
            version.map(|v| v.to_string()).unwrap_or_else(|| "unknown".to_string())
        ));
    }
    let envelope = decrypt_v3(&raw)?;

    // Ed25519 signature verification
    let key_id = envelope["store_public_key_id"].as_str()
        .ok_or_else(|| anyhow!("Missing store_public_key_id in envelope"))?;
    let sig_b64 = envelope["store_signature"].as_str()
        .ok_or_else(|| anyhow!("Missing store_signature in envelope"))?;
    let sig_bytes = B64.decode(sig_b64).context("Invalid base64 in store_signature")?;
    let signature = Signature::from_slice(&sig_bytes)
        .map_err(|e| anyhow!("Invalid Ed25519 signature format: {}", e))?;
    let verifying_key = get_store_public_key(key_id)?;

    let payload = envelope.get("signed_payload")
        .ok_or_else(|| anyhow!("Invalid decrypted envelope: missing signed_payload"))?;

    // Canonical JSON of signed_payload for verification (must match nilbox-core)
    let canonical_payload = canonical_json(payload);
    verifying_key.verify(canonical_payload.as_bytes(), &signature)
        .map_err(|e| anyhow!("Manifest signature verification failed: {}. Installation blocked.", e))?;

    let manifest_sha256 = payload["manifest_sha256"].as_str()
        .ok_or_else(|| anyhow!("Missing manifest_sha256 in signed_payload"))?
        .to_string();
    let manifest_value = payload["manifest"].clone();

    let manifest: AppManifest = serde_json::from_value(manifest_value.clone())
        .map_err(|e| anyhow!("Failed to parse manifest fields: {}", e))?;

    Ok((manifest_value, SignedManifest { manifest_sha256, manifest }))
}

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

fn verify_manifest_sha256(manifest_value: &serde_json::Value, expected: &str) -> Result<()> {
    let mut value = manifest_value.clone();

    if let Some(obj) = value.as_object_mut() {
        obj.remove("taskfile_content");
    }

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
    Ok(())
}

async fn fetch_and_verify_taskfile(url: &str, expected_sha256: &str) -> Result<String> {
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

    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let hash = format!("{:x}", hasher.finalize());

    if hash != expected_sha256 {
        return Err(anyhow!(
            "Taskfile SHA256 mismatch: expected {}, got {}\nThis may be caused by a stale store cache. Please try installing again later.",
            expected_sha256, hash
        ));
    }

    Ok(content)
}

/// Simple UUID v4 from /dev/urandom.
fn uuid_v4() -> String {
    let mut bytes = [0u8; 16];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = f.read_exact(&mut bytes);
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-\
         {:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        bytes[6], bytes[7],
        bytes[8], bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
    )
}

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: nilbox-install <base64-manifest-url>");
        process::exit(1);
    }

    let manifest_url = match URL_SAFE_NO_PAD.decode(&args[1]) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("nilbox-install: invalid UTF-8 in decoded URL: {}", e);
                process::exit(1);
            }
        },
        Err(e) => {
            eprintln!("nilbox-install: failed to decode base64 URL: {}", e);
            process::exit(1);
        }
    };

    let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
    let exit_code = rt.block_on(async {
        match run_install(&manifest_url).await {
            Ok(code) => code,
            Err(e) => {
                eprintln!("nilbox-install: {}", e);
                1
            }
        }
    });

    process::exit(exit_code);
}

async fn run_install(manifest_url: &str) -> Result<i32> {
    // 1. Fetch manifest
    eprintln!("Fetching manifest from {}", manifest_url);
    let (manifest_value, signed) = fetch_manifest(manifest_url).await?;

    // 2. Verify manifest SHA256 (use raw JSON to preserve null fields)
    verify_manifest_sha256(&manifest_value, &signed.manifest_sha256)?;

    // 3. Type guard
    if signed.manifest.manifest_type != "application" {
        return Err(anyhow!(
            "Invalid manifest type: expected 'application', got '{}'",
            signed.manifest.manifest_type
        ));
    }

    // 4. Fetch and verify taskfile (taskfile_sha256 is required for security)
    eprintln!("Fetching taskfile...");
    let taskfile_sha256 = signed.manifest.taskfile_sha256
        .as_deref()
        .ok_or_else(|| anyhow!("taskfile_sha256 is required for security"))?;
    let taskfile_content = fetch_and_verify_taskfile(
        &signed.manifest.taskfile_url,
        taskfile_sha256,
    ).await?;

    // 5. Write taskfile to a temp directory
    let uuid = uuid_v4();
    let work_dir = format!("/tmp/nilbox-app-{}", uuid);
    std::fs::create_dir_all(&work_dir)
        .map_err(|e| anyhow!("Failed to create work dir: {}", e))?;

    std::fs::write(format!("{}/Taskfile.yml", work_dir), &taskfile_content)
        .map_err(|e| anyhow!("Failed to write Taskfile.yml: {}", e))?;

    eprintln!("Running task install...");

    // 6. Run `task install` with inherited stdio (PTY — interactive)
    let status = std::process::Command::new("task")
        .arg("install")
        .current_dir(&work_dir)
        .env("PATH", "/usr/local/bin:/usr/bin:/bin:/usr/local/sbin:/usr/sbin:/sbin")
        .env("NODE_EXTRA_CA_CERTS", "/usr/local/share/ca-certificates/nilbox-inspect.crt")
        .env("NODE_TLS_REJECT_UNAUTHORIZED", "1")
        .env("SSL_CERT_FILE", "/etc/ssl/certs/ca-certificates.crt")
        .env("CURL_CA_BUNDLE", "/etc/ssl/certs/ca-certificates.crt")
        .env("REQUESTS_CA_BUNDLE", "/etc/ssl/certs/ca-certificates.crt")
        .status()
        .map_err(|e| anyhow!("Failed to spawn `task install`: {}", e))?;

    Ok(status.code().unwrap_or(1))
}
