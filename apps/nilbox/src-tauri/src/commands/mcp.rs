use crate::AppState;
use nilbox_core::mcp_bridge::{McpServerConfig, McpServerInfo};
use tauri::State;

#[tauri::command]
pub async fn mcp_register(
    state: State<'_, AppState>,
    config: McpServerConfig,
) -> Result<String, String> {
    state.service.mcp_register(config).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn mcp_unregister(
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    state.service.mcp_unregister(&id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn mcp_list(
    state: State<'_, AppState>,
) -> Result<Vec<McpServerInfo>, String> {
    Ok(state.service.mcp_list().await)
}

#[tauri::command]
pub async fn mcp_generate_claude_config(
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    Ok(state.service.mcp_generate_claude_config().await)
}
