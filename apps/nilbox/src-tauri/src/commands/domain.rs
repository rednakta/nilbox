use crate::AppState;
use nilbox_core::config_store::{AllowlistEntry, AwsProxyRoute, InspectMode};
use tauri::State;

#[tauri::command]
pub async fn resolve_domain_access(
    state: State<'_, AppState>,
    domain: String,
    action: String,
    env_names: Vec<String>,
) -> Result<(), String> {
    state.service.resolve_domain_access(&domain, action, env_names).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn resolve_token_mismatch(
    state: State<'_, AppState>,
    request_id: String,
    action: String,
) -> Result<(), String> {
    state.service.resolve_token_mismatch(request_id, action).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn add_allowlist_domain(
    state: State<'_, AppState>,
    domain: String,
    inspect_mode: Option<String>,
) -> Result<(), String> {
    let mode = inspect_mode
        .as_deref()
        .map(InspectMode::from_str)
        .unwrap_or(InspectMode::Inspect);
    state.service.add_allowlist_domain_with_mode(&domain, mode).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn add_denylist_domain(
    state: State<'_, AppState>,
    domain: String,
) -> Result<(), String> {
    state.service.add_denylist_domain(&domain).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn remove_allowlist_domain(
    state: State<'_, AppState>,
    domain: String,
) -> Result<(), String> {
    state.service.remove_allowlist_domain(&domain).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_allowlist_domains(
    state: State<'_, AppState>,
) -> Result<Vec<String>, String> {
    Ok(state.service.list_allowlist_domains().await)
}

#[tauri::command]
pub async fn remove_denylist_domain(
    state: State<'_, AppState>,
    domain: String,
) -> Result<(), String> {
    state.service.remove_denylist_domain(&domain).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_denylist_domains(
    state: State<'_, AppState>,
) -> Result<Vec<String>, String> {
    Ok(state.service.list_denylist_domains().await)
}

#[tauri::command]
pub async fn list_allowlist_entries(
    state: State<'_, AppState>,
) -> Result<Vec<AllowlistEntry>, String> {
    Ok(state.service.list_allowlist_entries())
}

#[tauri::command]
pub async fn count_allowlist_entries(
    state: State<'_, AppState>,
) -> Result<u32, String> {
    Ok(state.service.count_allowlist_entries())
}

#[tauri::command]
pub async fn list_allowlist_entries_paginated(
    state: State<'_, AppState>,
    page: u32,
    page_size: u32,
) -> Result<Vec<AllowlistEntry>, String> {
    Ok(state.service.list_allowlist_entries_paginated(page, page_size))
}

#[tauri::command]
pub async fn add_domain_token_account(
    state: State<'_, AppState>,
    domain: String,
    token_account: String,
    token_value: String,
) -> Result<(), String> {
    state.service
        .add_domain_token_account(&domain, &token_account, &token_value)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn remove_domain_token_account(
    state: State<'_, AppState>,
    domain: String,
    token_account: String,
) -> Result<(), String> {
    state.service
        .remove_domain_token_account(&domain, &token_account)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn map_env_to_domain(
    state: State<'_, AppState>,
    domain: String,
    env_name: String,
) -> Result<(), String> {
    state.service
        .map_env_to_domain(&domain, &env_name)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn unmap_env_from_domain(
    state: State<'_, AppState>,
    domain: String,
    env_name: String,
) -> Result<(), String> {
    state.service
        .unmap_env_from_domain(&domain, &env_name)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_domain_env_mappings(
    state: State<'_, AppState>,
    domain: String,
    env_names: Vec<String>,
) -> Result<(), String> {
    state.service
        .set_domain_env_mappings(&domain, env_names)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn add_aws_proxy_route(
    state: State<'_, AppState>,
    path_prefix: String,
    aws_host: String,
) -> Result<(), String> {
    state.service.state.config_store
        .add_aws_proxy_route(&path_prefix, &aws_host)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_aws_proxy_routes(
    state: State<'_, AppState>,
) -> Result<Vec<AwsProxyRoute>, String> {
    state.service.state.config_store
        .list_aws_proxy_routes()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn remove_aws_proxy_route(
    state: State<'_, AppState>,
    path_prefix: String,
) -> Result<(), String> {
    state.service.state.config_store
        .remove_aws_proxy_route(&path_prefix)
        .map_err(|e| e.to_string())
}
