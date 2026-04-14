//! BearerDelegator — Engine A: simple header substitution.
//!
//! Wraps the existing `ApiKeyGate` logic: looks up a real API key from the
//! keystore (prompting the user if missing) and replaces the dummy auth header
//! value with the real secret.

use super::AuthDelegator;
use crate::proxy::api_key_gate::ApiKeyGate;
use crate::proxy::oauth_token_vault::OAuthTokenVault;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use std::sync::Arc;
use tracing::warn;

pub struct BearerDelegator {
    api_key_gate: Arc<ApiKeyGate>,
    oauth_vault: Option<Arc<OAuthTokenVault>>,
}

impl BearerDelegator {
    pub fn new(api_key_gate: Arc<ApiKeyGate>, oauth_vault: Option<Arc<OAuthTokenVault>>) -> Self {
        Self { api_key_gate, oauth_vault }
    }
}

#[async_trait]
impl AuthDelegator for BearerDelegator {
    fn kind(&self) -> &str {
        "bearer"
    }

    async fn apply_auth(
        &self,
        request: &mut reqwest::Request,
        domain: &str,
        credential_account: &str,
    ) -> Result<()> {
        // OAuth dummy token detection: resolve to real token and replace header
        if let Some(ref vault) = self.oauth_vault {
            if let Some(existing) = request.headers().get("authorization").cloned() {
                let val = existing.to_str().unwrap_or("");
                let bearer_value = val.strip_prefix("Bearer ")
                    .or_else(|| val.strip_prefix("token "));
                if let Some(token) = bearer_value {
                    if OAuthTokenVault::is_dummy_access_token(token) {
                        match vault.resolve_access_token(token).await {
                            Ok(Some(real)) => {
                                let prefix = if val.starts_with("Bearer ") { "Bearer " } else { "token " };
                                request.headers_mut().insert(
                                    "authorization",
                                    format!("{}{}", prefix, real).parse()?,
                                );
                                return Ok(());
                            }
                            Ok(None) => {
                                warn!("OAuth dummy token not found in vault: {}", token);
                            }
                            Err(e) => {
                                warn!("Failed to resolve OAuth dummy token: {}", e);
                            }
                        }
                    }
                }
            }
        }

        let real_key = self
            .api_key_gate
            .get_or_prompt(credential_account, domain)
            .await
            .ok_or_else(|| anyhow!("User cancelled API key prompt"))?;

        // Detect which auth header is present and replace its value
        if let Some(existing) = request.headers().get("authorization").cloned() {
            let val = existing.to_str().unwrap_or("");
            let prefix = if val.starts_with("Bearer ") {
                "Bearer "
            } else if val.starts_with("token ") {
                "token "
            } else if val.starts_with("Basic ") {
                "Basic "
            } else {
                ""
            };
            request.headers_mut().insert(
                "authorization",
                format!("{}{}", prefix, real_key).parse()?,
            );
        } else if request.headers().contains_key("x-api-key") {
            request
                .headers_mut()
                .insert("x-api-key", real_key.parse()?);
        } else if request.headers().contains_key("x-goog-api-key") {
            request
                .headers_mut()
                .insert("x-goog-api-key", real_key.parse()?);
        } else {
            // No auth header in the request — inject as Bearer by default
            request.headers_mut().insert(
                "authorization",
                format!("Bearer {}", real_key).parse()?,
            );
        }

        Ok(())
    }

    fn credential_description(&self) -> &str {
        "API Key (Bearer token, x-api-key, or x-goog-api-key)"
    }
}
