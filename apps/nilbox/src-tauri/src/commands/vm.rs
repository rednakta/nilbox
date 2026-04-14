use crate::AppState;
use nilbox_core::state::VmInfo;
use nilbox_core::vm_platform::{VmConfig, VmStatus};
use nilbox_core::vm_install::CachedImageInfo;
use tauri::State;

#[tauri::command]
pub async fn vm_install_from_manifest_url(
    state: State<'_, AppState>,
    url: String,
) -> Result<String, String> {
    state
        .service
        .install_vm_from_manifest_url(&url)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn vm_install_from_cache(
    state: State<'_, AppState>,
    app_id: String,
) -> Result<String, String> {
    state
        .service
        .install_vm_from_cache(&app_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_cached_os_images(
    state: State<'_, AppState>,
) -> Result<Vec<CachedImageInfo>, String> {
    Ok(state.service.list_cached_os_images())
}

#[tauri::command]
pub async fn create_vm(
    state: State<'_, AppState>,
    name: String,
    config: VmConfig,
) -> Result<String, String> {
    state.service.create_vm(name, config).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_vm(
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    state.service.delete_vm(&id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn select_vm(
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    state.service.select_vm(&id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_vms(
    state: State<'_, AppState>,
) -> Result<Vec<VmInfo>, String> {
    Ok(state.service.list_vms().await)
}

#[tauri::command]
pub async fn start_vm(
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    state.service.start_vm(&id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn stop_vm(
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    state.service.stop_vm(&id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn vm_status(
    state: State<'_, AppState>,
    id: String,
) -> Result<VmStatus, String> {
    state.service.vm_status(&id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn add_vm_admin_url(
    state: State<'_, AppState>,
    vm_id: String,
    url: String,
    label: String,
) -> Result<i64, String> {
    state.service.add_vm_admin_url(&vm_id, url, label)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn remove_vm_admin_url(
    state: State<'_, AppState>,
    vm_id: String,
    url_id: i64,
) -> Result<(), String> {
    state.service.remove_vm_admin_url(&vm_id, url_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_vm_disk_size(
    state: State<'_, AppState>,
    id: String,
) -> Result<u64, String> {
    state.service.get_vm_disk_size(&id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn resize_vm_disk(
    state: State<'_, AppState>,
    id: String,
    new_size_gb: u32,
) -> Result<u64, String> {
    state.service.resize_vm_disk(&id, new_size_gb).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_vm_fs_info(
    state: State<'_, AppState>,
    id: String,
) -> Result<nilbox_core::service::VmFsInfo, String> {
    state.service.get_vm_fs_info(&id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn expand_vm_partition(
    state: State<'_, AppState>,
    id: String,
) -> Result<String, String> {
    state.service.expand_vm_partition(&id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_vm_memory(
    state: State<'_, AppState>,
    id: String,
    memory_mb: u32,
) -> Result<(), String> {
    state.service.update_vm_memory(&id, memory_mb).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_vm_cpus(
    state: State<'_, AppState>,
    id: String,
    cpus: u32,
) -> Result<(), String> {
    state.service.update_vm_cpus(&id, cpus).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_vm_name(
    state: State<'_, AppState>,
    id: String,
    name: String,
) -> Result<(), String> {
    state.service.update_vm_name(&id, name).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_vm_description(
    state: State<'_, AppState>,
    id: String,
    description: Option<String>,
) -> Result<(), String> {
    state.service.update_vm_description(&id, description).await.map_err(|e| e.to_string())
}
