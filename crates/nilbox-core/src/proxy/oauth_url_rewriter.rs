//! OAuth URL rewriter — replace dummy credentials with real ones from KeyStore
//!
//! Uses OAuthScriptEngine to find the appropriate provider and rewrite URLs.

use crate::config_store::ConfigStore;
use crate::keystore::KeyStore;
use crate::proxy::oauth_script_engine::{
    OAuthScriptEngine, resolve_json_path_with_fallback,
};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::warn;

/// Registry of domain-specific OAuth URL rewriters (engine-backed).
/// Uses a live reference to the shared engine so script updates are picked up immediately.
pub struct OAuthUrlRewriter {
    engine: Arc<RwLock<Arc<OAuthScriptEngine>>>,
}

impl OAuthUrlRewriter {
    pub fn new(engine: Arc<RwLock<Arc<OAuthScriptEngine>>>) -> Self {
        Self { engine }
    }

    /// Rewrite dummy credentials in the URL by looking up the appropriate
    /// provider script and fetching the secret from KeyStore.
    pub async fn rewrite(
        &self,
        url: &str,
        domain: &str,
        config_store: &ConfigStore,
        keystore: &dyn KeyStore,
        token_overrides: &[String],
    ) -> Result<String> {
        // Read the live engine snapshot
        let engine = self.engine.read().await.clone();

        // Find matching provider for this domain
        let provider = match engine.find_by_auth_domain(domain) {
            Some(p) => p,
            None => return Ok(url.to_string()),
        };

        // Use overrides if provided, otherwise look up from DB,
        // falling back to the provider's token_path (OAuth secret key in KeyStore)
        let mut token_accounts = if !token_overrides.is_empty() {
            token_overrides.to_vec()
        } else {
            config_store.list_domain_tokens(domain)?
        };
        if token_accounts.is_empty() {
            // For OAuth providers, the secret is stored under token_path in KeyStore
            token_accounts.push(provider.info.token_path.clone());
            // Also check oauth:{provider_name} pattern (used by frontend Mappings.tsx)
            let oauth_key = format!("oauth:{}", provider.info.name);
            if !token_accounts.contains(&oauth_key) {
                if keystore.has(&oauth_key).await.unwrap_or(false) {
                    token_accounts.push(oauth_key);
                }
            }
            // Also search keystore for keys matching OAUTH_{NAME}_ prefix
            // (cached script may have outdated token_path)
            let prefix = format!("OAUTH_{}_", provider.info.name.to_uppercase());
            if let Ok(all_keys) = keystore.list().await {
                for key in all_keys {
                    if key.starts_with(&prefix) && !token_accounts.contains(&key) {
                        token_accounts.push(key);
                    }
                }
            }
        }

        // Get placeholder extraction instructions
        let extraction = match engine.call_placeholder_extraction_instructions(provider) {
            Ok(e) => e,
            Err(e) => {
                warn!("oauth_rewrite: placeholder_extraction_instructions failed: {}", e);
                return Ok(url.to_string());
            }
        };
        if extraction.is_empty() {
            return Ok(url.to_string());
        }

        // Try each token_account until one works
        for account in &token_accounts {
            match keystore.get(account).await {
                Ok(secret_json) => {
                    let secret_doc: serde_json::Value = match serde_json::from_str(&secret_json) {
                        Ok(v) => v,
                        Err(e) => {
                            // Maybe it's a double-encoded JSON string
                            match serde_json::from_str::<String>(&secret_json) {
                                Ok(inner) => match serde_json::from_str(&inner) {
                                    Ok(v) => v,
                                    Err(_) => {
                                        warn!("oauth_rewrite: double-decode failed for '{}': {}", account, e);
                                        continue;
                                    }
                                }
                                Err(_) => {
                                    warn!("oauth_rewrite: invalid JSON for '{}': {}", account, e);
                                    continue;
                                }
                            }
                        }
                    };

                    // Extract real values using JSON paths
                    let mut placeholders: HashMap<String, String> = HashMap::new();
                    let mut all_found = true;
                    for (placeholder_name, json_path) in &extraction {
                        match resolve_json_path_with_fallback(&secret_doc, json_path) {
                            Some(value) => {
                                placeholders.insert(placeholder_name.clone(), value);
                            }
                            None => {
                                warn!("oauth_rewrite: JSON path '{}' not found for placeholder '{}'", json_path, placeholder_name);
                                all_found = false;
                                break;
                            }
                        }
                    }
                    if !all_found {
                        continue;
                    }

                    // Call rewrite_auth_url script function
                    match engine.call_rewrite_auth_url(provider, url, &placeholders) {
                        Ok(rewritten) if rewritten != url => return Ok(rewritten),
                        Ok(_) => {} // script didn't rewrite — fall through to generic fallback
                        Err(e) => {
                            warn!("oauth_rewrite: rewrite_auth_url failed: {}", e);
                        }
                    }

                    // Generic fallback: {placeholder_prefix}_{KEY} → real value
                    let mut result = url.to_string();
                    for (key, value) in &placeholders {
                        let placeholder = format!("{}_{}", provider.info.placeholder_prefix, key);
                        result = result.replace(&placeholder, value);
                    }
                    if result != url {
                        return Ok(result);
                    }
                }
                Err(e) => {
                    warn!("oauth_rewrite: failed to read keystore for {}: {}", account, e);
                    continue;
                }
            }
        }

        Ok(url.to_string())
    }
}
