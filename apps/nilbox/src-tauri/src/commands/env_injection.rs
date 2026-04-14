use crate::AppState;
use nilbox_core::config_store::{EnvProvider, EnvVarEntry, OAuthProvider};
use nilbox_core::proxy::oauth_token_vault::OAuthSessionInfo;
use nilbox_core::store::envelope::parse_envelope;
use nilbox_core::store::pinning::build_pinned_http_client;
use nilbox_core::store::verify::verify_envelope;
use nilbox_core::store::STORE_BASE_URL;
use tauri::State;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, error};

#[derive(Serialize)]
pub struct EnvProvidersResponse {
    pub version: String,
    pub providers: Vec<EnvProvider>,
    pub skipped: bool,
}

/// List all env providers from the database with version.
#[tauri::command]
pub async fn list_env_providers(
    state: State<'_, AppState>,
) -> Result<EnvProvidersResponse, String> {
    let version = state.service.state.config_store
        .get_env_providers_version()
        .unwrap_or_else(|_| "19700101-01".to_string());
    let providers = state.service.state.config_store
        .list_env_providers()
        .map_err(|e| e.to_string())?;
    Ok(EnvProvidersResponse { version, providers, skipped: false })
}

/// List all env var entries for a VM: DB providers merged with per-VM overrides + custom entries.
#[tauri::command]
pub async fn list_env_entries(
    state: State<'_, AppState>,
    vm_id: String,
) -> Result<Vec<EnvVarEntry>, String> {
    let providers = state.service.state.config_store
        .list_env_providers()
        .map_err(|e| e.to_string())?;

    let custom_vars = state.service.state.config_store
        .list_custom_env_vars()
        .map_err(|e| e.to_string())?;

    let overrides = state.service.state.config_store
        .list_env_entry_overrides(&vm_id)
        .map_err(|e| e.to_string())?;

    // Build lookup: name → enabled flag (from per-VM overrides)
    let override_map: HashMap<String, bool> = overrides
        .iter()
        .map(|e| (e.name.clone(), e.enabled))
        .collect();

    // Start with the dynamic provider list (order by sort_order)
    let mut entries: Vec<EnvVarEntry> = providers
        .iter()
        .map(|p| {
            let enabled = override_map.get(&p.env_name).copied().unwrap_or(false);
            EnvVarEntry {
                name: p.env_name.clone(),
                value: p.env_name.clone(),
                enabled,
                builtin: true,
                domain: p.domain.clone(),
            }
        })
        .collect();

    // Append custom entries from the global custom_env_vars table
    for cv in &custom_vars {
        let enabled = override_map.get(&cv.name).copied().unwrap_or(false);
        entries.push(EnvVarEntry {
            name: cv.name.clone(),
            value: cv.provider_name.clone(),
            enabled,
            builtin: false,
            domain: cv.domain.clone(),
        });
    }

    Ok(entries)
}

/// Enable or disable an env var entry for a VM.
#[tauri::command]
pub async fn set_env_entry_enabled(
    state: State<'_, AppState>,
    vm_id: String,
    name: String,
    enabled: bool,
) -> Result<(), String> {
    let providers = state.service.state.config_store
        .list_env_providers()
        .map_err(|e| e.to_string())?;
    let custom_vars = state.service.state.config_store
        .list_custom_env_vars()
        .map_err(|e| e.to_string())?;

    let builtin = providers.iter().any(|p| p.env_name == name)
        && !custom_vars.iter().any(|c| c.name == name);

    state.service.state.config_store
        .upsert_env_entry(&vm_id, &name, enabled, builtin)
        .map_err(|e| e.to_string())?;

    // If VM is running, immediately apply the change to the active shell sessions
    let _ = state.service.apply_env_injection(&vm_id, Some((&name, enabled))).await;

    Ok(())
}

/// Add a custom (non-builtin) env var definition (VM-independent).
#[tauri::command]
pub async fn add_custom_env_entry(
    state: State<'_, AppState>,
    name: String,
    provider_name: String,
    domain: String,
) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("Name cannot be empty".to_string());
    }
    if domain.trim().is_empty() {
        return Err("Domain cannot be empty".to_string());
    }
    state.service.state.config_store
        .upsert_custom_env_var(name.trim(), provider_name.trim(), domain.trim())
        .map_err(|e| e.to_string())
}

/// Remove a custom env var definition (VM-independent).
#[tauri::command]
pub async fn remove_custom_env_entry(
    state: State<'_, AppState>,
    name: String,
) -> Result<(), String> {
    state.service.state.config_store
        .delete_custom_env_var(&name)
        .map_err(|e| e.to_string())
}

/// Delete a single env provider by env_name from the local DB.
#[tauri::command]
pub async fn delete_env_provider(
    state: State<'_, AppState>,
    env_name: String,
) -> Result<(), String> {
    state.service.state.config_store
        .delete_env_provider(&env_name)
        .map_err(|e| e.to_string())
}

/// Fetch env providers from the store, verify the signed envelope, and update the local DB.
#[tauri::command]
pub async fn update_env_providers_from_store(
    state: State<'_, AppState>,
) -> Result<EnvProvidersResponse, String> {
    // Deserialize helper for the manifest's provider array
    #[derive(Deserialize)]
    struct ProviderItem {
        env_name: String,
        provider_name: String,
        sort_order: i32,
        #[serde(default)]
        domain: String,
    }

    // Fetch V3 envelope from store using the pinned HTTP client
    let url = format!("{}/environments", STORE_BASE_URL);
    debug!("[update_env_providers] fetching from {}", url);
    let http = build_pinned_http_client();
    let resp = http.get(&url).send().await
        .map_err(|e| { error!("[update_env_providers] HTTP request failed: {}", e); format!("Failed to fetch environments: {}", e) })?;
    let status = resp.status();
    debug!("[update_env_providers] HTTP status: {}", status);
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        error!("[update_env_providers] store error body: {}", body);
        return Err(format!("Store returned HTTP {}", status));
    }
    let raw: serde_json::Value = resp.json().await
        .map_err(|e| { error!("[update_env_providers] JSON parse failed: {}", e); format!("Invalid JSON from store: {}", e) })?;
    debug!("[update_env_providers] envelope version field: {:?}", raw.get("version"));

    // Parse + verify the envelope (V3 → decrypt → V2 → Ed25519 verify)
    debug!("[update_env_providers] parsing envelope");
    let envelope = parse_envelope(&raw)
        .map_err(|e| { error!("[update_env_providers] envelope parse error: {}", e); format!("Envelope parse error: {}", e) })?;
    debug!("[update_env_providers] verifying envelope");
    let manifest = verify_envelope(&envelope)
        .map_err(|e| { error!("[update_env_providers] envelope verify error: {}", e); format!("Envelope verify error: {}", e) })?;
    debug!("[update_env_providers] envelope verified OK");

    // Validate type field
    let manifest_type = manifest.get("type").and_then(|v| v.as_str()).unwrap_or("(missing)");
    debug!("[update_env_providers] manifest type: '{}'", manifest_type);
    if manifest_type != "env_providers" {
        error!("[update_env_providers] unexpected manifest type: '{}'", manifest_type);
        return Err(format!("Manifest type is not 'env_providers', got '{}'", manifest_type));
    }

    // Extract manifest version (yyyymmdd-NN string format)
    let manifest_version = manifest.get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("19700101-01")
        .to_string();
    debug!("[update_env_providers] manifest version: {}", manifest_version);

    // Always replace local data with server data (already fetched & verified).
    // No version skip — ensures server-side deletions propagate immediately.

    // Extract providers
    let providers_value = manifest.get("providers").cloned().unwrap_or(serde_json::Value::Array(vec![]));
    let items: Vec<ProviderItem> = serde_json::from_value(providers_value)
        .map_err(|e| { error!("[update_env_providers] providers parse error: {}", e); format!("Failed to parse providers: {}", e) })?;
    debug!("[update_env_providers] parsed {} providers from manifest", items.len());

    let providers: Vec<EnvProvider> = items.into_iter().map(|p| EnvProvider {
        env_name: p.env_name,
        provider_name: p.provider_name,
        sort_order: p.sort_order,
        domain: p.domain,
    }).collect();

    state.service.state.config_store
        .update_env_providers(&providers, &manifest_version)
        .map_err(|e| e.to_string())?;

    // Return updated version + providers
    let version = state.service.state.config_store
        .get_env_providers_version()
        .unwrap_or_else(|_| "19700101-01".to_string());
    let saved = state.service.state.config_store
        .list_env_providers()
        .map_err(|e| e.to_string())?;
    Ok(EnvProvidersResponse { version, providers: saved, skipped: false })
}

/// Rewrite /etc/profile.d/nilbox-envs.sh in a running VM to reflect current
/// enabled env entries.  No-op (Ok) if the VM is not running.
#[tauri::command]
pub async fn apply_env_injection(
    state: State<'_, AppState>,
    vm_id: String,
) -> Result<(), String> {
    state.service.apply_env_injection(&vm_id, None).await
        .map_err(|e| e.to_string())
}

// ── OAuth Provider Commands ──────────────────────────────────

#[derive(Serialize)]
pub struct OAuthProviderItemResponse {
    pub provider_id: String,
    pub provider_name: String,
    pub domain: String,
    pub sort_order: i32,
    pub input_type: String,
    pub is_custom: bool,
    pub script_code: Option<String>,
    pub envs: Vec<OAuthProviderEnvResponse>,
}

#[derive(Serialize)]
pub struct OAuthProviderEnvResponse {
    pub env_name: String,
}

#[derive(Serialize)]
pub struct OAuthProvidersResponse {
    pub version: String,
    pub providers: Vec<OAuthProviderItemResponse>,
    pub skipped: bool,
}

/// Internal helper: read OAuth providers from DB and build response.
fn read_oauth_providers_from_db(
    config_store: &nilbox_core::config_store::ConfigStore,
) -> Result<OAuthProvidersResponse, String> {
    let version = config_store
        .get_oauth_providers_version()
        .unwrap_or_else(|_| "19700101-01".to_string());
    let providers = config_store
        .list_oauth_providers()
        .map_err(|e| e.to_string())?;
    let mut items = Vec::new();
    for p in &providers {
        let envs = config_store
            .list_oauth_provider_envs(&p.provider_id)
            .map_err(|e| e.to_string())?;
        items.push(OAuthProviderItemResponse {
            provider_id: p.provider_id.clone(),
            provider_name: p.provider_name.clone(),
            domain: p.domain.clone(),
            sort_order: p.sort_order,
            input_type: p.input_type.clone(),
            is_custom: p.is_custom,
            script_code: p.script_code.clone(),
            envs: envs.into_iter().map(|e| OAuthProviderEnvResponse { env_name: e.env_name }).collect(),
        });
    }
    Ok(OAuthProvidersResponse { version, providers: items, skipped: false })
}

/// Core download logic: fetch from store, verify, save to DB + keystore.
/// Returns `true` if skipped (local version already up-to-date).
async fn fetch_and_save_oauth_providers(
    core: &nilbox_core::state::CoreState,
) -> Result<bool, String> {
    #[derive(Deserialize)]
    struct EnvItem { env_name: String }
    #[derive(Deserialize)]
    struct ScriptItem { version: String, signature: String, code: String }
    #[derive(Deserialize)]
    struct ProviderItem {
        provider_id: String,
        provider_name: String,
        #[serde(default)] domain: String,
        sort_order: i32,
        #[serde(default = "default_input_type")] input_type: String,
        #[serde(default)] envs: Vec<EnvItem>,
        #[serde(default)] script: Option<ScriptItem>,
    }
    fn default_input_type() -> String { "input".to_string() }

    let url = format!("{}/oauth-providers", STORE_BASE_URL);
    debug!("[update_oauth_providers] fetching from {}", url);
    let http = build_pinned_http_client();
    let resp = http.get(&url).send().await
        .map_err(|e| { error!("[update_oauth_providers] HTTP request failed: {}", e); format!("Failed to fetch oauth-providers: {}", e) })?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        error!("[update_oauth_providers] store error body: {}", body);
        return Err(format!("Store returned HTTP {}", status));
    }
    let raw: serde_json::Value = resp.json().await
        .map_err(|e| format!("Invalid JSON from store: {}", e))?;

    let envelope = parse_envelope(&raw)
        .map_err(|e| format!("Envelope parse error: {}", e))?;
    let manifest = verify_envelope(&envelope)
        .map_err(|e| format!("Envelope verify error: {}", e))?;

    let manifest_type = manifest.get("type").and_then(|v| v.as_str()).unwrap_or("(missing)");
    if manifest_type != "oauth_providers" {
        return Err(format!("Manifest type is not 'oauth_providers', got '{}'", manifest_type));
    }

    let manifest_version = manifest.get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("19700101-01")
        .to_string();

    // Always replace local data with server data (already fetched & verified).
    // No version skip — ensures server-side deletions propagate immediately.

    let providers_value = manifest.get("providers").cloned().unwrap_or(serde_json::Value::Array(vec![]));
    let items: Vec<ProviderItem> = serde_json::from_value(providers_value)
        .map_err(|e| format!("Failed to parse providers: {}", e))?;

    let providers: Vec<OAuthProvider> = items.iter().map(|p| OAuthProvider {
        provider_id: p.provider_id.clone(),
        provider_name: p.provider_name.clone(),
        domain: p.domain.clone(),
        sort_order: p.sort_order,
        input_type: p.input_type.clone(),
        is_custom: false,
        script_code: None,
    }).collect();

    let envs_map: HashMap<String, Vec<String>> = items.iter().map(|p| {
        (p.provider_id.clone(), p.envs.iter().map(|e| e.env_name.clone()).collect())
    }).collect();

    core.config_store
        .update_oauth_providers(&providers, &envs_map, &manifest_version)
        .map_err(|e| e.to_string())?;

    // Save OAuth scripts to keystore (with signature verification)
    let mut scripts_saved = 0u32;
    for item in &items {
        if let Some(ref script) = item.script {
            use base64::{Engine as B64Engine, engine::general_purpose::STANDARD};
            use ed25519_dalek::{Signature, Verifier};
            use nilbox_core::store::keys::get_store_public_key;

            let code_bytes = match STANDARD.decode(&script.code) {
                Ok(b) => b,
                Err(e) => { error!("[update_oauth_providers] base64 decode failed for {}: {}", item.provider_id, e); continue; }
            };
            let sig_bytes = match STANDARD.decode(&script.signature) {
                Ok(b) => b,
                Err(e) => { error!("[update_oauth_providers] sig base64 decode failed for {}: {}", item.provider_id, e); continue; }
            };
            let signature = match Signature::from_slice(&sig_bytes) {
                Ok(s) => s,
                Err(e) => { error!("[update_oauth_providers] invalid signature format for {}: {}", item.provider_id, e); continue; }
            };

            let key_ids = ["nilbox-store-dev", "nilbox-store-2026"];
            let mut verified = false;
            for key_id in &key_ids {
                if let Ok(vk) = get_store_public_key(key_id) {
                    if vk.verify(&code_bytes, &signature).is_ok() { verified = true; break; }
                }
            }
            if !verified {
                error!("[update_oauth_providers] script signature verification failed for {}", item.provider_id);
                continue;
            }

            let entry = serde_json::json!({ "code": script.code, "version": script.version, "signature": script.signature });
            let account = format!("OAUTH_SCRIPT:{}", item.provider_id);
            if let Err(e) = core.keystore.set(&account, &entry.to_string()).await {
                error!("[update_oauth_providers] keystore save failed for {}: {}", item.provider_id, e);
            } else {
                scripts_saved += 1;
                debug!("[update_oauth_providers] saved script for {} (version={})", item.provider_id, script.version);
            }
        }
    }

    if scripts_saved > 0 {
        let _ = core.keystore.set("OAUTH_SCRIPTS_MANIFEST_VERSION", &manifest_version).await;
        let new_engine = nilbox_core::proxy::oauth_script_engine::OAuthScriptEngine::load_all(
            core.keystore.as_ref(),
            &core.config_store,
        ).await.unwrap_or_else(|e| {
            error!("[update_oauth_providers] Failed to reload OAuth engine: {}", e);
            nilbox_core::proxy::oauth_script_engine::OAuthScriptEngine::empty()
        });
        *core.oauth_engine.write().await = std::sync::Arc::new(new_engine);
        debug!("[update_oauth_providers] OAuth script engine reloaded ({} script(s) saved)", scripts_saved);
    }

    debug!("[update_oauth_providers] done — version={}, scripts_saved={}", manifest_version, scripts_saved);
    Ok(false) // not skipped
}

/// Auto-seed oauth providers on first install (no-op if already populated).
/// Intended to be called from the startup background task.
pub async fn seed_oauth_providers_if_empty(core: std::sync::Arc<nilbox_core::state::CoreState>) {
    let version = core.config_store
        .get_oauth_providers_version()
        .unwrap_or_else(|_| "19700101-01".to_string());
    let is_empty = core.config_store
        .list_oauth_providers()
        .map(|v| v.is_empty())
        .unwrap_or(true);
    if !is_empty || version != "19700101-01" {
        return;
    }
    debug!("[seed_oauth_providers] first install detected — auto-downloading from store");
    match fetch_and_save_oauth_providers(&core).await {
        Ok(_) => debug!("[seed_oauth_providers] done"),
        Err(e) => tracing::warn!("[seed_oauth_providers] auto-download failed (non-fatal): {}", e),
    }
}

/// List all OAuth providers with their env vars.
#[tauri::command]
pub async fn list_oauth_providers(
    state: State<'_, AppState>,
) -> Result<OAuthProvidersResponse, String> {
    read_oauth_providers_from_db(&state.service.state.config_store)
}

/// Fetch OAuth providers from the store, verify the signed envelope, and update local DB.
#[tauri::command]
pub async fn update_oauth_providers_from_store(
    state: State<'_, AppState>,
) -> Result<OAuthProvidersResponse, String> {
    let skipped = fetch_and_save_oauth_providers(&state.service.state).await?;
    read_oauth_providers_from_db(&state.service.state.config_store)
        .map(|mut r| { r.skipped = skipped; r })
}

// ── OAuth Session Commands ──────────────────────────────────

/// List active OAuth sessions for a VM.
#[tauri::command]
pub async fn list_oauth_sessions(
    state: State<'_, AppState>,
    vm_id: String,
) -> Result<Vec<OAuthSessionInfo>, String> {
    let filter = if vm_id.is_empty() { None } else { Some(vm_id.as_str()) };
    let sessions = state.service.state.oauth_vault
        .list_sessions(filter)
        .await
        .map_err(|e| e.to_string())?;
    Ok(sessions)
}

/// Delete a single OAuth session by its session key.
#[tauri::command]
pub async fn delete_oauth_session(
    state: State<'_, AppState>,
    session_key: String,
) -> Result<(), String> {
    state.service.state.oauth_vault
        .delete_session(&session_key)
        .await
        .map_err(|e| e.to_string())
}

/// Delete all OAuth sessions for a given VM.
#[tauri::command]
pub async fn delete_all_oauth_sessions(
    state: State<'_, AppState>,
    vm_id: String,
) -> Result<(), String> {
    state.service.state.oauth_vault
        .cleanup_vm_sessions(&vm_id)
        .await
        .map_err(|e| e.to_string())
}

// ── Custom OAuth Provider Commands ──────────────────────────

/// Save (create or update) a custom OAuth provider.
#[tauri::command]
pub async fn save_custom_oauth_provider(
    state: State<'_, AppState>,
    provider_id: String,
    provider_name: String,
    domain: String,
    sort_order: i32,
    input_type: String,
    script_code: String,
    env_names: Vec<String>,
) -> Result<(), String> {
    if sort_order < 1000 {
        return Err("Custom OAuth provider sort_order must be >= 1000".into());
    }

    state.service.state.config_store
        .save_custom_oauth_provider(
            &provider_id, &provider_name, &domain,
            sort_order, &input_type, &script_code, &env_names,
        )
        .map_err(|e| e.to_string())?;

    // Reload OAuth script engine
    let new_engine = nilbox_core::proxy::oauth_script_engine::OAuthScriptEngine::load_all(
        state.service.state.keystore.as_ref(),
        &state.service.state.config_store,
    ).await.unwrap_or_else(|e| {
        error!("[save_custom_oauth] Failed to reload OAuth engine: {}", e);
        nilbox_core::proxy::oauth_script_engine::OAuthScriptEngine::empty()
    });
    *state.service.state.oauth_engine.write().await = std::sync::Arc::new(new_engine);

    Ok(())
}

/// Delete a custom OAuth provider.
#[tauri::command]
pub async fn delete_custom_oauth_provider(
    state: State<'_, AppState>,
    provider_id: String,
) -> Result<(), String> {
    state.service.state.config_store
        .delete_custom_oauth_provider(&provider_id)
        .map_err(|e| e.to_string())?;

    // Clean up keystore entries for this provider
    let keystore = state.service.state.keystore.as_ref();
    let accounts = keystore.list().await.unwrap_or_default();
    for account in &accounts {
        if account.starts_with(&format!("oauth:{}:", provider_id))
            || account == &format!("OAUTH_SCRIPT:{}", provider_id)
        {
            let _ = keystore.delete(account).await;
        }
    }

    // Reload OAuth script engine
    let new_engine = nilbox_core::proxy::oauth_script_engine::OAuthScriptEngine::load_all(
        state.service.state.keystore.as_ref(),
        &state.service.state.config_store,
    ).await.unwrap_or_else(|e| {
        error!("[delete_custom_oauth] Failed to reload OAuth engine: {}", e);
        nilbox_core::proxy::oauth_script_engine::OAuthScriptEngine::empty()
    });
    *state.service.state.oauth_engine.write().await = std::sync::Arc::new(new_engine);

    Ok(())
}

/// Validate a Rhai OAuth script (compile + call provider_info()).
#[tauri::command]
pub async fn validate_oauth_script(
    script_code: String,
) -> Result<serde_json::Value, String> {
    match nilbox_core::proxy::oauth_script_engine::OAuthScriptEngine::validate_script(&script_code) {
        Ok(info) => Ok(serde_json::json!({
            "valid": true,
            "provider_info": {
                "name": info.name,
                "token_path": info.token_path,
                "placeholder_prefix": info.placeholder_prefix,
                "auth_domains": info.auth_domains,
                "token_path_pattern": info.token_path_pattern,
            }
        })),
        Err(e) => Ok(serde_json::json!({
            "valid": false,
            "error": e.to_string(),
        })),
    }
}
