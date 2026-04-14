use crate::AppState;
use nilbox_core::config_store::FunctionKeyRecord;
use tauri::State;

#[tauri::command]
pub async fn list_function_keys(
    state: State<'_, AppState>,
    vm_id: String,
) -> Result<Vec<FunctionKeyRecord>, String> {
    Ok(state.service.list_function_keys(&vm_id))
}

#[tauri::command]
pub async fn add_function_key(
    state: State<'_, AppState>,
    vm_id: String,
    label: String,
    bash: String,
) -> Result<(), String> {
    state.service.add_function_key(&vm_id, &label, &bash).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn remove_function_key(
    state: State<'_, AppState>,
    key_id: i64,
) -> Result<(), String> {
    state.service.remove_function_key(key_id).map_err(|e| e.to_string())
}
