use crate::{AppState, PENDING_UPDATE_VERSION};
use nilbox_core::store::version_check::{UpdateInfo, UpdateSettings};
use nilbox_core::store::auth::ForceUpgradeInfo;
use tauri::State;
use tauri_plugin_updater::UpdaterExt;

#[tauri::command]
pub async fn check_for_update(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<UpdateInfo, String> {
    // Update last_update_check timestamp
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string();
    let _ = state.service.state.config_store.set_last_update_check(&now);

    // Use Tauri updater plugin
    let updater = app_handle.updater_builder().build()
        .map_err(|e| e.to_string())?;

    match updater.check().await {
        Ok(Some(update)) => {
            tracing::debug!("Update available: {}", update.version);
            Ok(UpdateInfo {
                available: true,
                version: update.version.clone(),
                notes: update.body.clone().unwrap_or_default(),
                date: update.date.map(|d| d.to_string()).unwrap_or_default(),
            })
        }
        Ok(None) => {
            tracing::debug!("Update check: no update available (server returned 204 or version is current)");
            Ok(UpdateInfo::none())
        }
        Err(e) => {
            tracing::error!("Update check error: {}", e);
            Err(e.to_string())
        }
    }
}

#[tauri::command]
pub async fn install_update(
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let updater = app_handle.updater_builder().build()
        .map_err(|e| format!("Updater build failed: {}", e))?;

    let update = updater.check().await
        .map_err(|e| format!("Update check failed: {}", e))?;

    match update {
        Some(upd) => {
            upd.download_and_install(|_, _| {}, || {}).await
                .map_err(|e| format!("Update install failed: {}", e))?;
            Ok(())
        }
        None => Err("No update available".to_string()),
    }
}

#[tauri::command]
pub async fn get_update_settings(
    state: State<'_, AppState>,
) -> Result<UpdateSettings, String> {
    let cs = &state.service.state.config_store;
    Ok(UpdateSettings {
        auto_update_check: cs.get_auto_update_check(),
        last_update_check: cs.get_last_update_check(),
    })
}

#[tauri::command]
pub async fn set_update_settings(
    state: State<'_, AppState>,
    auto_update_check: bool,
) -> Result<(), String> {
    state.service.state.config_store
        .set_auto_update_check(auto_update_check)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_pending_update() -> Option<String> {
    PENDING_UPDATE_VERSION.get().cloned()
}

#[tauri::command]
pub async fn get_developer_mode(
    state: State<'_, AppState>,
) -> Result<bool, String> {
    Ok(state.service.state.config_store.get_developer_mode())
}

#[tauri::command]
pub async fn set_developer_mode(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<(), String> {
    state.service.state.config_store
        .set_developer_mode(enabled)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_cdp_browser(
    state: State<'_, AppState>,
) -> Result<String, String> {
    Ok(state.service.state.config_store.get_cdp_browser())
}

#[tauri::command]
pub async fn set_cdp_browser(
    state: State<'_, AppState>,
    browser: String,
) -> Result<(), String> {
    state.service.state.config_store
        .set_cdp_browser(&browser)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_cdp_open_mode(
    state: State<'_, AppState>,
) -> Result<String, String> {
    Ok(state.service.state.config_store.get_cdp_open_mode())
}

#[tauri::command]
pub async fn set_cdp_open_mode(
    state: State<'_, AppState>,
    mode: String,
) -> Result<(), String> {
    state.service.state.config_store
        .set_cdp_open_mode(&mode)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_force_upgrade_info(
    state: State<'_, AppState>,
) -> Result<Option<ForceUpgradeInfo>, String> {
    Ok(state.service.state.store_auth.force_upgrade_info().await)
}
