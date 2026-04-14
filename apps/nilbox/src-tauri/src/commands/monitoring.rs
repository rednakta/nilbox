use crate::AppState;
use nilbox_core::monitoring::VmMetrics;
use tauri::State;

#[tauri::command]
pub async fn get_vm_metrics(
    state: State<'_, AppState>,
) -> Result<VmMetrics, String> {
    Ok(state.service.get_vm_metrics())
}
