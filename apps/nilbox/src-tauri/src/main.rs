#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod commands;

use nilbox_core::audit::AuditLog;
use nilbox_core::config_store::ConfigStore;
use nilbox_core::events::EventEmitter;
use nilbox_core::gateway::Gateway;
use nilbox_core::keystore;
use nilbox_core::mcp_bridge::McpBridgeManager;
use nilbox_core::monitoring::MonitoringCollector;
use nilbox_core::proxy::api_key_gate::ApiKeyGate;
use nilbox_core::proxy::auth_delegator::bearer::BearerDelegator;
use nilbox_core::proxy::auth_router::AuthRouter;
use nilbox_core::proxy::domain_gate::DomainGate;
use nilbox_core::proxy::token_mismatch_gate::TokenMismatchGate;
use nilbox_core::proxy::llm_detector::LlmProviderMatcher;
use nilbox_core::proxy::oauth_token_vault::OAuthTokenVault;
use nilbox_core::recovery::RecoveryManager;
use nilbox_core::service::NilBoxService;
use nilbox_core::ssh_gateway::SshGateway;
use nilbox_core::state::CoreState;
use nilbox_core::store::StoreManager;
use nilbox_core::store::auth::StoreAuth;
use nilbox_core::store::client::StoreClient;
use nilbox_core::store::STORE_BASE_URL;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::sync::Arc;
use tauri::Manager;
use tauri::Emitter;
use tokio::sync::RwLock;
use tracing::{warn, debug};

static IS_QUITTING: AtomicBool = AtomicBool::new(false);
static INIT_DONE: AtomicBool = AtomicBool::new(false);
static CLOSE_REQUEST_TX: OnceLock<tokio::sync::mpsc::UnboundedSender<()>> = OnceLock::new();
static PENDING_UPDATE_VERSION: OnceLock<String> = OnceLock::new();

// ── Tauri Event Emitter ──────────────────────────────────────

struct TauriEventEmitter {
    app_handle: tauri::AppHandle,
}

impl TauriEventEmitter {
    fn new(app_handle: tauri::AppHandle) -> Self {
        Self { app_handle }
    }
}

impl EventEmitter for TauriEventEmitter {
    fn emit(&self, event: &str, payload: &str) {
        // payload is a pre-serialized JSON string (from emit_typed).
        // Parse it back to a Value so Tauri doesn't double-encode it as a JSON string.
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(payload) {
            let _ = self.app_handle.emit(event, val);
        } else {
            let _ = self.app_handle.emit(event, payload);
        }
    }

    fn emit_bytes(&self, event: &str, payload: &[u8]) {
        let _ = self.app_handle.emit(event, payload.to_vec());
    }
}

// ── App State ────────────────────────────────────────────────

pub struct AppState {
    pub service: Arc<NilBoxService>,
}

// ── Main ─────────────────────────────────────────────────────

#[tauri::command]
fn quit_app(app_handle: tauri::AppHandle) {
    IS_QUITTING.store(true, Ordering::SeqCst);
    app_handle.exit(0);
}

#[cfg(target_os = "macos")]
extern "C" fn app_should_terminate(
    _this: &objc::runtime::Object,
    _sel: objc::runtime::Sel,
    _sender: *mut objc::runtime::Object,
) -> usize {
    if IS_QUITTING.load(Ordering::SeqCst) {
        1 // NSTerminateNow — allow termination
    } else {
        // Use channel send (panic-free) instead of calling emit directly
        // from an ObjC callback. A background task forwards this to the frontend.
        if let Some(tx) = CLOSE_REQUEST_TX.get() {
            let _ = tx.send(());
        }
        0 // NSTerminateCancel — block termination
    }
}

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
fn install_terminate_handler() {
    use objc::declare::ClassDecl;
    use objc::runtime::{Class, Object, Sel};
    use objc::{class, msg_send, sel, sel_impl};

    // `setClass:` is not a valid ObjC message — use the C runtime function directly.
    extern "C" {
        fn object_setClass(obj: *mut Object, cls: *const Class) -> *const Class;
    }

    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let delegate: *mut Object = msg_send![app, delegate];
        if delegate.is_null() {
            return;
        }

        let delegate_class: *const Class = msg_send![delegate, class];
        if delegate_class.is_null() {
            return;
        }

        if let Some(mut decl) = ClassDecl::new("NilBoxTerminateInterceptor", &*delegate_class) {
            decl.add_method(
                sel!(applicationShouldTerminate:),
                app_should_terminate as extern "C" fn(&Object, Sel, *mut Object) -> usize,
            );
            let new_class = decl.register();
            object_setClass(delegate, new_class as *const Class);
        }
    }
}

#[tauri::command]
fn is_initialized() -> bool {
    INIT_DONE.load(Ordering::Relaxed)
}

fn main() {
    // Parse CLI flags before anything else
    let args: Vec<String> = std::env::args().collect();
    let reset_guide = args.iter().any(|a| a == "--reset-guide");

    // Install rustls ring CryptoProvider before any TLS clients are created.
    // Required for reqwest 0.12 with rustls-tls-manual-roots.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let default_filter = if cfg!(debug_assertions) {
        "nilbox=debug,nilbox_core=debug"
    } else {
        "nilbox=info,nilbox_core=info"
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| default_filter.into()),
        )
        .init();

    // Production: serve frontend via HTTP localhost (port 14580) instead of tauri://
    // custom protocol. WKWebView blocks iframe loopback HTTPS in non-Safari apps
    // when using custom protocols, so HTTP origin is needed for Store iframe.
    #[cfg(not(debug_assertions))]
    const LOCALHOST_PORT: u16 = 14580;

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_http::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_notification::init());

    #[cfg(not(debug_assertions))]
    let builder = builder.plugin(tauri_plugin_localhost::Builder::new(LOCALHOST_PORT).build());

    let builder = builder
        .setup(move |app| {
            // Build macOS app menu inside setup (needs &App reference)
            #[cfg(target_os = "macos")]
            {
                use tauri::menu::{MenuBuilder, SubmenuBuilder, PredefinedMenuItem, MenuItemBuilder};

                let quit_item = MenuItemBuilder::with_id("quit_app", "Quit nilbox")
                    .accelerator("Cmd+Q")
                    .build(app)?;

                let app_submenu = SubmenuBuilder::new(app, "nilbox")
                    .item(&PredefinedMenuItem::hide(app, None)?)
                    .item(&PredefinedMenuItem::hide_others(app, None)?)
                    .item(&PredefinedMenuItem::show_all(app, None)?)
                    .separator()
                    .item(&quit_item)
                    .build()?;

                let edit_submenu = SubmenuBuilder::new(app, "Edit")
                    .item(&PredefinedMenuItem::undo(app, None)?)
                    .item(&PredefinedMenuItem::redo(app, None)?)
                    .separator()
                    .item(&PredefinedMenuItem::cut(app, None)?)
                    .item(&PredefinedMenuItem::copy(app, None)?)
                    .item(&PredefinedMenuItem::paste(app, None)?)
                    .item(&PredefinedMenuItem::select_all(app, None)?)
                    .build()?;

                let menu = MenuBuilder::new(app)
                    .item(&app_submenu)
                    .item(&edit_submenu)
                    .build()?;

                app.set_menu(menu)?;

                app.on_menu_event(move |app_handle, event| {
                    if event.id().as_ref() == "quit_app" {
                        let _ = app_handle.emit("app-close-requested", ());
                    }
                });
            }

            // Kill any orphaned nilbox-vmm processes from a previous session
            #[cfg(target_os = "macos")]
            {
                let _ = std::process::Command::new("killall")
                    .arg("nilbox-vmm")
                    .status();
                debug!("Checked for orphaned nilbox-vmm processes");
            }

            let app_data_dir = app.path().app_data_dir()
                .expect("Failed to resolve app data dir");

            debug!("App data directory: {:?}", app_data_dir);
            std::fs::create_dir_all(&app_data_dir).expect("Failed to create app data dir");

            // 1. Initialize ConfigStore (SQLite)
            let db_path = ConfigStore::db_path(&app_data_dir);
            let config_store = Arc::new(
                ConfigStore::open(&db_path).expect("Failed to open config database")
            );

            // 2. Migrate from legacy config.json if it exists
            let json_path = ConfigStore::legacy_json_path(&app_data_dir);
            match config_store.migrate_from_json(&json_path) {
                Ok(true) => debug!("Migrated config.json → SQLite (config.json.bak created)"),
                Ok(false) => debug!("No migration needed"),
                Err(e) => tracing::error!("Config migration failed: {}", e),
            }

            // 3. Seed default allowlist on first launch, then load whitelist mode
            config_store.seed_default_allowlist().unwrap_or_else(|e| {
                tracing::error!("Failed to seed default allowlist: {}", e);
            });

            // Initialize keystore
            let keystore = keystore::create_keystore(&app_data_dir);

            // Build emitter early (needed for DomainGate)
            let app_handle = app.handle().clone();
            let emitter: Arc<dyn EventEmitter> =
                Arc::new(TauriEventEmitter::new(app_handle.clone()));

            // 3b. Build DomainGate (needs config_store + emitter)
            let domain_gate = Arc::new(DomainGate::new(config_store.clone(), emitter.clone()));

            // 3b2. Build TokenMismatchGate (needs emitter)
            let token_mismatch_gate = Arc::new(TokenMismatchGate::new(emitter.clone()));

            // 3c. Build ApiKeyGate (needs keystore + emitter)
            let api_key_gate = Arc::new(ApiKeyGate::new(keystore.clone(), emitter.clone()));

            // 3d. Build OAuthTokenVault + AuthRouter with BearerDelegator as default
            let oauth_vault = Arc::new(OAuthTokenVault::new(keystore.clone()));
            let bearer_delegator = Arc::new(BearerDelegator::new(api_key_gate.clone(), Some(oauth_vault.clone())));
            let auth_router = Arc::new(AuthRouter::new(bearer_delegator));

            // 4. Build store auth + client
            let store_auth = Arc::new(StoreAuth::new(STORE_BASE_URL, keystore.clone()));
            let store_client = Arc::new(StoreClient::new(STORE_BASE_URL, store_auth.clone()));

            // 5. Resolve bundled QEMU binary path (Windows/Linux only)
            #[cfg(target_os = "windows")]
            let qemu_binary_path = std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|d| d.to_path_buf()))
                .and_then(|d| {
                    // Tauri externalBin places the exe at the app root
                    let root = d.join("qemu-system-x86_64.exe");
                    if root.exists() { return Some(root); }
                    // Dev build: binaries/windows/ subdirectory
                    let sub = d.join("windows").join("qemu-system-x86_64.exe");
                    if sub.exists() { return Some(sub); }
                    None
                });

            #[cfg(target_os = "linux")]
            let qemu_binary_path = {
                let sidecar_name = format!(
                    "qemu-system-x86_64-{}-unknown-linux-gnu",
                    std::env::consts::ARCH
                );
                // 1. Next to the current exe (production bundle)
                let beside_exe = std::env::current_exe().ok().and_then(|exe| {
                    exe.parent().map(|d| {
                        // Try exact name first, then sidecar-style name
                        let p1 = d.join("linux").join("qemu-system-x86_64");
                        if p1.exists() { return p1; }
                        let p2 = d.join("linux").join(&sidecar_name);
                        if p2.exists() { return p2; }
                        p1 // fallback (won't pass exists filter)
                    })
                }).filter(|p| p.exists());

                if beside_exe.is_some() {
                    beside_exe
                } else {
                    // 2. Dev environment: src-tauri/binaries/linux/
                    let dev_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .join("binaries")
                        .join("linux")
                        .join(&sidecar_name);
                    if dev_path.exists() {
                        Some(dev_path)
                    } else {
                        None
                    }
                }
            };

            // 6. Build CoreState (with config_store)
            let core_state = Arc::new(CoreState {
                vms: RwLock::new(HashMap::new()),
                active_vm: RwLock::new(None),
                keystore: keystore.clone(),
                gateway: Arc::new(Gateway::new()),
                config_store: config_store.clone(),
                app_data_dir,
                ssh_keys: Arc::new(tokio::sync::OnceCell::new()),
                api_key_gate,
                auth_router,
                store: Arc::new(StoreManager::new()),
                store_auth: store_auth.clone(),
                store_client,
                mcp_bridge: Arc::new(McpBridgeManager::new()),
                monitoring: Arc::new(MonitoringCollector::new()),
                audit_log: Arc::new(AuditLog::new()),
                recovery: Arc::new(RecoveryManager::new()),
                ssh_gateway: Arc::new(SshGateway::new()),
                domain_gate,
                token_mismatch_gate,
                oauth_engine: Arc::new(tokio::sync::RwLock::new(Arc::new(
                    nilbox_core::proxy::oauth_script_engine::OAuthScriptEngine::empty()
                ))),
                oauth_vault,
                llm_matcher: Arc::new(
                    LlmProviderMatcher::new_free_mode(keystore, config_store.clone())
                ),
                #[cfg(not(target_os = "macos"))]
                qemu_binary_path,
            });

            let service = Arc::new(NilBoxService::new(core_state, emitter));

            // Register managed state
            let service_clone     = service.clone();
            let app_handle_for_update = app_handle.clone();
            app_handle.manage(AppState { service });

            // 5. Load VMs from DB (no keyring access at startup)
            tauri::async_runtime::spawn(async move {
                // NOTE: cleanup_orphan_tokens and try_restore are deferred —
                // both access the OS keyring and would show a password dialog
                // before the window appears. cleanup runs lazily on first VM start;
                // store auth is restored on demand when the store tab is opened.

                // ── Critical path: register VMs and notify frontend ASAP ──
                let vm_records = service_clone.state.config_store.list_vms()
                    .unwrap_or_default();

                for record in &vm_records {
                    if record.disk_image.is_empty() {
                        continue;
                    }
                    match service_clone.register_vm(record).await {
                        Ok(id) => debug!("VM registered from DB: {} ({})", record.name, id),
                        Err(e) => tracing::error!("Failed to register VM {}: {}", record.name, e),
                    }
                }

                if vm_records.is_empty() {
                    debug!("No VMs in database — user can create one from the UI");
                }

                // Notify frontend immediately — everything below is non-critical.
                INIT_DONE.store(true, Ordering::Relaxed);
                let _ = app_handle_for_update.emit("vms-loaded", ());

                // ── Deferred tasks (do NOT block UI readiness) ──
                // SAFETY: window is already visible at this point (created in
                // setup() before this async task runs). The sleep guarantees the
                // Keychain password dialog never appears before the window.
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;

                // LLM provider reload triggers lazy keystore init (macOS Keychain).
                // Must run AFTER window is visible — keystore access may show
                // a macOS Keychain password prompt.
                if let Err(e) = service_clone.state.llm_matcher.reload().await {
                    warn!("Failed to load LLM providers from DB at startup: {}", e);
                }
                service_clone.load_blocklist_on_startup().await;
                service_clone.run_token_usage_maintenance().await;

                // If --reset-guide was passed, notify frontend to clear guide hints
                if reset_guide {
                    debug!("--reset-guide flag detected, resetting guide hints");
                    let _ = app_handle_for_update.emit("reset-guide", ());
                }

                // Auto-update check on startup (if enabled)
                if service_clone.state.config_store.get_auto_update_check() {
                    use tauri_plugin_updater::UpdaterExt;
                    let current_ver = app_handle_for_update.package_info().version.to_string();
                    debug!("Auto update check: current version = {}", current_ver);
                    match app_handle_for_update.updater_builder().build() {
                        Ok(updater) => {
                            match updater.check().await {
                                Ok(Some(update)) => {
                                    debug!("Update available: {}", update.version);
                                    let _ = PENDING_UPDATE_VERSION.set(update.version.clone());
                                    let _ = app_handle_for_update.emit("update-available", serde_json::json!({
                                        "version": update.version,
                                        "notes": update.body.clone().unwrap_or_default(),
                                        "date": update.date.map(|d| d.to_string()).unwrap_or_default(),
                                    }));
                                }
                                Ok(None) => debug!("App is up to date (remote version not newer than {})", current_ver),
                                Err(e) => debug!("Auto update check failed (non-fatal): {}", e),
                            }
                        }
                        Err(e) => debug!("Failed to build updater (non-fatal): {}", e),
                    }
                }
            });

            // Set up channel so the ObjC callback can safely notify the frontend.
            // emit must not be called directly from an extern "C" ObjC callback.
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
            let _ = CLOSE_REQUEST_TX.set(tx);
            let handle_for_close = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                while rx.recv().await.is_some() {
                    let _ = handle_for_close.emit("app-close-requested", ());
                }
            });

            #[cfg(target_os = "macos")]
            install_terminate_handler();

            // Create main window programmatically.
            // Production: HTTP localhost origin (via tauri-plugin-localhost) so
            // iframes to external HTTPS sites work in WKWebView.
            // Dev: App URL, which Tauri maps to devUrl automatically.
            {
                use tauri::webview::WebviewWindowBuilder;
                use tauri::WebviewUrl;

                #[cfg(not(debug_assertions))]
                let url = WebviewUrl::External(
                    format!("http://localhost:{}", LOCALHOST_PORT).parse().unwrap()
                );

                #[cfg(debug_assertions)]
                let url = WebviewUrl::App("index.html".into());

                let mut win_builder = WebviewWindowBuilder::new(app, "main", url)
                    .title("nilbox")
                    .inner_size(1200.0, 800.0)
                    .min_inner_size(800.0, 600.0)
                    .resizable(true)
                    .decorations(false)
                    .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.3 Safari/605.1.15");

                // macOS: first click after window focus switch should register
                // as a click, not just bring the window to front.
                #[cfg(target_os = "macos")]
                {
                    win_builder = win_builder.accept_first_mouse(true);
                }

                // macOS Sequoia WKWebView: touch event listeners interfere with
                // mouse click handling, causing intermittent click failures.
                // Block touch events and one-shot capture-click listeners that
                // WKWebView injects for touch-emulation.
                // See: https://github.com/tauri-apps/tauri/discussions/11957
                #[cfg(target_os = "macos")]
                {
                    win_builder = win_builder.initialization_script(r#"
                        (function() {
                            var orig = EventTarget.prototype.addEventListener;
                            EventTarget.prototype.addEventListener = function(type, fn, opts) {
                                if (type === 'touchstart' || type === 'touchend' || type === 'touchmove') return;
                                return orig.call(this, type, fn, opts);
                            };
                        })();
                    "#);
                }

                let win = win_builder.build()?;

                // On Linux (Wayland/GNOME), set_icon() has no effect.
                // GNOME resolves the app icon via the .desktop file + icon theme.
                // Install icon and .desktop entry to the user's local directories
                // so the correct icon appears in the alt-tab switcher and dock.
                #[cfg(target_os = "linux")]
                {
                    let _ = win.set_icon(tauri::include_image!("icons/icon.png"));
                    linux_install_desktop_entry();
                }
            }

            debug!("NilBox initialized successfully");
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.emit("app-close-requested", ());
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            is_initialized,
            // VM
            commands::vm::create_vm,
            commands::vm::delete_vm,
            commands::vm::select_vm,
            commands::vm::list_vms,
            commands::vm::start_vm,
            commands::vm::stop_vm,
            commands::vm::vm_status,
            commands::vm::vm_install_from_manifest_url,
            commands::vm::vm_install_from_cache,
            commands::vm::list_cached_os_images,
            commands::vm::add_vm_admin_url,
            commands::vm::remove_vm_admin_url,
            commands::vm::get_vm_disk_size,
            commands::vm::resize_vm_disk,
            commands::vm::get_vm_fs_info,
            commands::vm::expand_vm_partition,
            commands::vm::update_vm_memory,
            commands::vm::update_vm_cpus,
            commands::vm::update_vm_name,
            commands::vm::update_vm_description,
            // Shell
            commands::shell::open_oauth_url,
            commands::shell::open_shell,
            commands::shell::write_shell,
            commands::shell::resize_shell,
            commands::shell::close_shell,
            // Port Mapping
            commands::port_mapping::add_port_mapping,
            commands::port_mapping::remove_port_mapping,
            commands::port_mapping::list_port_mappings,
            commands::port_mapping::open_admin_proxy,
            commands::port_mapping::close_admin_proxy,
            // API Key
            commands::api_key::set_api_key,
            commands::api_key::delete_api_key,
            commands::api_key::list_api_keys,
            commands::api_key::has_api_key,
            commands::api_key::resolve_api_key_request,
            // Domain
            commands::domain::resolve_domain_access,
            commands::domain::resolve_token_mismatch,
            commands::domain::add_allowlist_domain,
            commands::domain::add_denylist_domain,
            commands::domain::remove_allowlist_domain,
            commands::domain::list_allowlist_domains,
            commands::domain::remove_denylist_domain,
            commands::domain::list_denylist_domains,
            commands::domain::list_allowlist_entries,
            commands::domain::count_allowlist_entries,
            commands::domain::list_allowlist_entries_paginated,
            commands::domain::add_domain_token_account,
            commands::domain::remove_domain_token_account,
            commands::domain::map_env_to_domain,
            commands::domain::unmap_env_from_domain,
            commands::domain::set_domain_env_mappings,
            commands::domain::add_aws_proxy_route,
            commands::domain::list_aws_proxy_routes,
            commands::domain::remove_aws_proxy_route,
            // Store
            commands::store::store_list_catalog,
            commands::store::store_install,
            commands::store::store_uninstall,
            commands::store::store_list_installed,
            commands::store::store_register_install,
            commands::store::store_begin_login,
            commands::store::store_begin_login_browser,
            commands::store::store_cancel_login,
            commands::store::store_login,
            commands::store::store_logout,
            commands::store::store_auth_status,
            commands::store::store_check_auth_status,
            commands::store::warmup_keystore,
            commands::store::store_get_access_token,
            commands::store::get_host_platform,
            commands::store::get_macos_version,
            // MCP
            commands::mcp::mcp_register,
            commands::mcp::mcp_unregister,
            commands::mcp::mcp_list,
            commands::mcp::mcp_generate_claude_config,
            // Monitoring
            commands::monitoring::get_vm_metrics,
            // Audit
            commands::audit::audit_query,
            commands::audit::audit_export_json,
            // Recovery
            commands::recovery::recovery_enable,
            commands::recovery::recovery_disable,
            commands::recovery::recovery_status,
            // SSH Gateway
            commands::ssh_gateway::ssh_gateway_enable,
            commands::ssh_gateway::ssh_gateway_disable,
            commands::ssh_gateway::ssh_gateway_status,
            // File Mapping (FUSE)
            commands::file_mapping::change_shared_path,
            commands::file_mapping::get_path_state,
            commands::file_mapping::force_switch_path,
            commands::file_mapping::cancel_path_change,
            commands::file_mapping::list_file_mappings,
            commands::file_mapping::add_file_mapping,
            commands::file_mapping::remove_file_mapping,
            commands::file_mapping::force_unmount_file_proxy,
            // Function Key
            commands::function_key::list_function_keys,
            commands::function_key::add_function_key,
            commands::function_key::remove_function_key,
            // Env Injection
            commands::env_injection::list_env_providers,
            commands::env_injection::list_env_entries,
            commands::env_injection::set_env_entry_enabled,
            commands::env_injection::add_custom_env_entry,
            commands::env_injection::remove_custom_env_entry,
            commands::env_injection::apply_env_injection,
            commands::env_injection::delete_env_provider,
            commands::env_injection::update_env_providers_from_store,
            // OAuth Providers
            commands::env_injection::list_oauth_providers,
            commands::env_injection::update_oauth_providers_from_store,
// Custom OAuth Providers
            commands::env_injection::save_custom_oauth_provider,
            commands::env_injection::delete_custom_oauth_provider,
            commands::env_injection::validate_oauth_script,
            // OAuth Sessions
            commands::env_injection::list_oauth_sessions,
            commands::env_injection::delete_oauth_session,
            commands::env_injection::delete_all_oauth_sessions,
            // Blocklist Log
            commands::blocklist_log::get_blocklist_logs,
            commands::blocklist_log::clear_blocklist_logs,
            // Token Usage / LLM Monitor
            commands::token_usage::get_token_usage_monthly,
            commands::token_usage::get_token_usage_daily,
            commands::token_usage::get_token_usage_logs,
            commands::token_usage::count_token_usage_logs,
            commands::token_usage::get_token_usage_date_range,
            commands::token_usage::get_token_usage_daily_for_week,
            commands::token_usage::get_token_usage_weekly_for_month,
            commands::token_usage::get_token_usage_monthly_for_year,
            commands::token_usage::run_token_usage_maintenance_now,
            commands::token_usage::list_llm_providers,
            commands::token_usage::update_llm_providers_from_store,
            commands::token_usage::list_token_limits,
            commands::token_usage::upsert_token_limit,
            commands::token_usage::delete_token_limit,
            commands::token_usage::list_custom_llm_providers,
            commands::token_usage::save_custom_llm_provider,
            commands::token_usage::delete_custom_llm_provider,
            // Notify
            commands::notify::send_os_notification,
            // Update
            commands::update::check_for_update,
            commands::update::install_update,
            commands::update::get_update_settings,
            commands::update::set_update_settings,
            commands::update::get_developer_mode,
            commands::update::set_developer_mode,
            commands::update::get_cdp_browser,
            commands::update::set_cdp_browser,
            commands::update::get_cdp_open_mode,
            commands::update::set_cdp_open_mode,
            commands::update::get_force_upgrade_info,
            commands::update::get_pending_update,
            // Admin Webview
            commands::admin_webview::admin_webview_open,
            commands::admin_webview::admin_webview_focus,
            commands::admin_webview::admin_webview_navigate,
            commands::admin_webview::admin_webview_hide,
            commands::admin_webview::admin_webview_reload,
            // WHPX (Windows Hypervisor Platform)
            commands::whpx_windows::check_whpx_status,
            commands::whpx_windows::enable_whpx,
            commands::whpx_windows::reboot_for_whpx,
            quit_app,
        ]);

    builder
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app_handle, _event| {});
}

/// Install the nilbox icon and .desktop entry into the user's local directories.
/// On Wayland/GNOME, the app icon in the Alt+Tab switcher and dock is resolved
/// from the .desktop file + XDG icon theme — set_icon() has no effect there.
#[cfg(target_os = "linux")]
fn linux_install_desktop_entry() {
    use std::fs;

    const APP_ID: &str = "run.nilbox.app";
    const ICON_BYTES: &[u8] = include_bytes!("../icons/icon.png");

    let home = match std::env::var("HOME") {
        Ok(h) => std::path::PathBuf::from(h),
        Err(_) => return,
    };

    // Install icon: ~/.local/share/icons/hicolor/256x256/apps/<app-id>.png
    let icon_dir = home.join(".local/share/icons/hicolor/256x256/apps");
    if fs::create_dir_all(&icon_dir).is_ok() {
        let icon_dest = icon_dir.join(format!("{}.png", APP_ID));
        let _ = fs::write(&icon_dest, ICON_BYTES);
    }

    // Install .desktop file: ~/.local/share/applications/<app-id>.desktop
    let apps_dir = home.join(".local/share/applications");
    if fs::create_dir_all(&apps_dir).is_ok() {
        let exe = std::env::current_exe()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let desktop = format!(
            "[Desktop Entry]\nName=nilbox\nExec={exe}\nIcon={APP_ID}\nType=Application\nCategories=Utility;\nStartupWMClass=nilbox\n"
        );
        let _ = fs::write(apps_dir.join(format!("{}.desktop", APP_ID)), desktop);

        // Refresh desktop database so GNOME picks up the new entry immediately
        let _ = std::process::Command::new("update-desktop-database")
            .arg(&apps_dir)
            .status();
    }

    // Refresh icon cache
    let _ = std::process::Command::new("gtk-update-icon-cache")
        .args(["-f", "-t", home.join(".local/share/icons/hicolor").to_str().unwrap_or("")])
        .status();
}
