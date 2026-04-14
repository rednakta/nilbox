use crate::AppState;
use tauri::State;

#[tauri::command]
pub async fn ssh_gateway_enable(
    state: State<'_, AppState>,
    vm_id: String,
    host_port: u16,
) -> Result<(), String> {
    state.service.ssh_gateway_enable(&vm_id, host_port).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn ssh_gateway_disable(
    state: State<'_, AppState>,
    vm_id: String,
) -> Result<(), String> {
    state.service.ssh_gateway_disable(&vm_id).await;
    Ok(())
}

#[tauri::command]
pub async fn ssh_gateway_status(
    state: State<'_, AppState>,
    vm_id: String,
) -> Result<Option<u16>, String> {
    Ok(state.service.ssh_gateway_status(&vm_id).await)
}
