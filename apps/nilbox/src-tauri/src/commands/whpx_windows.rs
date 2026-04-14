//! WHPX (Windows Hypervisor Platform) detection and activation.
//! Real work is done only on Windows; other platforms return an error.

use serde::Serialize;
use tauri::command;

#[derive(Serialize, Clone)]
pub struct WhpxStatus {
    /// "Enabled" | "Disabled" | "EnablePending" | "Unknown"
    pub state: String,
    /// True when HypervisorPlatform is enabled but a reboot is still needed.
    pub needs_reboot: bool,
    /// True when WHPX is fully available (Enabled state).
    pub available: bool,
}

#[cfg(not(target_os = "windows"))]
#[command]
pub async fn check_whpx_status() -> Result<WhpxStatus, String> {
    Err("WHPX is only available on Windows".into())
}

#[cfg(not(target_os = "windows"))]
#[command]
pub async fn enable_whpx() -> Result<WhpxStatus, String> {
    Err("WHPX is only available on Windows".into())
}

#[cfg(not(target_os = "windows"))]
#[command]
pub async fn reboot_for_whpx() -> Result<(), String> {
    Err("WHPX is only available on Windows".into())
}

// ── Windows implementation ──────────────────────────────────

#[cfg(target_os = "windows")]
use tracing::{debug, warn};

#[cfg(target_os = "windows")]
#[command]
pub async fn check_whpx_status() -> Result<WhpxStatus, String> {
    use std::os::windows::process::CommandExt;

    let output = tokio::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "(Get-WindowsOptionalFeature -Online -FeatureName HypervisorPlatform).State",
        ])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .output()
        .await
        .map_err(|e| format!("Failed to run PowerShell: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    debug!("WHPX feature state: {:?}", stdout);

    let (state, needs_reboot, available) = match stdout.as_str() {
        "Enabled" => ("Enabled".into(), false, true),
        "Disabled" => ("Disabled".into(), false, false),
        "EnablePending" => ("EnablePending".into(), true, false),
        other => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            warn!("Unexpected WHPX state: {:?}, stderr: {:?}", other, stderr);
            ("Unknown".into(), false, false)
        }
    };

    Ok(WhpxStatus { state, needs_reboot, available })
}

#[cfg(target_os = "windows")]
#[command]
pub async fn enable_whpx() -> Result<WhpxStatus, String> {
    use std::os::windows::process::CommandExt;

    debug!("Requesting UAC elevation to enable HypervisorPlatform...");

    // Use PowerShell Start-Process -Verb RunAs to trigger UAC and wait for completion.
    // This avoids needing Win32 ShellExecuteEx bindings.
    let output = tokio::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Start-Process powershell -Verb RunAs -Wait -WindowStyle Hidden -ArgumentList '-NoProfile -Command \"Enable-WindowsOptionalFeature -Online -FeatureName HypervisorPlatform -NoRestart; exit $LASTEXITCODE\"'",
        ])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .output()
        .await
        .map_err(|e| format!("Failed to run PowerShell: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        // Start-Process -Verb RunAs fails with "elevation required" type error when user cancels UAC
        if stderr.contains("canceled") || stderr.contains("cancelled") || stderr.contains("elevation") {
            return Err("Admin permission was denied.".into());
        }
        return Err(format!("Failed to enable WHPX: {}", stderr));
    }

    debug!("Elevated process completed, re-checking WHPX status...");
    check_whpx_status().await
}

#[cfg(target_os = "windows")]
#[command]
pub async fn reboot_for_whpx() -> Result<(), String> {
    use std::os::windows::process::CommandExt;

    debug!("User requested system reboot for WHPX activation");
    tokio::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", "Restart-Computer -Force"])
        .creation_flags(0x08000000)
        .spawn()
        .map_err(|e| format!("Failed to initiate reboot: {}", e))?;
    Ok(())
}
