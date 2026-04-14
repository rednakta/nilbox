use crate::AppState;
use nilbox_core::config_store::BlocklistLogEntry;
use tauri::State;

#[tauri::command]
pub async fn get_blocklist_logs(
    state: State<'_, AppState>,
    vm_id: String,
    limit: Option<i64>,
) -> Result<Vec<BlocklistLogEntry>, String> {
    state.service.state.config_store
        .get_block_logs(&vm_id, limit.unwrap_or(200))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn clear_blocklist_logs(
    state: State<'_, AppState>,
    vm_id: String,
) -> Result<(), String> {
    state.service.state.config_store
        .clear_block_logs(&vm_id)
        .map_err(|e| e.to_string())
}
