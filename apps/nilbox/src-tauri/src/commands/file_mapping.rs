use crate::AppState;
use nilbox_core::config_store::FileMappingRecord;
use tauri::State;

// ── Existing FUSE path control commands ──────────────────────

#[tauri::command]
pub async fn change_shared_path(
    state: State<'_, AppState>,
    vm_id: String,
    mapping_id: i64,
    new_path: String,
) -> Result<bool, String> {
    state.service
        .change_shared_path(&vm_id, mapping_id, std::path::PathBuf::from(new_path))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_path_state(
    state: State<'_, AppState>,
    vm_id: String,
    mapping_id: i64,
) -> Result<(String, String), String> {
    state.service
        .get_path_state(&vm_id, mapping_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn force_switch_path(
    state: State<'_, AppState>,
    vm_id: String,
    mapping_id: i64,
) -> Result<(), String> {
    state.service
        .force_switch_path(&vm_id, mapping_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cancel_path_change(
    state: State<'_, AppState>,
    vm_id: String,
    mapping_id: i64,
) -> Result<(), String> {
    state.service
        .cancel_path_change(&vm_id, mapping_id)
        .await
        .map_err(|e| e.to_string())
}

// ── File Mapping CRUD commands ──────────────────────────────

#[tauri::command]
pub async fn list_file_mappings(
    state: State<'_, AppState>,
    vm_id: String,
) -> Result<Vec<FileMappingRecord>, String> {
    Ok(state.service.list_file_mappings(&vm_id).await)
}

#[tauri::command]
pub async fn add_file_mapping(
    state: State<'_, AppState>,
    vm_id: String,
    host_path: String,
    vm_mount: String,
    read_only: bool,
    label: String,
) -> Result<(), String> {
    state.service
        .add_file_mapping(&vm_id, &host_path, &vm_mount, read_only, &label)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn remove_file_mapping(
    state: State<'_, AppState>,
    vm_id: String,
    mapping_id: i64,
) -> Result<(), String> {
    state.service
        .remove_file_mapping(&vm_id, mapping_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn force_unmount_file_proxy(
    state: State<'_, AppState>,
    vm_id: String,
    mapping_id: i64,
) -> Result<(), String> {
    state.service
        .force_unmount_file_proxy(&vm_id, mapping_id)
        .await
        .map_err(|e| e.to_string())
}
