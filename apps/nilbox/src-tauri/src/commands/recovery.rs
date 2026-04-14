use crate::AppState;
use nilbox_core::recovery::RecoveryState;
use tauri::State;

#[tauri::command]
pub async fn recovery_enable(
    state: State<'_, AppState>,
    vm_id: String,
) -> Result<(), String> {
    state.service.recovery_enable(&vm_id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn recovery_disable(
    state: State<'_, AppState>,
    vm_id: String,
) -> Result<(), String> {
    state.service.recovery_disable(&vm_id).await;
    Ok(())
}

#[tauri::command]
pub async fn recovery_status(
    state: State<'_, AppState>,
    vm_id: String,
) -> Result<RecoveryState, String> {
    Ok(state.service.recovery_status(&vm_id).await)
}
