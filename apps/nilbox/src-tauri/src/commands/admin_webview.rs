//! Commands for managing an independent admin child WebviewWindow.
//! Opens as a normal resizable window — no coordinate syncing needed.

use tauri::{AppHandle, Manager, Url};
use tauri::webview::WebviewWindowBuilder;
use tauri::WebviewUrl;
use uuid::Uuid;

const ADMIN_WEBVIEW_LABEL_PREFIX: &str = "admin-";

/// Open the admin webview to the given URL in a new independent child window.
#[tauri::command]
pub async fn admin_webview_open(app: AppHandle, url: String, title: String) -> Result<String, String> {
    let parsed_url = Url::parse(&url).map_err(|e| e.to_string())?;
    let label = format!("{ADMIN_WEBVIEW_LABEL_PREFIX}{}", Uuid::new_v4().simple());

    let window_title = format!("[nilbox] {title}");
    WebviewWindowBuilder::new(
        &app,
        &label,
        WebviewUrl::External(parsed_url),
    )
    .title(&window_title)
    .inner_size(1000.0, 720.0)
    .min_inner_size(600.0, 400.0)
    .resizable(true)
    .decorations(true)
    .visible(true)
    .focused(true)
    .build()
    .map_err(|e| e.to_string())?;
    Ok(label)
}

/// Focus an existing admin webview window by label.
#[tauri::command]
pub async fn admin_webview_focus(app: AppHandle, label: String) -> Result<(), String> {
    let webview = app
        .get_webview_window(&label)
        .ok_or_else(|| format!("Admin window not found: {label}"))?;
    webview.show().map_err(|e| e.to_string())?;
    webview.set_focus().map_err(|e| e.to_string())?;
    Ok(())
}

/// Navigate the admin webview to a new URL.
#[tauri::command]
pub async fn admin_webview_navigate(app: AppHandle, label: String, url: String) -> Result<(), String> {
    let parsed_url = Url::parse(&url).map_err(|e| e.to_string())?;
    let webview = app
        .get_webview_window(&label)
        .ok_or_else(|| format!("Admin window not found: {label}"))?;
    webview.navigate(parsed_url).map_err(|e| e.to_string())?;
    Ok(())
}

/// Hide the admin webview without destroying it (keeps proxy alive).
#[tauri::command]
pub async fn admin_webview_hide(app: AppHandle, label: String) -> Result<(), String> {
    let webview = app
        .get_webview_window(&label)
        .ok_or_else(|| format!("Admin window not found: {label}"))?;
    webview.hide().map_err(|e| e.to_string())?;
    Ok(())
}

/// Reload the current page in the admin webview.
#[tauri::command]
pub async fn admin_webview_reload(app: AppHandle, label: String) -> Result<(), String> {
    let webview = app
        .get_webview_window(&label)
        .ok_or_else(|| format!("Admin window not found: {label}"))?;
    webview.eval("location.reload()").map_err(|e| e.to_string())?;
    Ok(())
}
