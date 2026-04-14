//! ScriptedOAuthDelegator — Rhai script-driven OAuth token exchange.
//!
//! Replaces GoogleOAuthDelegator with a generic delegator that uses
//! OAuthScriptEngine to determine field substitutions and target URL.

use super::AuthDelegator;
use crate::keystore::KeyStore;
use crate::proxy::oauth_script_engine::{
    OAuthScriptEngine, resolve_json_path_with_fallback,
};

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::debug;

pub struct ScriptedOAuthDelegator {
    engine: Arc<OAuthScriptEngine>,
    keystore: Arc<dyn KeyStore>,
}

impl ScriptedOAuthDelegator {
    pub fn new(engine: Arc<OAuthScriptEngine>, keystore: Arc<dyn KeyStore>) -> Self {
        Self { engine, keystore }
    }
}

#[async_trait]
impl AuthDelegator for ScriptedOAuthDelegator {
    fn kind(&self) -> &str {
        "scripted-oauth"
    }

    async fn apply_auth(
        &self,
        request: &mut reqwest::Request,
        _domain: &str,
        credential_account: &str,
    ) -> Result<()> {
        // Find the provider by token_path or by name prefix
        let provider = self.engine.providers()
            .find(|p| p.info.token_path == credential_account)
            .or_else(|| {
                // Fallback: match by OAUTH_{NAME}_ prefix pattern
                self.engine.providers().find(|p| {
                    let prefix = format!("OAUTH_{}_", p.info.name.to_uppercase());
                    credential_account.starts_with(&prefix)
                })
            })
            .ok_or_else(|| anyhow!("No OAuth script found for credential: {}", credential_account))?;

        // 1. Get placeholder extraction instructions (JSON paths)
        let extraction = self.engine.call_placeholder_extraction_instructions(provider)?;

        // 2. Read secret JSON from KeyStore. Try in order:
        //    a) credential_account (provider's token_path, e.g. OAUTH_GOOGLE_FILE)
        //    b) oauth:{provider_name} (used by Credentials screen input/json save)
        //    c) any keystore key matching OAUTH_{NAME}_ prefix
        let oauth_key = format!("oauth:{}", provider.info.name);
        let secret_json = match self.keystore.get(credential_account).await {
            Ok(json) => json,
            Err(_) => match self.keystore.get(&oauth_key).await {
                Ok(json) => json,
                Err(_) => {
                    let prefix = format!("OAUTH_{}_", provider.info.name.to_uppercase());
                    let mut found = None;
                    if let Ok(keys) = self.keystore.list().await {
                        for key in keys {
                            if key.starts_with(&prefix) {
                                if let Ok(json) = self.keystore.get(&key).await {
                                    found = Some(json);
                                    break;
                                }
                            }
                        }
                    }
                    found.ok_or_else(|| anyhow!("Failed to read {} from KeyStore: Key not found", credential_account))?
                }
            },
        };

        let secret_doc: serde_json::Value = serde_json::from_str(&secret_json)
            .map_err(|e| anyhow!("Invalid secret JSON in KeyStore for {}: {}", credential_account, e))?;

        // 3. Extract real values using JSON paths
        let mut real_values: HashMap<String, String> = HashMap::new();
        for (placeholder_name, json_path) in &extraction {
            let value = resolve_json_path_with_fallback(&secret_doc, json_path)
                .ok_or_else(|| anyhow!("JSON path '{}' not found in secret for placeholder '{}'", json_path, placeholder_name))?;
            real_values.insert(placeholder_name.clone(), value);
        }

        // 4. Parse POST body
        let body_bytes = request.body()
            .and_then(|b| b.as_bytes())
            .ok_or_else(|| anyhow!("Missing POST body for token request"))?
            .to_vec();

        let params: HashMap<String, String> = url::form_urlencoded::parse(&body_bytes)
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();

        // 5. Get token request instructions from script
        let instructions = self.engine.call_build_token_request_instructions(provider, &params)?;

        // 6. Validate redirect_uri
        if let Some(redirect_uri) = params.get("redirect_uri") {
            if let Ok(parsed) = url::Url::parse(redirect_uri) {
                let host = parsed.host_str().unwrap_or("");
                if !instructions.allowed_redirect_hosts.iter().any(|h| h == host) {
                    return Err(anyhow!("redirect_uri host '{}' not in allowed list", host));
                }
            }
        }

        // 7. Build new body with substituted values
        let params_vec: Vec<(String, String)> = url::form_urlencoded::parse(&body_bytes)
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();

        let new_body: String = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(params_vec.iter().map(|(k, v)| {
                let new_v = if let Some(placeholder_name) = instructions.field_substitutions.get(k.as_str()) {
                    real_values.get(placeholder_name).cloned().unwrap_or_else(|| v.clone())
                } else {
                    v.clone()
                };
                (k.as_str(), new_v)
            }))
            .finish();

        debug!("ScriptedOAuthDelegator [{}]: substituted fields, rewriting URL", provider.info.name);

        // 8. Rewrite URL to real token endpoint
        *request.url_mut() = reqwest::Url::parse(&instructions.target_url)
            .map_err(|e| anyhow!("Failed to parse target_url '{}': {}", instructions.target_url, e))?;

        // 9. Replace request body
        *request.body_mut() = Some(reqwest::Body::from(new_body));

        Ok(())
    }

    fn credential_description(&self) -> &str {
        "OAuth Client Secret (via script engine)"
    }
}
