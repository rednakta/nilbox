//! VM install from manifest — download, verify, extract, register
//!
//! Parses the store's existing `/apps/{id}/manifest` JSON format.
//!
//! Flow:
//!   1. Fetch manifest JSON from URL
//!   2. Emit progress events
//!   3. Stream-download source.image_url (.zip) with progress
//!   4. Verify SHA256 checksum (source.sha256, if present)
//!   5. Extract source.disk_image filename from .zip
//!   6. Persist VmRecord to config_store
//!   7. Return VmRecord (caller registers it in memory via service.register_vm)

use crate::config_store::{ConfigStore, VmRecord};
use crate::events::{EventEmitter, emit_typed};
use crate::store::client::StoreClient;

use anyhow::{anyhow, Context, Result};
use ed25519_dalek::{Signature, Verifier};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;
use tracing::{warn, debug, error};
use uuid::Uuid;

// ── Manifest types (store AppManifest format) ─────────────────

/// `source` block of the store manifest.
#[derive(Debug, Clone, Deserialize)]
pub struct ManifestSource {
    /// URL of the .tar.gz disk image archive.
    pub image_url: String,
    /// Disk image format (e.g. "raw").
    pub image_format: Option<String>,
    /// Script to run on first boot.
    pub first_boot_script: Option<String>,
    /// Filename of the image file inside the archive.
    pub disk_image: Option<String>,
    /// Optional SHA256 hex checksum of the .tar.gz archive.
    pub sha256: Option<String>,
    /// Kernel: HTTP URL to download, or local file path.
    pub kernel: Option<String>,
    /// Initrd: HTTP URL to download, or local file path.
    pub initrd: Option<String>,
    /// Optional kernel cmdline passed to VmRecord.
    pub append: Option<String>,
    /// Default RAM in MB (nil → 512).
    pub min_memory: Option<u32>,
    /// Default CPU count (nil → 2).
    pub cpus: Option<u32>,
    /// If true, persist zip + manifest to cache after first download.
    #[serde(default)]
    pub tauri_cacheable: bool,
}

/// `permissions` block of the store manifest.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ManifestPermissions {
    /// Inbound ports the VM listens on (host_port == vm_port).
    #[serde(default)]
    pub inbound_ports: Vec<u16>,
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    #[serde(default)]
    pub tokens: Vec<serde_json::Value>,
    /// Minimum verified level required to install (user | github | admin).
    #[serde(default)]
    pub verified: Option<String>,
}

/// `admin` block of the store manifest.
#[derive(Debug, Clone, Deserialize)]
pub struct AdminConfig {
    pub url: String,
    pub label: Option<String>,
}

/// Deserialize `admin` as a `Vec<AdminConfig>`, accepting null, missing, or an array.
fn deserialize_admin_vec<'de, D>(deserializer: D) -> std::result::Result<Vec<AdminConfig>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt: Option<Vec<AdminConfig>> = Option::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

/// Store AppManifest — parsed from `/apps/{id}/manifest`.
#[derive(Debug, Clone, Deserialize)]
pub struct VmManifest {
    /// Short identifier (unused at install time, kept for logging).
    #[serde(default)]
    pub id: String,
    /// Display name shown in VM list.
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    pub description: Option<String>,
    pub source: ManifestSource,
    #[serde(default)]
    pub permissions: ManifestPermissions,
    #[serde(default, deserialize_with = "deserialize_admin_vec")]
    pub admin: Vec<AdminConfig>,
    #[serde(default)]
    pub base_os: Option<String>,
    #[serde(default)]
    pub base_os_version: Option<String>,
    #[serde(rename = "platform", default)]
    pub target_platform: Option<Vec<String>>,
}

/// Wrapper returned by `/apps/{id}/manifest` endpoint.
#[derive(Debug, Deserialize)]
pub struct ManifestResponse {
    pub manifest_sha256: String,
    pub manifest: VmManifest,
}

// ── Progress event ────────────────────────────────────────────

/// Payload emitted as Tauri event `vm-install-progress`.
#[derive(Debug, Clone, Serialize)]
pub struct VmInstallProgress {
    /// "downloading" | "extracting" | "registering" | "complete" | "error"
    pub stage: String,
    /// 0-100
    pub percent: u8,
    pub vm_name: String,
    /// Present when stage == "complete"
    pub vm_id: Option<String>,
    /// Present when stage == "error"
    pub error: Option<String>,
}

// ── Main install function ─────────────────────────────────────

/// Download, extract, and register a VM from a store manifest URL.
///
/// Emits `vm-install-progress` events throughout.
/// Returns a `VmRecord` with `created_at` set (from DB insert).
/// Caller must call `service.register_vm(&record)` to add the VM to memory.
///
/// If `store_client` is provided, uses signed download URLs and verifies
/// file signatures. Falls back to direct `image_url` if unavailable.
pub async fn install_from_manifest_url(
    manifest_url: &str,
    app_data_dir: &Path,
    emitter: &Arc<dyn EventEmitter>,
    config_store: &Arc<ConfigStore>,
    store_client: Option<&StoreClient>,
    user_verified: Option<String>,
) -> Result<VmRecord> {
    // 1. Fetch manifest (also keep raw text for caching)
    debug!("[install_vm] fetching manifest: {}", manifest_url);
    let (mut manifest, manifest_raw, manifest_json) = fetch_manifest(manifest_url)
        .await
        .map_err(|e| {
            error!("[install_vm] fetch_manifest failed: {:#}", e);
            anyhow!("Unable to retrieve app information. Make sure you are connected to the official store at https://nilbox.run. (detail: {})", e)
        })?;

    // Platform compatibility check
    if let Some(ref platforms) = manifest.target_platform {
        if !platforms.is_empty() {
            let host = host_platform();
            if !platforms.iter().any(|p| p == host || host.starts_with(p)) {
                return Err(anyhow!(
                    "This image is not compatible with your system ({}). Supported platforms: {}",
                    host,
                    platforms.join(", ")
                ));
            }
        }
    }

    // Normalize all source URLs: rewrite any non-store host to https://store.nilbox.run
    manifest.source.image_url = rewrite_to_store(&manifest.source.image_url);
    manifest.source.kernel = manifest.source.kernel.map(|u| rewrite_to_store(&u));
    manifest.source.initrd = manifest.source.initrd.map(|u| rewrite_to_store(&u));

    let vm_id = Uuid::new_v4().to_string();
    let vm_dir = app_data_dir.join("vms").join(&vm_id);
    std::fs::create_dir_all(&vm_dir).context("Failed to create VM directory")?;

    let vm_name = config_store.unique_vm_name(&manifest.name)?;

    // 2. Request signed download URL if store_client available
    let (download_url, expected_sha256, store_signature) = if let Some(client) = store_client {
        match client.request_download_url(&manifest.id).await {
            Ok(resp) => {
                debug!("Using signed download URL for {}", vm_name);
                let url = rewrite_to_store(&resp.download_url);
                debug!("[download] URL: {}", url);
                if let (Some(ref signed), Some(ref manifest_hash)) = (&resp.sha256, &manifest.source.sha256) {
                    if signed.to_lowercase() != manifest_hash.to_lowercase() {
                        warn!("[download] SHA256 mismatch between signed URL ({}) and manifest ({})", signed, manifest_hash);
                    }
                }
                (url, resp.sha256, resp.store_signature)
            }
            Err(e) => {
                warn!("Signed download URL unavailable ({}), falling back to direct URL", e);
                (manifest.source.image_url.clone(), manifest.source.sha256.clone(), None)
            }
        }
    } else {
        (manifest.source.image_url.clone(), manifest.source.sha256.clone(), None)
    };

    // Pick HTTP client: prefer pinned client from store_client, else new client
    let owned_client;
    let http_client = if let Some(client) = store_client {
        client.http_client()
    } else {
        owned_client = reqwest::Client::new();
        &owned_client
    };

    // 3. Download or load from cache
    emit_progress(emitter, "downloading", 0, &vm_name, None, None);

    let archive_path = vm_dir.join("disk.zip");

    let mut needs_cache_save = false;

    if manifest.source.tauri_cacheable {
        let mut cache_used = false;

        if let Some(cached) = get_cache_zip(app_data_dir, &manifest.source) {
            let cache_dir = cached.parent().unwrap();
            let stored_manifest = std::fs::read_to_string(cache_dir.join("manifest.json"))
                .unwrap_or_default();

            if stored_manifest == manifest_raw {
                // Manifest matches — but verify cached file SHA256 against signed URL hash
                let cache_valid = if let Some(ref signed_hash) = expected_sha256 {
                    match verify_sha256(&cached, signed_hash) {
                        Ok(()) => true,
                        Err(_) => {
                            debug!("Cache file SHA256 does not match signed URL hash, invalidating cache");
                            false
                        }
                    }
                } else {
                    true // No signed hash to compare — trust manifest match
                };

                if cache_valid {
                    debug!("Cache hit (fresh) for {}, skipping download", vm_name);
                    emit_progress(emitter, "downloading", 10, &vm_name, None, None);
                    std::fs::copy(&cached, &archive_path).context("Failed to copy from cache")?;
                    emit_progress(emitter, "downloading", 100, &vm_name, None, None);
                    let full_path = cache_dir.join("manifest_full.json");
                    if !full_path.exists() && !manifest_json.is_empty() {
                        let _ = std::fs::write(&full_path, &manifest_json);
                    }
                    cache_used = true;
                } else {
                    let _ = std::fs::remove_dir_all(cache_dir);
                }
            } else {
                debug!("Cache stale (manifest changed) for {}, re-downloading", vm_name);
                let _ = std::fs::remove_dir_all(cache_dir);
            }
        }

        if !cache_used {
            let _hash = download_file_with_hash(http_client, &download_url, &archive_path, emitter, &vm_name)
                .await
                .context("Failed to download disk image")?;
            needs_cache_save = true;
        }
    } else {
        let download_hash = download_file_with_hash(http_client, &download_url, &archive_path, emitter, &vm_name)
            .await
            .context("Failed to download disk image")?;

        // Verify streaming SHA256 against expected hash from signed URL response
        if let Some(ref expected) = expected_sha256 {
            let actual_hex = download_hash.iter().map(|b| format!("{:02x}", b)).collect::<String>();
            if actual_hex != expected.to_lowercase() {
                return Err(anyhow!(
                    "The downloaded file appears to be corrupted or was modified. \
                     Make sure you are connected to the official store at https://nilbox.run."
                ));
            }
            debug!("Streaming SHA256 verified for {} ({})", vm_name, &actual_hex[..12]);

            // Verify store Ed25519 signature over the SHA256 hex
            if let Some(ref sig) = store_signature {
                verify_file_signature(expected, sig)
                    .context("File signature verification failed")?;
                debug!("Store file signature verified for {}", vm_name);
            }
        }
    }

    // Always verify zip SHA256 when provided (all paths: download, cache-fresh copy, stale re-download)
    // Prefer signed URL SHA256 (fresher) over manifest SHA256 (may be stale after server-side update)
    let verify_hash = expected_sha256.as_ref().or(manifest.source.sha256.as_ref());
    if let Some(ref expected) = verify_hash {
        verify_sha256(&archive_path, expected).context("The download appears to be corrupted or was modified. Make sure you are connected to the official store at https://nilbox.run.")?;
        debug!("SHA256 verified for {}", vm_name);
    }

    // Save to cache only after successful SHA256 verification
    if needs_cache_save {
        save_to_cache(app_data_dir, &manifest.source, &archive_path, &manifest_raw, &manifest_json, &manifest.id, verify_hash.map(|s| s.as_str()))
            .context("Failed to save to cache")?;
    }

    // 4. Derive filename: prefer explicit field, fall back to URL basename (strip .zip)
    let disk_filename = manifest
        .source
        .disk_image
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            std::path::Path::new(&manifest.source.image_url)
                .file_stem()        // strip .zip → e.g. "debian-vm"
                .and_then(|s| s.to_str())
                .map(|n| format!("{}.img", n))
                .unwrap_or_else(|| "disk.img".to_string())
        });

    debug!("[install_vm] disk_filename='{}' archive='{}' ({} bytes)",
        disk_filename,
        archive_path.display(),
        archive_path.metadata().map(|m| m.len()).unwrap_or(0));

    // 5. Extract
    emit_progress(emitter, "extracting", 0, &vm_name, None, None);

    let disk_image_path = vm_dir.join("disk.img");
    debug!("[install_vm] extracting disk image → {}", disk_image_path.display());
    extract_zip(&archive_path, &disk_filename, &disk_image_path)
        .map_err(|e| { error!("[install_vm] extract_zip failed: {:#}", e); e })
        .context("Failed to extract disk image")?;
    debug!("[install_vm] disk image extracted ({} bytes)", disk_image_path.metadata().map(|m| m.len()).unwrap_or(0));

    // 6. Extract kernel from zip (or download if HTTP URL / use if absolute path)
    debug!("[install_vm] resolving kernel: {:?}", manifest.source.kernel);
    let kernel_path = resolve_boot_file(
        manifest.source.kernel.as_deref(),
        &vm_dir,
        "vmlinuz",
        &archive_path,
        emitter,
        &vm_name,
    )
    .await
    .map_err(|e| { error!("[install_vm] resolve kernel failed: {:#}", e); e })
    .context("Failed to resolve kernel")?;
    debug!("[install_vm] kernel resolved: {:?}", kernel_path);

    // 7. Extract initrd from zip (or download if HTTP URL / use if absolute path)
    debug!("[install_vm] resolving initrd: {:?}", manifest.source.initrd);
    let initrd_path = resolve_boot_file(
        manifest.source.initrd.as_deref(),
        &vm_dir,
        "initrd.img",
        &archive_path,
        emitter,
        &vm_name,
    )
    .await
    .map_err(|e| { error!("[install_vm] resolve initrd failed: {:#}", e); e })
    .context("Failed to resolve initrd")?;
    debug!("[install_vm] initrd resolved: {:?}", initrd_path);

    // Remove archive after all extractions are complete
    let _ = std::fs::remove_file(&archive_path);

    // 8. Register
    debug!("[install_vm] registering VM: name='{}' disk='{}' kernel={:?} initrd={:?} append={:?}",
        vm_name, disk_image_path.display(), kernel_path, initrd_path, manifest.source.append);
    emit_progress(emitter, "registering", 100, &vm_name, None, None);

    let record = VmRecord {
        id: vm_id.clone(),
        name: vm_name.clone(),
        disk_image: disk_image_path.to_string_lossy().to_string(),
        kernel: kernel_path,
        initrd: initrd_path,
        append: manifest.source.append.clone(),
        memory_mb: manifest.source.min_memory.unwrap_or(512),
        cpus: manifest.source.cpus.unwrap_or(2),
        is_default: false,
        description: manifest.description.clone(),
        last_boot_at: None,
        created_at: String::new(), // set by insert_vm
        admin_url: manifest.admin.first().map(|a| a.url.clone()),
        admin_label: manifest.admin.first().and_then(|a| a.label.clone()),
        base_os: manifest.base_os.clone(),
        base_os_version: manifest.base_os_version.clone(),
        target_platform: manifest.target_platform.as_ref().map(|v| v.join(",")),
    };

    let admin_urls: Vec<(String, String)> = manifest
        .admin
        .iter()
        .map(|a| (a.url.clone(), a.label.clone().unwrap_or_default()))
        .collect();

    let created_at = config_store
        .insert_vm(&record, &admin_urls)
        .context("Failed to insert VM record into database")?;

    // Save inbound ports from manifest permissions as port mappings (admin only)
    if user_verified.as_deref() == Some("admin") {
        for &port in &manifest.permissions.inbound_ports {
            config_store.insert_port_mapping(&vm_id, port, port, &manifest.name)
                .context("Failed to insert port mapping")?;
            debug!("Port mapping added: {}", port);
        }
    } else if !manifest.permissions.inbound_ports.is_empty() {
        debug!("Skipping inbound ports (user verified={:?}, not admin)", user_verified);
    }

    debug!("VM installed: {} ({})", vm_name, vm_id);

    Ok(VmRecord { created_at, ..record })
}

/// Install a VM from local cache only — no network access.
/// Reads `manifest_full.json` and copies `disk.zip` from the cache directory
/// identified by `app_id`.
pub fn install_from_cache(
    app_id: &str,
    app_data_dir: &Path,
    emitter: &Arc<dyn EventEmitter>,
    config_store: &Arc<ConfigStore>,
) -> Result<VmRecord> {
    let cache_root = app_data_dir.join("cache");
    let entries = std::fs::read_dir(&cache_root)
        .context("Cache directory not found")?;

    // Find the cache entry matching this app_id
    let mut cache_dir: Option<std::path::PathBuf> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() { continue; }
        if let Ok(stored_id) = std::fs::read_to_string(path.join("app_id")) {
            if stored_id.trim() == app_id {
                cache_dir = Some(path);
                break;
            }
        }
    }

    let cache_dir = cache_dir.ok_or_else(|| anyhow!("No cache entry found for app {}", app_id))?;

    // disk.zip must exist
    let cached_zip = cache_dir.join("disk.zip");
    if !cached_zip.exists() {
        return Err(anyhow!("Cached disk.zip not found for app {}", app_id));
    }

    // Read manifest — manifest_full.json is required (manifest.json only stores SHA256 hash)
    let manifest_full_path = cache_dir.join("manifest_full.json");
    if !manifest_full_path.exists() {
        return Err(anyhow!(
            "Cache entry for app {} is outdated (missing manifest_full.json). Please reinstall from the Store.",
            app_id
        ));
    }
    let manifest_str = std::fs::read_to_string(&manifest_full_path)
        .context("Failed to read cached manifest_full.json")?;

    let manifest: VmManifest = serde_json::from_str(&manifest_str)
        .context("Failed to parse cached manifest")?;

    // Platform compatibility check
    if let Some(ref platforms) = manifest.target_platform {
        if !platforms.is_empty() {
            let host = host_platform();
            if !platforms.iter().any(|p| p == host || host.starts_with(p)) {
                return Err(anyhow!(
                    "This image is not compatible with your system ({}). Supported platforms: {}",
                    host,
                    platforms.join(", ")
                ));
            }
        }
    }

    let vm_id = Uuid::new_v4().to_string();
    let vm_dir = app_data_dir.join("vms").join(&vm_id);
    std::fs::create_dir_all(&vm_dir).context("Failed to create VM directory")?;

    let vm_name = config_store.unique_vm_name(&manifest.name)?;

    // Copy cached zip
    emit_progress(emitter, "downloading", 0, &vm_name, None, None);
    debug!("Cache install for {}, copying from {}", vm_name, cached_zip.display());
    let archive_path = vm_dir.join("disk.zip");
    std::fs::copy(&cached_zip, &archive_path).context("Failed to copy from cache")?;
    emit_progress(emitter, "downloading", 100, &vm_name, None, None);

    // Verify SHA256 if available
    // Prefer disk.sha256 (the hash actually used during download verification) over manifest.source.sha256,
    // because the signed download URL may provide a different hash than the manifest field.
    let cached_sha256 = std::fs::read_to_string(cache_dir.join("disk.sha256")).ok();
    let verify_sha = cached_sha256.as_deref().or(manifest.source.sha256.as_deref());
    if let Some(expected) = verify_sha {
        verify_sha256(&archive_path, expected)
            .context("Cached image integrity check failed")?;
        debug!("SHA256 verified for cached {}", vm_name);
    }

    // Derive disk filename
    let disk_filename = manifest
        .source
        .disk_image
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            std::path::Path::new(&manifest.source.image_url)
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|n| format!("{}.img", n))
                .unwrap_or_else(|| "disk.img".to_string())
        });

    // Extract
    emit_progress(emitter, "extracting", 0, &vm_name, None, None);
    let disk_image_path = vm_dir.join("disk.img");
    extract_zip(&archive_path, &disk_filename, &disk_image_path)
        .context("Failed to extract disk image from cache")?;

    // Extract kernel & initrd from zip (local paths only, no downloads)
    let kernel_path = extract_boot_file_from_zip(
        manifest.source.kernel.as_deref(), &archive_path, &vm_dir, "vmlinuz",
    );
    let initrd_path = extract_boot_file_from_zip(
        manifest.source.initrd.as_deref(), &archive_path, &vm_dir, "initrd.img",
    );

    // Remove archive
    let _ = std::fs::remove_file(&archive_path);

    // Register
    emit_progress(emitter, "registering", 100, &vm_name, None, None);

    let record = VmRecord {
        id: vm_id.clone(),
        name: vm_name.clone(),
        disk_image: disk_image_path.to_string_lossy().to_string(),
        kernel: kernel_path,
        initrd: initrd_path,
        append: manifest.source.append.clone(),
        memory_mb: manifest.source.min_memory.unwrap_or(512),
        cpus: manifest.source.cpus.unwrap_or(2),
        is_default: false,
        description: manifest.description.clone(),
        last_boot_at: None,
        created_at: String::new(),
        admin_url: manifest.admin.first().map(|a| a.url.clone()),
        admin_label: manifest.admin.first().and_then(|a| a.label.clone()),
        base_os: manifest.base_os.clone(),
        base_os_version: manifest.base_os_version.clone(),
        target_platform: manifest.target_platform.as_ref().map(|v| v.join(",")),
    };

    let admin_urls: Vec<(String, String)> = manifest
        .admin
        .iter()
        .map(|a| (a.url.clone(), a.label.clone().unwrap_or_default()))
        .collect();

    let created_at = config_store
        .insert_vm(&record, &admin_urls)
        .context("Failed to insert VM record into database")?;

    debug!("VM installed from cache: {} ({})", vm_name, vm_id);

    Ok(VmRecord { created_at, ..record })
}

/// Extract a boot file (kernel/initrd) from zip if the source is a filename (not URL).
fn extract_boot_file_from_zip(
    source: Option<&str>,
    archive_path: &Path,
    vm_dir: &Path,
    default_name: &str,
) -> Option<String> {
    let name = source?;
    // Skip URLs — cache install is offline
    if name.starts_with("http://") || name.starts_with("https://") {
        return None;
    }
    // If it's an absolute path, use as-is
    if std::path::Path::new(name).is_absolute() {
        return Some(name.to_string());
    }
    // Try to extract from zip
    let dest = vm_dir.join(default_name);
    match extract_zip(archive_path, name, &dest) {
        Ok(_) => Some(dest.to_string_lossy().to_string()),
        Err(_) => None,
    }
}

// ── Platform detection ────────────────────────────────────────

/// Returns the platform identifier for the current host.
fn host_platform() -> &'static str {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    { "arm_mac" }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    { "intel_mac" }
    #[cfg(target_os = "linux")]
    { "linux" }
    #[cfg(target_os = "windows")]
    { "win10" } // TODO: detect win10 vs win11
}

// ── Private helpers ───────────────────────────────────────────

fn emit_progress(
    emitter: &Arc<dyn EventEmitter>,
    stage: &str,
    percent: u8,
    vm_name: &str,
    vm_id: Option<String>,
    error: Option<String>,
) {
    emit_typed(
        emitter,
        "vm-install-progress",
        &VmInstallProgress {
            stage: stage.to_string(),
            percent,
            vm_name: vm_name.to_string(),
            vm_id,
            error,
        },
    );
}

/// Recursively rebuild a JSON value with all object keys sorted (BTreeMap order).
/// This matches Python's `json.dumps(..., sort_keys=True)`.
pub fn canonical_json_value(v: serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(map) => {
            let sorted: BTreeMap<String, serde_json::Value> =
                map.into_iter().map(|(k, v)| (k, canonical_json_value(v))).collect();
            serde_json::Value::Object(sorted.into_iter().collect())
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.into_iter().map(canonical_json_value).collect())
        }
        other => other,
    }
}

/// Replace the scheme+host of any URL with https://store.nilbox.run.
/// Leaves absolute local paths (starting with '/') and bare filenames unchanged.
fn rewrite_to_store(url: &str) -> String {
    use crate::store::STORE_BASE_URL;
    if url.starts_with('/') || (!url.starts_with("http://") && !url.starts_with("https://")) {
        return url.to_string();
    }
    // Find start of path after scheme://host
    if let Some(after_scheme) = url.find("//") {
        if let Some(path_start) = url[after_scheme + 2..].find('/') {
            let path = &url[after_scheme + 2 + path_start..];
            return format!("{}{}", STORE_BASE_URL, path);
        }
    }
    url.to_string()
}

/// Returns (manifest, manifest_sha256, manifest_json_string).
async fn fetch_manifest(url: &str) -> Result<(VmManifest, String, String)> {
    use crate::store::envelope::parse_envelope;
    use crate::store::verify::verify_envelope;

    debug!("[fetch_manifest] fetching: {}", url);

    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| {
            error!("[fetch_manifest] HTTP request failed: {}", e);
            anyhow!("HTTP request failed: {}", e)
        })?;

    let status = resp.status();
    debug!("[fetch_manifest] HTTP status: {}", status);

    if !status.is_success() {
        error!("[fetch_manifest] non-success HTTP status: {}", status);
        return Err(anyhow!("Manifest fetch failed: HTTP {}", status));
    }

    let text = resp.text().await.map_err(|e| {
        error!("[fetch_manifest] failed to read response body: {}", e);
        anyhow!("Failed to read manifest body: {}", e)
    })?;

    debug!("[fetch_manifest] body length: {} bytes", text.len());

    // Parse as raw Value first so we can verify integrity before deserializing
    let raw: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        error!("[fetch_manifest] JSON parse failed: {} | body preview: {:.200}", e, &text);
        anyhow!("Failed to parse manifest JSON: {}", e)
    })?;

    let envelope = parse_envelope(&raw).map_err(|e| {
        error!("[fetch_manifest] envelope parse failed: {}", e);
        anyhow!("Failed to parse manifest envelope: {}", e)
    })?;

    let manifest_value = verify_envelope(&envelope).map_err(|e| {
        error!("[fetch_manifest] envelope verification failed: {}", e);
        anyhow!("The app information appears to be corrupted or was modified. Make sure you are connected to the official store at https://nilbox.run. (detail: {})", e)
    })?;

    let manifest: VmManifest = serde_json::from_value(manifest_value.clone()).map_err(|e| {
        error!("[fetch_manifest] manifest deserialization failed: {}", e);
        anyhow!("Failed to deserialize manifest: {}", e)
    })?;

    // For cache key compatibility, extract manifest_sha256
    let manifest_sha256 = match &envelope {
        crate::store::envelope::ManifestEnvelope::V3(v3) => v3.inner.signed_payload.manifest_sha256.clone(),
        crate::store::envelope::ManifestEnvelope::V2(v2) => v2.signed_payload.manifest_sha256.clone(),
        crate::store::envelope::ManifestEnvelope::Legacy(p) => p.manifest_sha256.clone(),
    };

    let manifest_json = serde_json::to_string(&manifest_value).unwrap_or_default();

    debug!("[fetch_manifest] success: id={}, name={}", manifest.id, manifest.name);

    Ok((manifest, manifest_sha256, manifest_json))
}

/// Cache key: the archive SHA256 if known, else a hash of the image URL.
fn cache_key_for(source: &ManifestSource) -> String {
    if let Some(sha256) = &source.sha256 {
        sha256.clone()
    } else {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        source.image_url.hash(&mut h);
        format!("url-{:016x}", h.finish())
    }
}

/// Returns the cached zip path if it exists on disk.
fn get_cache_zip(app_data_dir: &Path, source: &ManifestSource) -> Option<std::path::PathBuf> {
    let p = app_data_dir
        .join("cache")
        .join(cache_key_for(source))
        .join("disk.zip");
    if p.exists() { Some(p) } else { None }
}

/// Persist zip + manifest JSON to cache directory (permanent storage).
fn save_to_cache(
    app_data_dir: &Path,
    source: &ManifestSource,
    zip_path: &Path,
    manifest_raw: &str,
    manifest_json: &str,
    app_id: &str,
    verified_sha256: Option<&str>,
) -> Result<()> {
    let current_key = cache_key_for(source);
    let dir = app_data_dir.join("cache").join(&current_key);
    std::fs::create_dir_all(&dir).context("Failed to create cache directory")?;
    std::fs::copy(zip_path, dir.join("disk.zip")).context("Failed to copy zip to cache")?;
    std::fs::write(dir.join("manifest.json"), manifest_raw)
        .context("Failed to write manifest to cache")?;
    std::fs::write(dir.join("manifest_full.json"), manifest_json)
        .context("Failed to write full manifest to cache")?;
    std::fs::write(dir.join("app_id"), app_id)
        .context("Failed to write app_id to cache")?;
    if let Some(sha256) = verified_sha256 {
        std::fs::write(dir.join("disk.sha256"), sha256)
            .context("Failed to write sha256 to cache")?;
    }
    debug!("Cached: {}", dir.display());

    // Evict old version caches for the same app_id
    evict_old_caches(app_data_dir, app_id, &current_key);

    Ok(())
}

/// Delete cached entries for `app_id` that are not the current `current_key`.
fn evict_old_caches(app_data_dir: &Path, app_id: &str, current_key: &str) {
    let cache_root = app_data_dir.join("cache");
    let entries = match std::fs::read_dir(&cache_root) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() { continue; }
        // Skip the directory we just wrote
        if path.file_name().and_then(|n| n.to_str()) == Some(current_key) { continue; }
        // Check if this cache entry belongs to the same app
        let stored_id = match std::fs::read_to_string(path.join("app_id")) {
            Ok(s) => s,
            Err(_) => continue, // No app_id file — not managed by us, skip
        };
        if stored_id.trim() == app_id {
            match std::fs::remove_dir_all(&path) {
                Ok(_) => debug!("Evicted old cache for {}: {}", app_id, path.display()),
                Err(e) => warn!("Failed to evict old cache {}: {}", path.display(), e),
            }
        }
    }
}

/// Summary of a cached OS image found in the local cache directory.
#[derive(Debug, Clone, Serialize)]
pub struct CachedImageInfo {
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub base_os: Option<String>,
    pub base_os_version: Option<String>,
    pub manifest_url: String,
}

/// List locally cached OS images by scanning `app_data_dir/cache/*/`.
/// Reads `manifest_full.json` if available; falls back to `app_id` file.
pub fn list_cached_images(app_data_dir: &Path) -> Vec<CachedImageInfo> {
    let cache_root = app_data_dir.join("cache");
    let entries = match std::fs::read_dir(&cache_root) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut result = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() { continue; }
        if !path.join("disk.zip").exists() { continue; }

        // Only include entries with manifest_full.json (has full metadata)
        let manifest_str = match std::fs::read_to_string(path.join("manifest_full.json")) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let manifest: VmManifest = match serde_json::from_str(&manifest_str) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let app_id = std::fs::read_to_string(path.join("app_id"))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| manifest.id.clone());
        let manifest_url = format!("https://store.nilbox.run/apps/{}/manifest", app_id);
        result.push(CachedImageInfo {
            id: app_id,
            name: manifest.name,
            version: manifest.version,
            base_os: manifest.base_os,
            base_os_version: manifest.base_os_version,
            manifest_url,
        });
    }
    result
}

/// Download a file with streaming SHA256, `.part` temp file, and resume support.
///
/// Returns the SHA256 hash of the complete downloaded file.
async fn download_file_with_hash(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    emitter: &Arc<dyn EventEmitter>,
    vm_name: &str,
) -> Result<[u8; 32]> {
    let part_path = dest.with_extension("zip.part");

    // Check for resumable partial download (< 24h old)
    let mut resume_offset: u64 = 0;
    let mut hasher = Sha256::new();

    if part_path.exists() {
        if let Ok(meta) = std::fs::metadata(&part_path) {
            let age = meta.modified()
                .ok()
                .and_then(|t| t.elapsed().ok())
                .map(|d| d.as_secs())
                .unwrap_or(u64::MAX);

            if age < 86400 && meta.len() > 0 {
                // Re-hash partial file for resume
                debug!("Found .part file ({} bytes, {}s old), attempting resume", meta.len(), age);
                let mut f = std::fs::File::open(&part_path).context("Failed to open .part file")?;
                let mut buf = [0u8; 65536];
                loop {
                    let n = f.read(&mut buf).context("Read error during .part hash")?;
                    if n == 0 { break; }
                    hasher.update(&buf[..n]);
                }
                resume_offset = meta.len();
            } else {
                let _ = std::fs::remove_file(&part_path);
            }
        }
    }

    // Build request with optional Range header
    let mut req = client.get(url);
    if resume_offset > 0 {
        req = req.header("Range", format!("bytes={}-", resume_offset));
    }

    let resp = req.send().await.context("HTTP GET failed")?;

    // Log redirect if final URL differs from requested URL
    let final_url = resp.url().as_str();
    if final_url != url {
        debug!("[download] Redirected to: {}", final_url);
    }

    let status = resp.status().as_u16();

    match status {
        200 => {
            // Server ignores Range or fresh download — restart from scratch
            if resume_offset > 0 {
                debug!("Server returned 200 (ignoring Range), restarting download");
                hasher = Sha256::new();
                resume_offset = 0;
            }
        }
        206 => {
            // Partial content — resume from offset
            debug!("Resuming download from offset {}", resume_offset);
        }
        416 => {
            // Range not satisfiable — restart
            debug!("Range not satisfiable, restarting download");
            hasher = Sha256::new();
            let _ = std::fs::remove_file(&part_path);
            // Re-request without Range
            let resp = client.get(url).send().await.context("HTTP GET retry failed")?;
            let final_url = resp.url().as_str();
            if final_url != url {
                debug!("[download] Redirected to: {}", final_url);
            }
            if !resp.status().is_success() {
                return Err(anyhow!("Download failed: HTTP {}", resp.status()));
            }
            return download_stream(resp, &part_path, dest, hasher, 0, emitter, vm_name).await;
        }
        _ if resp.status().is_success() => {}
        _ => {
            return Err(anyhow!("Download failed: HTTP {}", resp.status()));
        }
    }

    download_stream(resp, &part_path, dest, hasher, resume_offset, emitter, vm_name).await
}

/// Stream response body to .part file, compute SHA256, rename on completion.
async fn download_stream(
    resp: reqwest::Response,
    part_path: &Path,
    dest: &Path,
    mut hasher: Sha256,
    resume_offset: u64,
    emitter: &Arc<dyn EventEmitter>,
    vm_name: &str,
) -> Result<[u8; 32]> {
    let total_bytes = resp.content_length().map(|cl| cl + resume_offset);
    let mut downloaded: u64 = resume_offset;
    let mut last_percent: u8 = 0;

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(resume_offset > 0)
        .write(true)
        .truncate(resume_offset == 0)
        .open(part_path)
        .context("Failed to create .part download file")?;

    let mut stream = resp.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("Stream error during download")?;
        file.write_all(&chunk).context("Write error during download")?;
        hasher.update(&chunk);
        downloaded += chunk.len() as u64;

        if let Some(total) = total_bytes {
            let percent = ((downloaded as f64 / total as f64) * 100.0).min(99.0) as u8;
            if percent > last_percent {
                last_percent = percent;
                emit_progress(emitter, "downloading", percent, vm_name, None, None);
            }
        }
    }

    drop(file);

    // Rename .part → final destination
    std::fs::rename(part_path, dest).context("Failed to rename .part file to final destination")?;

    let hash: [u8; 32] = hasher.finalize().into();
    Ok(hash)
}

/// Legacy download_file wrapper for resolve_boot_file (no SHA256 return needed).
async fn download_file(
    url: &str,
    dest: &Path,
    emitter: &Arc<dyn EventEmitter>,
    vm_name: &str,
) -> Result<()> {
    let client = reqwest::Client::new();
    let _hash = download_file_with_hash(&client, url, dest, emitter, vm_name).await?;
    Ok(())
}

fn verify_sha256(path: &Path, expected: &str) -> Result<()> {
    let file_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let mut file = std::fs::File::open(path).context("Failed to open file for SHA256")?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf).context("Read error during SHA256")?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected.to_lowercase() {
        error!("SHA256 mismatch: file={} size={} actual={} expected={}", path.display(), file_size, actual, expected.to_lowercase());
        return Err(anyhow!(
            "The download appears to be corrupted or was modified (size={}, actual={}, expected={})",
            file_size, &actual[..12], &expected.to_lowercase()[..12]
        ));
    }
    Ok(())
}

/// Verify an Ed25519 store signature over a file's SHA256 hex string.
///
/// The store signs `sha256_hex.encode()` with the same Ed25519 key used for manifests.
/// `sig_b64` is the base64-encoded 64-byte signature.
fn verify_file_signature(sha256_hex: &str, sig_b64: &str) -> Result<()> {
    use crate::store::keys::get_store_public_key;
    use base64::{Engine as _, engine::general_purpose::STANDARD};

    // Try all known key IDs (dev key in dev-store mode, production otherwise)
    let key_ids = [
        #[cfg(any(test, feature = "dev-store"))]
        "nilbox-store-dev",
        "nilbox-store-2026",
    ];

    let sig_bytes = STANDARD.decode(sig_b64)
        .map_err(|e| anyhow!("Invalid file signature base64: {}", e))?;
    let signature = Signature::from_slice(&sig_bytes)
        .map_err(|e| anyhow!("Invalid file signature format: {}", e))?;

    for key_id in &key_ids {
        if let Ok(verifying_key) = get_store_public_key(key_id) {
            if verifying_key.verify(sha256_hex.as_bytes(), &signature).is_ok() {
                debug!("File signature verified with key '{}'", key_id);
                return Ok(());
            }
        }
    }

    Err(anyhow!("File signature verification failed — no matching store key"))
}

/// Resolve a kernel or initrd value from the manifest to a local absolute path.
///
/// - None / empty        → None
/// - absolute path (`/…`) → use as-is (already local)
/// - http(s):// URL      → download to `vm_dir/dest_name`
/// - bare filename       → extract from `archive` zip (no network request)
async fn resolve_boot_file(
    value: Option<&str>,
    vm_dir: &Path,
    dest_name: &str,
    archive: &Path,
    emitter: &Arc<dyn EventEmitter>,
    vm_name: &str,
) -> Result<Option<String>> {
    let s = match value {
        None | Some("") => return Ok(None),
        Some(s) => s,
    };

    // Already an absolute local path — use directly
    if s.starts_with('/') {
        return Ok(Some(s.to_string()));
    }

    // HTTP URL — download directly
    if s.starts_with("http://") || s.starts_with("https://") {
        let dest = vm_dir.join(dest_name);
        download_file(s, &dest, emitter, vm_name)
            .await
            .with_context(|| format!("Failed to download {} from {}", dest_name, s))?;
        return Ok(Some(dest.to_string_lossy().to_string()));
    }

    // Bare filename — extract from zip archive
    let dest = vm_dir.join(dest_name);
    match try_extract_from_zip(archive, s, &dest)? {
        Some(path) => Ok(Some(path)),
        None => Err(anyhow!("'{}' not found in zip archive", s)),
    }
}

/// Try to extract a named file from the zip archive.
/// Returns the destination path if found, None if the entry is absent.
fn try_extract_from_zip(archive: &Path, filename: &str, dest: &Path) -> Result<Option<String>> {
    let file = std::fs::File::open(archive).context("Failed to open ZIP archive")?;
    let mut zip = zip::ZipArchive::new(file).context("Failed to read ZIP archive")?;

    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).context("Failed to read ZIP entry")?;
        let name = entry.name().to_string();
        let base = std::path::Path::new(&name)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        if base == filename {
            let mut dest_file = std::fs::File::create(dest)
                .with_context(|| format!("Failed to create {}", dest.display()))?;
            std::io::copy(&mut entry, &mut dest_file)
                .context("Failed to extract ZIP entry")?;
            debug!("Extracted '{}' → {}", filename, dest.display());
            return Ok(Some(dest.to_string_lossy().to_string()));
        }
    }

    Ok(None)
}

fn extract_zip(archive: &Path, target_filename: &str, dest: &Path) -> Result<()> {
    let file = std::fs::File::open(archive).context("Failed to open ZIP archive")?;
    let mut zip = zip::ZipArchive::new(file).context("Failed to read ZIP archive")?;

    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).context("Failed to read ZIP entry")?;

        // Match by bare filename (strip any directory prefix in the zip entry name)
        let name = entry.name().to_string();
        let base = std::path::Path::new(&name)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        if base == target_filename {
            let mut dest_file = std::fs::File::create(dest)
                .with_context(|| format!("Failed to create {}", dest.display()))?;
            std::io::copy(&mut entry, &mut dest_file)
                .context("Failed to extract ZIP entry")?;
            debug!("Extracted '{}' → {}", target_filename, dest.display());
            return Ok(());
        }
    }

    Err(anyhow!(
        "File '{}' not found in ZIP archive",
        target_filename
    ))
}

// ── Unit tests ────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::envelope::parse_envelope;
    use crate::store::verify::verify_envelope;

    #[test]
    fn test_manifest_deserialize() {
        let json = r#"{
            "manifest_sha256": "ac06a46a98554614ba11ff4f2a495734757367f9427682421de991b771dac4ce",
            "manifest": {
                "id": "debian-bookworm",
                "name": "debian-bookworm",
                "version": "1.0.0",
                "icon_url": null,
                "description": "Debian Bookworm base image for ARM64",
                "category": "utility",
                "type": "vm_image",
                "source": {
                    "image_url": "https://example.com/debian-bookworm-arm64.tar.gz",
                    "image_format": "raw",
                    "first_boot_script": "install.sh",
                    "disk_image": "debian-bookworm-arm64.img",
                    "sha256": "abc123def456abc123def456abc123def456abc123def456abc123def456abcd",
                    "kernel": "http://localhost:8000/vm-files/vmlinuz",
                    "initrd": "http://localhost:8000/vm-files/initrd.img",
                    "append": "console=hvc0 root=/dev/vda1 rw",
                    "min_memory": 2048,
                    "cpus": 4,
                    "tauri_cacheable": false
                },
                "taskfile_url": null,
                "taskfile_sha256": null,
                "taskfile_content": "excluded from hash",
                "source_url": null,
                "admin": null,
                "permissions": {
                    "allowed_domains": [],
                    "inbound_ports": [],
                    "tokens": []
                }
            }
        }"#;

        let raw: serde_json::Value = serde_json::from_str(json).unwrap();
        let envelope = parse_envelope(&raw).unwrap();
        verify_envelope(&envelope).expect("integrity check should pass");

        let resp: ManifestResponse = serde_json::from_str(json).unwrap();
        let manifest = resp.manifest;
        assert_eq!(manifest.name, "debian-bookworm");
        assert_eq!(manifest.source.image_url, "https://example.com/debian-bookworm-arm64.tar.gz");
        assert_eq!(manifest.source.image_format.as_deref(), Some("raw"));
        assert_eq!(manifest.source.first_boot_script.as_deref(), Some("install.sh"));
        assert_eq!(manifest.source.disk_image.as_deref(), Some("debian-bookworm-arm64.img"));
        assert_eq!(manifest.source.min_memory, Some(2048));
        assert_eq!(manifest.source.cpus, Some(4));
        assert!(manifest.source.sha256.is_some());
        assert_eq!(manifest.source.append.as_deref(), Some("console=hvc0 root=/dev/vda1 rw"));
        assert_eq!(manifest.source.kernel.as_deref(), Some("http://localhost:8000/vm-files/vmlinuz"));
        assert_eq!(manifest.source.initrd.as_deref(), Some("http://localhost:8000/vm-files/initrd.img"));
        assert_eq!(manifest.permissions.allowed_domains.len(), 0);
        assert_eq!(manifest.permissions.tokens.len(), 0);
    }

    #[test]
    fn test_manifest_minimal() {
        let json = r#"{
            "manifest_sha256": "6450368e4ebc7a11efaecee92fc0f150bdad2ab7cec2cc23809a301189f6e5f2",
            "manifest": {
                "name": "Minimal VM",
                "source": {
                    "image_url": "https://example.com/vm.tar.gz"
                }
            }
        }"#;

        let raw: serde_json::Value = serde_json::from_str(json).unwrap();
        let envelope = parse_envelope(&raw).unwrap();
        verify_envelope(&envelope).expect("integrity check should pass");

        let resp: ManifestResponse = serde_json::from_str(json).unwrap();
        let manifest = resp.manifest;
        assert_eq!(manifest.name, "Minimal VM");
        assert!(manifest.source.disk_image.is_none());
        assert!(manifest.source.sha256.is_none());
        assert_eq!(manifest.source.min_memory, None);
        assert_eq!(manifest.source.cpus, None);
    }

    #[test]
    fn test_manifest_integrity_tamper_detected() {
        let json = r#"{
            "manifest_sha256": "6450368e4ebc7a11efaecee92fc0f150bdad2ab7cec2cc23809a301189f6e5f2",
            "manifest": {
                "name": "TAMPERED VM",
                "source": {
                    "image_url": "https://example.com/vm.tar.gz"
                }
            }
        }"#;

        let raw: serde_json::Value = serde_json::from_str(json).unwrap();
        let envelope = parse_envelope(&raw).unwrap();
        assert!(verify_envelope(&envelope).is_err());
    }
}
