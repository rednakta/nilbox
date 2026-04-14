use crate::AppState;
use tauri::State;

#[tauri::command]
pub async fn set_api_key(
    state: State<'_, AppState>,
    account: String,
    key: String,
) -> Result<(), String> {
    state.service.set_api_key(&account, &key).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_api_key(
    state: State<'_, AppState>,
    account: String,
) -> Result<(), String> {
    state.service.delete_api_key(&account).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_api_keys(
    state: State<'_, AppState>,
) -> Result<Vec<String>, String> {
    state.service.list_api_keys().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn has_api_key(
    state: State<'_, AppState>,
    account: String,
) -> Result<bool, String> {
    state.service.has_api_key(&account).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn resolve_api_key_request(
    state: State<'_, AppState>,
    account: String,
    key: Option<String>,
) -> Result<(), String> {
    state.service.resolve_api_key_request(&account, key).await;
    Ok(())
}
