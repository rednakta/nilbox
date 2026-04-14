use crate::AppState;
use nilbox_core::config::PortMappingConfig;
use tauri::State;

#[tauri::command]
pub async fn add_port_mapping(
    state: State<'_, AppState>,
    vm_id: String,
    host_port: u16,
    vm_port: u16,
    label: String,
) -> Result<(), String> {
    state.service.add_mapping(&vm_id, host_port, vm_port, label).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn remove_port_mapping(
    state: State<'_, AppState>,
    host_port: u16,
) -> Result<(), String> {
    state.service.remove_mapping(host_port).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_port_mappings(
    state: State<'_, AppState>,
    vm_id: String,
) -> Result<Vec<PortMappingConfig>, String> {
    Ok(state.service.list_mappings(&vm_id).await)
}

#[tauri::command]
pub async fn open_admin_proxy(
    state: State<'_, AppState>,
    vm_id: String,
    vm_port: u16,
) -> Result<u16, String> {
    state.service.open_admin_proxy(&vm_id, vm_port).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn close_admin_proxy(
    state: State<'_, AppState>,
    host_port: u16,
) -> Result<(), String> {
    state.service.close_admin_proxy(host_port).await.map_err(|e| e.to_string())
}
