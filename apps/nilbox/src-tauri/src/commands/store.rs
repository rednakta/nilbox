use crate::AppState;
use nilbox_core::store::{StoreItem, InstalledItem, InstallVerifyConfig};
use nilbox_core::store::auth::AuthStatus;
use tauri::State;

#[tauri::command]
pub async fn store_list_catalog(
    state: State<'_, AppState>,
) -> Result<Vec<StoreItem>, String> {
    Ok(state.service.store_list_catalog().await)
}

#[tauri::command]
pub async fn store_install(
    state: State<'_, AppState>,
    vm_id: String,
    manifest_url: String,
    verify_token: Option<String>,
    callback_url: Option<String>,
) -> Result<String, String> {
    let verify_config = match (verify_token, callback_url) {
        (Some(token), Some(url)) => Some(InstallVerifyConfig {
            verify_token: token,
            callback_url: url,
        }),
        _ => None,
    };
    state.service.store_install_app(&vm_id, &manifest_url, verify_config).await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn store_uninstall(
    state: State<'_, AppState>,
    item_id: String,
) -> Result<(), String> {
    state.service.store_uninstall(&item_id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn store_list_installed(
    state: State<'_, AppState>,
    vm_id: Option<String>,
) -> Result<Vec<InstalledItem>, String> {
    let vm_id = match vm_id {
        Some(id) => id,
        None => {
            let active = state.service.state.active_vm.read().await;
            match active.clone() {
                Some(id) => id,
                None => return Ok(vec![]),
            }
        }
    };
    state.service.store_list_installed_apps(&vm_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn store_register_install(
    state: State<'_, AppState>,
    vm_id: String,
    manifest_url: String,
) -> Result<(), String> {
    state.service.store_register_install(&vm_id, &manifest_url).await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn store_begin_login(
    state: State<'_, AppState>,
) -> Result<String, String> {
    state.service.store_begin_login().await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn store_begin_login_browser(
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.service.store_begin_login_browser().await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn store_cancel_login(state: State<'_, AppState>) {
    state.service.store_cancel_login();
}

#[tauri::command]
pub async fn store_login(
    state: State<'_, AppState>,
) -> Result<AuthStatus, String> {
    state.service.store_login().await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn store_logout(
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.service.store_logout().await;
    Ok(())
}

#[tauri::command]
pub async fn store_auth_status(
    state: State<'_, AppState>,
) -> Result<AuthStatus, String> {
    Ok(state.service.store_auth_status().await)
}

#[tauri::command]
pub async fn store_check_auth_status(
    state: State<'_, AppState>,
) -> Result<AuthStatus, String> {
    Ok(state.service.store_auth_status_memory_only().await)
}

#[tauri::command]
pub async fn warmup_keystore(
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.service.warmup_keystore().await;
    // After keystore is unlocked, seed oauth providers if this is the first install.
    let core = state.service.state.clone();
    tauri::async_runtime::spawn(async move {
        crate::commands::env_injection::seed_oauth_providers_if_empty(core).await;
    });
    Ok(())
}

#[tauri::command]
pub async fn store_get_access_token(
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    Ok(state.service.store_access_token().await)
}

#[tauri::command]
pub fn get_host_platform() -> String {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "arm_mac".to_string(),
        ("macos", "x86_64") => "intel_mac".to_string(),
        ("windows", _) => "win".to_string(),
        ("linux", _) => "linux".to_string(),
        (os, arch) => format!("{}_{}", os, arch),
    }
}

/// Returns macOS product version (e.g. "15.4.1") via sw_vers.
/// Non-macOS platforms return None.
#[tauri::command]
pub fn get_macos_version() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}
