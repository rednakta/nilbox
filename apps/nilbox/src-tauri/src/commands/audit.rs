use crate::AppState;
use nilbox_core::audit::AuditEntry;
use tauri::State;

#[tauri::command]
pub async fn audit_query(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<AuditEntry>, String> {
    Ok(state.service.audit_query(limit).await)
}

#[tauri::command]
pub async fn audit_export_json(
    state: State<'_, AppState>,
) -> Result<Vec<u8>, String> {
    state.service.audit_export_json().await.map_err(|e| e.to_string())
}
