//! Auth delegator — Strategy Pattern for vendor-specific auth injection.
//!
//! Each `AuthDelegator` implementation knows how to transform an outgoing
//! HTTP request so that it carries valid credentials for its target service.

pub mod aws_sigv4;
pub mod bearer;
pub mod scripted_oauth;
pub mod telegram;

use crate::config_store::ConfigStore;
use crate::keystore::KeyStore;
use crate::proxy::api_key_gate::ApiKeyGate;
use crate::proxy::oauth_token_vault::OAuthTokenVault;

use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

/// Strategy trait for vendor-specific auth injection.
#[async_trait]
pub trait AuthDelegator: Send + Sync {
    /// Unique identifier for this delegator type (e.g., "bearer", "aws-sigv4").
    fn kind(&self) -> &str;

    /// Inspect the request and inject/replace auth headers/signature.
    ///
    /// * `request` — mutable reference to the reqwest::Request about to be sent
    /// * `domain` — target domain (e.g., "api.openai.com")
    /// * `credential_account` — the keychain account name holding the credential
    async fn apply_auth(
        &self,
        request: &mut reqwest::Request,
        domain: &str,
        credential_account: &str,
    ) -> Result<()>;

    /// Human-readable description of what credentials this delegator needs.
    fn credential_description(&self) -> &str;
}

/// Create a delegator instance by its kind identifier.
///
/// Returns `None` for unrecognized kinds.
pub fn create_delegator(
    kind: &str,
    api_key_gate: &Arc<ApiKeyGate>,
    keystore: &Arc<dyn KeyStore>,
    config_store: &Arc<ConfigStore>,
    oauth_vault: Option<Arc<OAuthTokenVault>>,
) -> Option<Arc<dyn AuthDelegator>> {
    match kind {
        "bearer" => Some(Arc::new(bearer::BearerDelegator::new(api_key_gate.clone(), oauth_vault))),
        "aws-sigv4" => Some(Arc::new(aws_sigv4::AwsSigV4Delegator::new(
            keystore.clone(),
            config_store.clone(),
        ))),
        "telegram" => Some(Arc::new(telegram::TelegramDelegator::new(keystore.clone()))),
        _ => None,
    }
}
