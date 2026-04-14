//! OAuthScriptEngine — loads and caches Rhai scripts for dynamic OAuth providers.
//!
//! Each `.rhai` file implements 5 functions (provider_info, placeholder_extraction_instructions,
//! make_dummy_secret, rewrite_auth_url, build_token_request_instructions).
//! Scripts return *instructions* only — they never see actual secrets.

use anyhow::{Result, anyhow};
use rhai::{Engine, AST, Scope, Dynamic, Map};
use std::collections::HashMap;
use tracing::{debug, warn};

use crate::keystore::KeyStore;

/// Metadata extracted from a provider script's `provider_info()` call.
#[derive(Debug, Clone)]
pub struct ProviderInfo {
    pub name: String,
    pub token_path: String,
    pub placeholder_prefix: String,
    pub auth_domains: Vec<String>,
    pub token_path_pattern: String,
    /// OAuth flow type: "placeholder" (default) or "pkce"
    pub flow_type: String,
    /// Token endpoint domains for PKCE domain+path matching (empty for placeholder flows)
    pub token_endpoint_domains: Vec<String>,
    /// Cross-domain list: additional domains where this OAuth token may be used (e.g. chatgpt.com for OpenAI)
    pub cross_domains: Vec<String>,
}

/// A loaded OAuth provider: compiled AST + cached metadata.
pub struct OAuthProvider {
    pub info: ProviderInfo,
    pub ast: AST,
}

pub struct OAuthScriptEngine {
    engine: Engine,
    providers: HashMap<String, OAuthProvider>,
}

impl OAuthScriptEngine {
    /// Load scripts from KeyStore (store-deployed model).
    /// Reads `OAUTH_SCRIPT:*` entries, verifies Ed25519 signatures, compiles verified code.
    pub async fn load_from_keystore(keystore: &dyn KeyStore) -> Result<Self> {
        use base64::{Engine as B64Engine, engine::general_purpose::STANDARD};
        use ed25519_dalek::{Signature, Verifier};
        use crate::store::keys::get_store_public_key;

        let engine = Engine::new();
        let mut providers = HashMap::new();

        let accounts = keystore.list().await?;
        let script_accounts: Vec<&String> = accounts.iter()
            .filter(|a| a.starts_with("OAUTH_SCRIPT:"))
            .collect();

        if script_accounts.is_empty() {
            debug!("No OAuth scripts found in keystore");
            return Ok(Self { engine, providers });
        }

        for account in script_accounts {
            let provider_id = account.strip_prefix("OAUTH_SCRIPT:").unwrap();
            let value = match keystore.get(account).await {
                Ok(v) => v,
                Err(e) => {
                    warn!("Failed to read keystore entry {}: {}", account, e);
                    continue;
                }
            };

            // Parse JSON: {"code":"...","version":"...","signature":"..."}
            let entry: serde_json::Value = match serde_json::from_str(&value) {
                Ok(v) => v,
                Err(e) => {
                    warn!("Failed to parse keystore entry {}: {}", account, e);
                    continue;
                }
            };

            let code_b64 = match entry.get("code").and_then(|v| v.as_str()) {
                Some(c) => c,
                None => { warn!("Missing 'code' in keystore entry {}", account); continue; }
            };
            let sig_b64 = match entry.get("signature").and_then(|v| v.as_str()) {
                Some(s) => s,
                None => { warn!("Missing 'signature' in keystore entry {}", account); continue; }
            };

            let code_bytes = match STANDARD.decode(code_b64) {
                Ok(b) => b,
                Err(e) => { warn!("Invalid base64 code in {}: {}", account, e); continue; }
            };
            let sig_bytes = match STANDARD.decode(sig_b64) {
                Ok(b) => b,
                Err(e) => { warn!("Invalid base64 signature in {}: {}", account, e); continue; }
            };

            // Verify Ed25519 signature with any known store key
            let signature = match Signature::from_slice(&sig_bytes) {
                Ok(s) => s,
                Err(e) => { warn!("Invalid signature format in {}: {}", account, e); continue; }
            };

            let key_ids = ["nilbox-store-dev", "nilbox-store-2026"];
            let mut verified = false;
            for key_id in &key_ids {
                if let Ok(vk) = get_store_public_key(key_id) {
                    if vk.verify(&code_bytes, &signature).is_ok() {
                        verified = true;
                        break;
                    }
                }
            }
            if !verified {
                warn!("Signature verification failed for {}, skipping", account);
                continue;
            }

            let code_str = match String::from_utf8(code_bytes) {
                Ok(s) => s,
                Err(e) => { warn!("Invalid UTF-8 code in {}: {}", account, e); continue; }
            };

            match Self::load_one_from_code(&engine, provider_id, &code_str) {
                Ok(provider) => {
                    debug!("Loaded OAuth script from keystore: {} ({})", provider.info.name, provider_id);
                    providers.insert(provider_id.to_string(), provider);
                }
                Err(e) => {
                    warn!("Failed to compile OAuth script {}: {}", provider_id, e);
                }
            }
        }

        Ok(Self { engine, providers })
    }

    /// Create an empty engine with no providers.
    pub fn empty() -> Self {
        Self { engine: Engine::new(), providers: HashMap::new() }
    }

    /// Load scripts from both KeyStore (server-deployed, signature-verified)
    /// and ConfigStore (custom providers, no signature).
    pub async fn load_all(
        keystore: &dyn KeyStore,
        config_store: &crate::config_store::ConfigStore,
    ) -> Result<Self> {
        let instance = Self::load_from_keystore(keystore).await?;

        // Load custom provider scripts from DB (no signature verification)
        let providers = config_store.list_oauth_providers()?;
        for p in providers {
            if !p.is_custom {
                continue;
            }
            if let Some(ref code) = p.script_code {
                match Self::load_one_from_code(&instance.engine, &p.provider_id, code) {
                    Ok(_provider) => {
                        // debug!("Loaded custom OAuth script: {} ({})", _provider.info.name, p.provider_id);
                        // instance.providers.insert(p.provider_id.clone(), _provider);
                    }
                    Err(e) => {
                        warn!("Failed to compile custom OAuth script {}: {}", p.provider_id, e);
                    }
                }
            }
        }

        Ok(instance)
    }

    /// Validate a Rhai script by compiling it and calling provider_info().
    /// Returns the ProviderInfo on success.
    pub fn validate_script(code: &str) -> Result<ProviderInfo> {
        let engine = Engine::new();
        let provider = Self::load_one_from_code(&engine, "_validate", code)?;
        Ok(provider.info)
    }

    pub(crate) fn load_one_from_code(engine: &Engine, name: &str, code: &str) -> Result<OAuthProvider> {
        let ast = engine.compile(code)
            .map_err(|e| anyhow!("Compile error in script '{}': {}", name, e))?;

        let mut scope = Scope::new();
        let result: Dynamic = engine.call_fn(&mut scope, &ast, "provider_info", ())
            .map_err(|e| anyhow!("provider_info() failed in script '{}': {}", name, e))?;

        let map = result.cast::<Map>();
        let info = ProviderInfo {
            name: map.get("name")
                .and_then(|v| v.clone().into_string().ok())
                .ok_or_else(|| anyhow!("missing 'name' in provider_info"))?,
            token_path: map.get("token_path")
                .and_then(|v| v.clone().into_string().ok())
                .ok_or_else(|| anyhow!("missing 'token_path' in provider_info"))?,
            placeholder_prefix: map.get("placeholder_prefix")
                .and_then(|v| v.clone().into_string().ok())
                .ok_or_else(|| anyhow!("missing 'placeholder_prefix' in provider_info"))?,
            auth_domains: extract_string_array(&map, "auth_domains")?,
            token_path_pattern: map.get("token_path_pattern")
                .and_then(|v| v.clone().into_string().ok())
                .ok_or_else(|| anyhow!("missing 'token_path_pattern' in provider_info"))?,
            flow_type: map.get("flow_type")
                .and_then(|v| v.clone().into_string().ok())
                .unwrap_or_else(|| "placeholder".to_string()),
            token_endpoint_domains: map.get("token_endpoint_domains")
                .and_then(|v| v.clone().into_array().ok())
                .map(|arr| arr.into_iter()
                    .filter_map(|v| v.into_string().ok())
                    .collect())
                .unwrap_or_default(),
            cross_domains: map.get("cross_domains")
                .and_then(|v| v.clone().into_array().ok())
                .map(|arr| arr.into_iter()
                    .filter_map(|v| v.into_string().ok())
                    .collect())
                .unwrap_or_default(),
        };

        Ok(OAuthProvider { info, ast })
    }

    /// Find a provider by auth domain (Phase 1: browser URL rewriting).
    pub fn find_by_auth_domain(&self, domain: &str) -> Option<&OAuthProvider> {
        self.providers.values()
            .find(|p| p.info.auth_domains.iter().any(|d| d == domain))
    }

    /// Find a provider by token path pattern (Phase 3: token exchange).
    pub fn find_by_token_path(&self, path: &str) -> Option<&OAuthProvider> {
        let result = self.providers.values()
            .find(|p| !p.info.token_path_pattern.is_empty()
                && path.contains(&p.info.token_path_pattern));
        // debug!(
        //     "OAuth script: find_by_token_path path={} matched={}",
        //     path,
        //     result.map(|p| format!("{}(pattern={})", p.info.name, p.info.token_path_pattern))
        //         .unwrap_or_else(|| "none".into())
        // );
        result
    }

    /// Find a provider by token endpoint domain + path (PKCE flows).
    pub fn find_by_token_domain_and_path(&self, domain: &str, path: &str) -> Option<&OAuthProvider> {
        let result = self.providers.values().find(|p| {
            p.info.token_endpoint_domains.iter().any(|d| d == domain)
                && path.contains(&p.info.token_path_pattern)
        });
        // debug!(
        //     "OAuth script: find_by_token_domain_and_path domain={} path={} matched={}",
        //     domain, path,
        //     result.map(|p| p.info.name.as_str()).unwrap_or("none")
        // );
        result
    }

    /// Get provider by name.
    pub fn get_provider(&self, name: &str) -> Option<&OAuthProvider> {
        self.providers.get(name)
    }

    /// Iterate all providers.
    pub fn providers(&self) -> impl Iterator<Item = &OAuthProvider> {
        self.providers.values()
    }

    // ── Script function wrappers ──

    /// Call `token_response_fields()` → provider-specific response field mapping.
    /// Returns `Default` (RFC 6749 standard) if the function is not defined in the script.
    pub fn call_token_response_fields(
        &self,
        provider: &OAuthProvider,
    ) -> TokenResponseFields {
        let mut scope = Scope::new();
        match self.engine.call_fn::<Dynamic>(&mut scope, &provider.ast, "token_response_fields", ()) {
            Ok(result) => {
                let map = result.cast::<Map>();
                TokenResponseFields {
                    access_token_field: map.get("access_token_field")
                        .and_then(|v| v.clone().into_string().ok())
                        .unwrap_or_else(|| "access_token".into()),
                    refresh_token_field: map.get("refresh_token_field")
                        .and_then(|v| v.clone().into_string().ok())
                        .unwrap_or_else(|| "refresh_token".into()),
                    expires_in_field: map.get("expires_in_field")
                        .and_then(|v| v.clone().into_string().ok())
                        .unwrap_or_else(|| "expires_in".into()),
                    token_type_field: map.get("token_type_field")
                        .and_then(|v| v.clone().into_string().ok())
                        .unwrap_or_else(|| "token_type".into()),
                    response_format: map.get("response_format")
                        .and_then(|v| v.clone().into_string().ok())
                        .unwrap_or_else(|| "json".into()),
                }
            }
            Err(_) => TokenResponseFields::default(),
        }
    }

    /// Call `placeholder_extraction_instructions()` → HashMap<placeholder_name, json_path>.
    pub fn call_placeholder_extraction_instructions(
        &self,
        provider: &OAuthProvider,
    ) -> Result<HashMap<String, String>> {
        let mut scope = Scope::new();
        let result: Dynamic = self.engine
            .call_fn(&mut scope, &provider.ast, "placeholder_extraction_instructions", ())
            .map_err(|e| anyhow!("placeholder_extraction_instructions() failed: {}", e))?;

        let map = result.cast::<Map>();
        let mut out = HashMap::new();
        for (k, v) in map {
            let key = k.to_string();
            let val = v.into_string()
                .map_err(|_| anyhow!("non-string value in placeholder_extraction_instructions"))?;
            out.insert(key, val);
        }
        Ok(out)
    }

    /// Call `make_dummy_secret(prefix)` → dummy secret JSON string with placeholders expanded.
    pub fn call_make_dummy_secret(&self, provider: &OAuthProvider) -> Result<String> {
        let mut scope = Scope::new();
        let prefix = provider.info.placeholder_prefix.clone();
        let result: Dynamic = self.engine
            .call_fn(&mut scope, &provider.ast, "make_dummy_secret", (prefix,))
            .map_err(|e| anyhow!("make_dummy_secret() failed: {}", e))?;
        result.into_string()
            .map_err(|_| anyhow!("make_dummy_secret() did not return a string"))
    }

    /// Call `rewrite_auth_url(url, placeholders)` → rewritten URL string.
    pub fn call_rewrite_auth_url(
        &self,
        provider: &OAuthProvider,
        url: &str,
        placeholders: &HashMap<String, String>,
    ) -> Result<String> {
        let mut scope = Scope::new();
        let rhai_map: Map = placeholders.iter()
            .map(|(k, v)| (k.clone().into(), Dynamic::from(v.clone())))
            .collect();

        let result: Dynamic = self.engine
            .call_fn(&mut scope, &provider.ast, "rewrite_auth_url", (url.to_string(), rhai_map))
            .map_err(|e| anyhow!("rewrite_auth_url() failed: {}", e))?;
        result.into_string()
            .map_err(|_| anyhow!("rewrite_auth_url() did not return a string"))
    }

    /// Call `is_token_exchange_request(body_params)` → true if this POST is a token exchange.
    /// Returns true if the function is not defined in the script (default: match all).
    pub fn call_is_token_exchange_request(
        &self,
        provider: &OAuthProvider,
        body_params: &HashMap<String, String>,
    ) -> bool {
        let rhai_map: Map = body_params.iter()
            .map(|(k, v)| (k.clone().into(), Dynamic::from(v.clone())))
            .collect();
        match self.engine.call_fn::<bool>(
            &mut Scope::new(), &provider.ast,
            "is_token_exchange_request", (rhai_map,)
        ) {
            Ok(result) => result,
            Err(_) => true,
        }
    }

    /// Call `build_token_request_instructions(body_params)` → instruction map.
    pub fn call_build_token_request_instructions(
        &self,
        provider: &OAuthProvider,
        body_params: &HashMap<String, String>,
    ) -> Result<TokenRequestInstructions> {
        let mut scope = Scope::new();
        let rhai_map: Map = body_params.iter()
            .map(|(k, v)| (k.clone().into(), Dynamic::from(v.clone())))
            .collect();

        let result: Dynamic = self.engine
            .call_fn(&mut scope, &provider.ast, "build_token_request_instructions", (rhai_map,))
            .map_err(|e| anyhow!("build_token_request_instructions() failed: {}", e))?;

        let map = result.cast::<Map>();

        let field_substitutions = {
            let fs = map.get("field_substitutions")
                .ok_or_else(|| anyhow!("missing field_substitutions"))?
                .clone()
                .cast::<Map>();
            let mut out = HashMap::new();
            for (k, v) in fs {
                let key = k.to_string();
                let val = v.into_string()
                    .map_err(|_| anyhow!("non-string value in field_substitutions"))?;
                out.insert(key, val);
            }
            out
        };

        let target_url = map.get("target_url")
            .and_then(|v| v.clone().into_string().ok())
            .ok_or_else(|| anyhow!("missing target_url"))?;

        let allowed_redirect_hosts = extract_string_array(&map, "allowed_redirect_hosts")?;

        Ok(TokenRequestInstructions {
            field_substitutions,
            target_url,
            allowed_redirect_hosts,
        })
    }
}

/// Instructions returned by `build_token_request_instructions()`.
pub struct TokenRequestInstructions {
    /// POST body field name → placeholder name (Rust resolves to real value).
    pub field_substitutions: HashMap<String, String>,
    /// The real token endpoint URL.
    pub target_url: String,
    /// Allowed hosts for redirect_uri validation.
    pub allowed_redirect_hosts: Vec<String>,
}

/// Provider-specific token response field mapping.
/// Returned by `token_response_fields()` Rhai function.
/// Default uses RFC 6749 standard field names.
pub struct TokenResponseFields {
    /// JSON key for access token (dot notation for nested: "data.access_token")
    pub access_token_field: String,
    /// JSON key for refresh token ("" = no refresh token)
    pub refresh_token_field: String,
    /// JSON key for expires_in
    pub expires_in_field: String,
    /// JSON key for token_type
    pub token_type_field: String,
    /// Response format: "json" or "form"
    pub response_format: String,
}

impl Default for TokenResponseFields {
    fn default() -> Self {
        Self {
            access_token_field: "access_token".into(),
            refresh_token_field: "refresh_token".into(),
            expires_in_field: "expires_in".into(),
            token_type_field: "token_type".into(),
            response_format: "json".into(),
        }
    }
}

/// Extract a string array from a Rhai map field.
fn extract_string_array(map: &Map, key: &str) -> Result<Vec<String>> {
    let arr = map.get(key)
        .ok_or_else(|| anyhow!("missing '{}' in map", key))?
        .clone()
        .into_array()
        .map_err(|_| anyhow!("'{}' is not an array", key))?;
    arr.into_iter()
        .map(|v| v.into_string()
            .map_err(|_| anyhow!("non-string element in '{}'", key)))
        .collect()
}

/// Resolve a dot-notation JSON path (e.g., "installed.client_id") against a serde_json::Value.
pub fn resolve_json_path(value: &serde_json::Value, path: &str) -> Option<String> {
    let mut current = value;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    // Try "installed" then "web" for Google-style nested secrets
    current.as_str().map(|s| s.to_string())
}

/// Resolve a dot-notation JSON path, trying "web" as fallback for first segment if "installed" fails.
pub fn resolve_json_path_with_fallback(value: &serde_json::Value, path: &str) -> Option<String> {
    // First try the literal path
    if let Some(result) = resolve_json_path(value, path) {
        return Some(result);
    }
    // If the first segment is "installed", try "web" as fallback
    let parts: Vec<&str> = path.splitn(2, '.').collect();
    if parts.len() == 2 && parts[0] == "installed" {
        let fallback_path = format!("web.{}", parts[1]);
        return resolve_json_path(value, &fallback_path);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;
    use crate::config_store::LlmProvider;

    /// In-memory mock KeyStore for tests.
    struct MockKeyStore {
        data: Mutex<HashMap<String, String>>,
    }

    impl MockKeyStore {
        fn new() -> Self {
            Self { data: Mutex::new(HashMap::new()) }
        }
        fn insert(&self, key: &str, value: &str) {
            self.data.lock().unwrap().insert(key.to_string(), value.to_string());
        }
    }

    #[async_trait]
    impl KeyStore for MockKeyStore {
        async fn get(&self, account: &str) -> Result<String> {
            self.data.lock().unwrap().get(account).cloned()
                .ok_or_else(|| anyhow!("not found: {}", account))
        }
        async fn set(&self, account: &str, value: &str) -> Result<()> {
            self.data.lock().unwrap().insert(account.to_string(), value.to_string());
            Ok(())
        }
        async fn delete(&self, account: &str) -> Result<()> {
            self.data.lock().unwrap().remove(account);
            Ok(())
        }
        async fn list(&self) -> Result<Vec<String>> {
            Ok(self.data.lock().unwrap().keys().cloned().collect())
        }
        async fn has(&self, account: &str) -> Result<bool> {
            Ok(self.data.lock().unwrap().contains_key(account))
        }
        async fn rename(&self, old_account: &str, new_account: &str) -> Result<()> {
            let mut data = self.data.lock().unwrap();
            if let Some(v) = data.remove(old_account) {
                data.insert(new_account.to_string(), v);
            }
            Ok(())
        }
        async fn get_ssh_private_key(&self) -> Result<Option<String>> { Ok(None) }
        async fn set_ssh_private_key(&self, _: &str) -> Result<()> { Ok(()) }
        async fn list_llm_providers(&self) -> Result<Vec<LlmProvider>> { Ok(vec![]) }
        async fn replace_llm_providers(&self, _: &[LlmProvider], _: &str) -> Result<()> { Ok(()) }
        async fn get_llm_providers_version(&self) -> Result<Option<String>> { Ok(None) }
    }

    fn make_signed_entry(code: &str) -> String {
        use base64::{Engine as B64Engine, engine::general_purpose::STANDARD};
        use ed25519_dalek::{SigningKey, Signer};

        // Dev key: all-zeros seed
        let signing_key = SigningKey::from_bytes(&[0u8; 32]);
        let code_bytes = code.as_bytes();
        let sig = signing_key.sign(code_bytes);
        let code_b64 = STANDARD.encode(code_bytes);
        let sig_b64 = STANDARD.encode(sig.to_bytes());

        serde_json::json!({
            "code": code_b64,
            "version": "1",
            "signature": sig_b64,
        }).to_string()
    }

    const GOOGLE_RHAI: &str = r#"// Google OAuth provider script

fn provider_info() {
    #{
        name: "google",
        token_path: "OAUTH_GOOGLE_FILE",
        placeholder_prefix: "NILBOX_OAUTH_GOOGLE",
        auth_domains: ["accounts.google.com"],
        token_path_pattern: "/oauth2/token",
    }
}

fn placeholder_extraction_instructions() {
    #{
        "ID": "installed.client_id",
        "SECRET": "installed.client_secret",
    }
}

fn make_dummy_secret(prefix) {
    "dummy"
}

fn rewrite_auth_url(url, placeholders) {
    url
}

fn build_token_request_instructions(body_params) {
    #{
        field_substitutions: #{
            "client_id": "ID",
            "client_secret": "SECRET",
        },
        target_url: "https://oauth2.googleapis.com/token",
        allowed_redirect_hosts: ["localhost"],
    }
}
"#;

    #[tokio::test]
    async fn test_load_from_keystore() {
        let ks = MockKeyStore::new();
        ks.insert("OAUTH_SCRIPT:google", &make_signed_entry(GOOGLE_RHAI));

        let engine = OAuthScriptEngine::load_from_keystore(&ks).await.unwrap();
        let google = engine.get_provider("google").expect("google provider should be loaded");
        assert_eq!(google.info.name, "google");
        assert_eq!(google.info.token_path, "OAUTH_GOOGLE_FILE");
        assert_eq!(google.info.auth_domains, vec!["accounts.google.com"]);
    }

    #[tokio::test]
    async fn test_load_from_keystore_bad_signature() {
        use base64::{Engine as B64Engine, engine::general_purpose::STANDARD};
        use ed25519_dalek::{SigningKey, Signer};

        let ks = MockKeyStore::new();
        // Sign with a DIFFERENT key (not the dev key) — signature will fail verification
        let wrong_key = SigningKey::from_bytes(&[1u8; 32]);
        let code_bytes = GOOGLE_RHAI.as_bytes();
        let wrong_sig = wrong_key.sign(code_bytes);
        let code_b64 = STANDARD.encode(code_bytes);
        let sig_b64 = STANDARD.encode(wrong_sig.to_bytes());
        let entry = serde_json::json!({
            "code": code_b64,
            "version": "1",
            "signature": sig_b64,
        }).to_string();
        ks.insert("OAUTH_SCRIPT:google", &entry);

        let engine = OAuthScriptEngine::load_from_keystore(&ks).await.unwrap();
        assert!(engine.get_provider("google").is_none(), "bad signature should skip provider");
    }

    #[tokio::test]
    async fn test_load_from_keystore_empty() {
        let ks = MockKeyStore::new();
        let engine = OAuthScriptEngine::load_from_keystore(&ks).await.unwrap();
        assert_eq!(engine.providers().count(), 0);
    }

    #[tokio::test]
    async fn test_token_response_fields_default() {
        // GOOGLE_RHAI has no token_response_fields() → should return Default
        let ks = MockKeyStore::new();
        ks.insert("OAUTH_SCRIPT:google", &make_signed_entry(GOOGLE_RHAI));

        let engine = OAuthScriptEngine::load_from_keystore(&ks).await.unwrap();
        let google = engine.get_provider("google").unwrap();
        let fields = engine.call_token_response_fields(google);

        assert_eq!(fields.access_token_field, "access_token");
        assert_eq!(fields.refresh_token_field, "refresh_token");
        assert_eq!(fields.expires_in_field, "expires_in");
        assert_eq!(fields.token_type_field, "token_type");
        assert_eq!(fields.response_format, "json");
    }

    #[tokio::test]
    async fn test_token_response_fields_custom() {
        let custom_rhai = r#"
fn provider_info() {
    #{
        name: "custom",
        token_path: "OAUTH_CUSTOM_FILE",
        placeholder_prefix: "NILBOX_OAUTH_CUSTOM",
        auth_domains: ["auth.custom.com"],
        token_path_pattern: "/api/token",
    }
}
fn placeholder_extraction_instructions() { #{} }
fn make_dummy_secret(prefix) { "dummy" }
fn rewrite_auth_url(url, placeholders) { url }
fn build_token_request_instructions(body_params) {
    #{ field_substitutions: #{}, target_url: "https://custom.com/token", allowed_redirect_hosts: [] }
}
fn token_response_fields() {
    #{
        access_token_field: "data.access_token",
        refresh_token_field: "",
        expires_in_field: "data.expires_in",
        token_type_field: "data.token_type",
        response_format: "json",
    }
}
"#;
        let ks = MockKeyStore::new();
        ks.insert("OAUTH_SCRIPT:custom", &make_signed_entry(custom_rhai));

        let engine = OAuthScriptEngine::load_from_keystore(&ks).await.unwrap();
        let custom = engine.get_provider("custom").unwrap();
        let fields = engine.call_token_response_fields(custom);

        assert_eq!(fields.access_token_field, "data.access_token");
        assert_eq!(fields.refresh_token_field, "");
        assert_eq!(fields.expires_in_field, "data.expires_in");
        assert_eq!(fields.token_type_field, "data.token_type");
        assert_eq!(fields.response_format, "json");
    }

    const OPENAI_PKCE_RHAI: &str = r#"
fn provider_info() {
    #{
        name: "openai",
        token_path: "oauth:openai",
        placeholder_prefix: "",
        auth_domains: ["auth.openai.com"],
        token_path_pattern: "/oauth/token",
        flow_type: "pkce",
        token_endpoint_domains: ["auth.openai.com"],
    }
}
fn placeholder_extraction_instructions() { #{} }
fn make_dummy_secret(prefix) { "" }
fn rewrite_auth_url(url, placeholders) { url }
fn build_token_request_instructions(body_params) {
    #{ field_substitutions: #{}, target_url: "https://auth.openai.com/oauth/token", allowed_redirect_hosts: ["localhost", "127.0.0.1"] }
}
fn token_response_fields() {
    #{
        access_token_field: "access_token",
        refresh_token_field: "refresh_token",
        expires_in_field: "expires_in",
        token_type_field: "token_type",
        response_format: "json",
    }
}
fn is_token_exchange_request(body_params) {
    let gt = body_params.get("grant_type");
    gt == "authorization_code" || gt == "refresh_token"
}
"#;

    #[test]
    fn test_pkce_provider_info_parsing() {
        let engine = Engine::new();
        let provider = OAuthScriptEngine::load_one_from_code(&engine, "openai", OPENAI_PKCE_RHAI).unwrap();
        assert_eq!(provider.info.name, "openai");
        assert_eq!(provider.info.flow_type, "pkce");
        assert_eq!(provider.info.token_endpoint_domains, vec!["auth.openai.com"]);
        assert_eq!(provider.info.placeholder_prefix, "");
        assert_eq!(provider.info.token_path, "oauth:openai");
    }

    #[test]
    fn test_default_flow_type_is_placeholder() {
        let engine = Engine::new();
        let provider = OAuthScriptEngine::load_one_from_code(&engine, "google", GOOGLE_RHAI).unwrap();
        assert_eq!(provider.info.flow_type, "placeholder");
        assert!(provider.info.token_endpoint_domains.is_empty());
    }

    #[tokio::test]
    async fn test_find_by_token_domain_and_path_exact_match() {
        let ks = MockKeyStore::new();
        ks.insert("OAUTH_SCRIPT:openai", &make_signed_entry(OPENAI_PKCE_RHAI));

        let engine = OAuthScriptEngine::load_from_keystore(&ks).await.unwrap();
        let found = engine.find_by_token_domain_and_path("auth.openai.com", "/oauth/token");
        assert!(found.is_some());
        assert_eq!(found.unwrap().info.name, "openai");
    }

    #[tokio::test]
    async fn test_find_by_token_domain_and_path_domain_mismatch() {
        let ks = MockKeyStore::new();
        ks.insert("OAUTH_SCRIPT:openai", &make_signed_entry(OPENAI_PKCE_RHAI));

        let engine = OAuthScriptEngine::load_from_keystore(&ks).await.unwrap();
        assert!(engine.find_by_token_domain_and_path("other.com", "/oauth/token").is_none());
    }

    #[tokio::test]
    async fn test_find_by_token_domain_and_path_path_mismatch() {
        let ks = MockKeyStore::new();
        ks.insert("OAUTH_SCRIPT:openai", &make_signed_entry(OPENAI_PKCE_RHAI));

        let engine = OAuthScriptEngine::load_from_keystore(&ks).await.unwrap();
        assert!(engine.find_by_token_domain_and_path("auth.openai.com", "/api/v1/data").is_none());
    }

    #[tokio::test]
    async fn test_find_by_token_domain_and_path_multi_provider() {
        let ks = MockKeyStore::new();
        ks.insert("OAUTH_SCRIPT:google", &make_signed_entry(GOOGLE_RHAI));
        ks.insert("OAUTH_SCRIPT:openai", &make_signed_entry(OPENAI_PKCE_RHAI));

        let engine = OAuthScriptEngine::load_from_keystore(&ks).await.unwrap();
        // OpenAI matches by domain
        let openai = engine.find_by_token_domain_and_path("auth.openai.com", "/oauth/token");
        assert!(openai.is_some());
        assert_eq!(openai.unwrap().info.name, "openai");
        // Google has no token_endpoint_domains → not matched by domain+path
        let google = engine.find_by_token_domain_and_path("oauth2.googleapis.com", "/oauth2/token");
        assert!(google.is_none());
    }

    #[tokio::test]
    async fn test_is_token_exchange_request_with_function() {
        let ks = MockKeyStore::new();
        ks.insert("OAUTH_SCRIPT:openai", &make_signed_entry(OPENAI_PKCE_RHAI));

        let engine = OAuthScriptEngine::load_from_keystore(&ks).await.unwrap();
        let provider = engine.get_provider("openai").unwrap();

        let mut params = HashMap::new();
        params.insert("grant_type".to_string(), "authorization_code".to_string());
        assert!(engine.call_is_token_exchange_request(provider, &params));

        params.insert("grant_type".to_string(), "refresh_token".to_string());
        assert!(engine.call_is_token_exchange_request(provider, &params));

        params.insert("grant_type".to_string(), "client_credentials".to_string());
        assert!(!engine.call_is_token_exchange_request(provider, &params));
    }

    #[tokio::test]
    async fn test_is_token_exchange_request_missing_function() {
        // GOOGLE_RHAI does not implement is_token_exchange_request → should return true
        let ks = MockKeyStore::new();
        ks.insert("OAUTH_SCRIPT:google", &make_signed_entry(GOOGLE_RHAI));

        let engine = OAuthScriptEngine::load_from_keystore(&ks).await.unwrap();
        let provider = engine.get_provider("google").unwrap();

        let params = HashMap::new();
        assert!(engine.call_is_token_exchange_request(provider, &params));
    }
}
