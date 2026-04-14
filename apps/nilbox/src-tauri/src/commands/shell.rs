use crate::AppState;
use tauri::State;

#[tauri::command]
pub async fn open_oauth_url(
    state: State<'_, AppState>,
    vm_id: String,
    url: String,
) -> Result<(), String> {
    state.service.open_oauth_url_from_shell(&vm_id, &url).await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn open_shell(
    state: State<'_, AppState>,
    vm_id: String,
    cols: u32,
    rows: u32,
    install_url: Option<String>,
) -> Result<u64, String> {
    state.service.open_shell(&vm_id, cols, rows, install_url.as_deref()).await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn write_shell(
    state: State<'_, AppState>,
    session_id: u64,
    data: Vec<u8>,
) -> Result<(), String> {
    state.service.write_shell(session_id, data).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn resize_shell(
    state: State<'_, AppState>,
    session_id: u64,
    cols: u32,
    rows: u32,
) -> Result<(), String> {
    state.service.resize_shell(session_id, cols, rows).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn close_shell(
    state: State<'_, AppState>,
    session_id: u64,
) -> Result<(), String> {
    state.service.close_shell(session_id).await.map_err(|e| e.to_string())
}
