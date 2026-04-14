//! System operation handlers for the Control Port.
//!
//! - `get_fs_info`: filesystem usage via libc::statvfs (no shell)
//! - `get_system_metrics`: reads /proc files directly (no shell)
//! - `expand_partition`: validated device path + Command::new (no shell interpolation)

use anyhow::{Result, anyhow};
use tracing::{debug, warn};

// ── Device path validation (ported from nilbox-core validate.rs) ────────

fn is_valid_device_path(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("/dev/vd") else { return false };
    let mut chars = rest.chars();
    let Some(disk) = chars.next() else { return false };
    if !disk.is_ascii_lowercase() {
        return false;
    }
    let tail: String = chars.collect();
    tail.is_empty() || tail.chars().all(|c| c.is_ascii_digit())
}

// ── get_fs_info ─────────────────────────────────────────────────────────

/// Get filesystem info for `mount_point` using libc::statvfs.
/// Returns JSON with device, total_mb, used_mb, avail_mb, use_pct.
#[cfg(target_os = "linux")]
pub fn handle_get_fs_info(mount_point: &str) -> Result<serde_json::Value> {
    use std::ffi::CString;

    // Validate mount_point: must start with / and contain no suspicious chars
    if !mount_point.starts_with('/') || mount_point.contains("..") {
        return Err(anyhow!("Invalid mount point: {}", mount_point));
    }

    let c_path = CString::new(mount_point)
        .map_err(|e| anyhow!("Invalid mount point: {}", e))?;

    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let ret = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
    if ret != 0 {
        return Err(anyhow!("statvfs failed for {}: errno={}", mount_point, std::io::Error::last_os_error()));
    }

    let block_size = stat.f_frsize as u64;
    let total_mb = (stat.f_blocks * block_size) / (1024 * 1024);
    let avail_mb = (stat.f_bavail * block_size) / (1024 * 1024);
    let free_mb = (stat.f_bfree * block_size) / (1024 * 1024);
    let used_mb = total_mb - free_mb;
    let use_pct = if total_mb > 0 { (used_mb * 100) / total_mb } else { 0 };

    // Find device from /proc/mounts
    let device = find_mount_device(mount_point).unwrap_or_else(|| "unknown".to_string());

    Ok(serde_json::json!({
        "device": device,
        "total_mb": total_mb,
        "used_mb": used_mb,
        "avail_mb": avail_mb,
        "use_pct": use_pct,
    }))
}

#[cfg(not(target_os = "linux"))]
pub fn handle_get_fs_info(_mount_point: &str) -> Result<serde_json::Value> {
    Err(anyhow!("get_fs_info not supported on this platform"))
}

/// Find the device for a given mount point by reading /proc/mounts.
fn find_mount_device(mount_point: &str) -> Option<String> {
    let mounts = std::fs::read_to_string("/proc/mounts").ok()?;
    for line in mounts.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 && parts[1] == mount_point {
            return Some(parts[0].to_string());
        }
    }
    None
}

// ── get_system_metrics ──────────────────────────────────────────────────

/// Read /proc/stat, /proc/meminfo, and /proc/net/dev as raw text.
/// The host already has parsers for these formats.
pub fn handle_get_system_metrics() -> Result<serde_json::Value> {
    let proc_stat = std::fs::read_to_string("/proc/stat")
        .unwrap_or_default();
    let meminfo = std::fs::read_to_string("/proc/meminfo")
        .unwrap_or_default();
    let net_dev = std::fs::read_to_string("/proc/net/dev")
        .unwrap_or_default();

    // Concatenate in the same format as `cat /proc/stat /proc/meminfo /proc/net/dev`
    // so the existing host-side parsers work unchanged.
    let combined = format!("{}{}{}", proc_stat, meminfo, net_dev);

    Ok(serde_json::json!({ "raw": combined }))
}

// ── expand_partition ────────────────────────────────────────────────────

/// Expand a disk partition. Validates device path, then runs growpart + resize2fs.
/// Subprocess arguments are passed as separate args — never through a shell.
pub async fn handle_expand_partition(root_device: &str) -> Result<String> {
    // 1. Validate device path (defense in depth — host also validates)
    if !is_valid_device_path(root_device) {
        return Err(anyhow!("Invalid device path: {}", root_device));
    }

    debug!("expand_partition: root_device={}", root_device);

    // 2. Detect partition number suffix (e.g. "/dev/vda1" → disk="vda", num="1")
    let part_num = root_device
        .strip_prefix("/dev/vda")
        .filter(|s| !s.is_empty());

    let mut output = String::new();

    if let Some(num) = part_num {
        // Partitioned disk: grow partition first
        let grow_result = tokio::process::Command::new("/usr/bin/growpart")
            .args(["/dev/vda", num])
            .output()
            .await;

        match grow_result {
            Ok(o) => {
                let stdout = String::from_utf8_lossy(&o.stdout);
                let stderr = String::from_utf8_lossy(&o.stderr);
                output.push_str(&format!("growpart /dev/vda {}: {}{}\n", num, stdout, stderr));
                debug!("growpart exit={}", o.status);
            }
            Err(e) => {
                warn!("growpart failed: {}", e);
                output.push_str(&format!("growpart failed: {}\n", e));
            }
        }
    }

    // resize2fs
    let resize_result = tokio::process::Command::new("/usr/sbin/resize2fs")
        .arg(root_device)
        .output()
        .await;

    match resize_result {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let stderr = String::from_utf8_lossy(&o.stderr);
            output.push_str(&format!("resize2fs {}: {}{}", root_device, stdout, stderr));
            debug!("resize2fs exit={}", o.status);
        }
        Err(e) => {
            warn!("resize2fs failed: {}", e);
            output.push_str(&format!("resize2fs failed: {}", e));
        }
    }

    Ok(output)
}
