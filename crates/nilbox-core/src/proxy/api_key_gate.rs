//! ApiKeyGate — runtime API key resolution with user prompt fallback.
//!
//! When the reverse proxy detects an auth header (x-api-key / authorization):
//! 1. Looks up the header value (= account name) in the KeyStore
//! 2. If found, returns the real secret immediately
//! 3. If not found, emits `api-key-request` to the frontend and blocks
//!    until the user provides a key (or cancels / 120s timeout)
//! 4. Deduplicates concurrent requests for the same account

use crate::events::{EventEmitter, emit_typed};
use crate::keystore::KeyStore;

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{watch, Mutex};

#[derive(Debug, Clone, PartialEq, Eq)]
enum ApiKeyResponse {
    Pending,
    Provided(String),
    Cancelled,
}

pub struct ApiKeyGate {
    keystore: Arc<dyn KeyStore>,
    emitter: Arc<dyn EventEmitter>,
    pending: Mutex<HashMap<String, watch::Sender<ApiKeyResponse>>>,
}

impl ApiKeyGate {
    pub fn new(keystore: Arc<dyn KeyStore>, emitter: Arc<dyn EventEmitter>) -> Self {
        Self {
            keystore,
            emitter,
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// Look up the real API key for `account`.
    /// If the key is not in the keystore, emit a UI prompt and wait up to 120s.
    /// Returns `Some(key)` on success, `None` if cancelled or timed out.
    pub async fn get_or_prompt(&self, account: &str, domain: &str) -> Option<String> {
        // 1. Fast path — key already in keystore
        if let Ok(key) = self.keystore.get(account).await {
            return Some(key);
        }

        // 2. Slow path — ask the user via frontend modal
        let rx = {
            let mut pending = self.pending.lock().await;
            if let Some(tx) = pending.get(account) {
                // Another request for the same account is already waiting — share channel
                tx.subscribe()
            } else {
                let (tx, rx) = watch::channel(ApiKeyResponse::Pending);
                pending.insert(account.to_string(), tx);
                drop(pending);

                // Emit event to frontend
                emit_typed(
                    &self.emitter,
                    "api-key-request",
                    &serde_json::json!({
                        "account": account,
                        "domain": domain,
                    }),
                );
                rx
            }
        };

        // 3. Wait for user response (120s timeout)
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            Self::wait_for_response(rx),
        )
        .await
        .unwrap_or(ApiKeyResponse::Cancelled);

        // On timeout/cancel, clean up pending entry
        if response == ApiKeyResponse::Cancelled {
            self.pending.lock().await.remove(account);
        }

        match response {
            ApiKeyResponse::Provided(key) => Some(key),
            _ => None,
        }
    }

    async fn wait_for_response(mut rx: watch::Receiver<ApiKeyResponse>) -> ApiKeyResponse {
        loop {
            if rx.changed().await.is_err() {
                return ApiKeyResponse::Cancelled;
            }
            let val = rx.borrow().clone();
            if val != ApiKeyResponse::Pending {
                return val;
            }
        }
    }

    /// Called from the Tauri command when the user submits or cancels the modal.
    /// `key`: Some(secret) to save & provide, None to cancel.
    pub async fn resolve(&self, account: &str, key: Option<String>) {
        let response = if let Some(ref secret) = key {
            // Save to keystore for future requests
            let _ = self.keystore.set(account, secret).await;
            ApiKeyResponse::Provided(secret.clone())
        } else {
            ApiKeyResponse::Cancelled
        };

        if let Some(tx) = self.pending.lock().await.remove(account) {
            let _ = tx.send(response);
        }
    }
}
