//! TokenMismatchGate — runtime token mismatch warning for HTTPS requests.
//!
//! When the VM makes an HTTPS request to a whitelisted domain with mapped tokens,
//! but the request's auth header value doesn't match any of those mapped tokens:
//! - Emits `token-mismatch-warning` event to frontend
//! - Blocks the connection until user picks Send Anyway / Cancel (30s timeout)
//! - On timeout, defaults to "pass through" (sends request unchanged)

use crate::events::{EventEmitter, emit_typed};

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{watch, Mutex};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenMismatchDecision {
    Pending,
    PassThrough,
    Block,
}

pub struct TokenMismatchGate {
    emitter: Arc<dyn EventEmitter>,
    /// Pending user decisions — deduplicated by request_id
    pending: Mutex<HashMap<String, watch::Sender<TokenMismatchDecision>>>,
}

impl TokenMismatchGate {
    pub fn new(emitter: Arc<dyn EventEmitter>) -> Self {
        Self {
            emitter,
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// Check for token mismatch and prompt user.
    /// Returns true if the request should proceed (pass through), false if blocked.
    /// On timeout (30s), defaults to pass through.
    pub async fn check(
        &self,
        domain: &str,
        request_account: &str,
        mapped_tokens: &[String],
    ) -> bool {
        let request_id = uuid::Uuid::new_v4().to_string();

        let rx = {
            let mut pending = self.pending.lock().await;
            let (tx, rx) = watch::channel(TokenMismatchDecision::Pending);
            pending.insert(request_id.clone(), tx);
            drop(pending);

            emit_typed(
                &self.emitter,
                "token-mismatch-warning",
                &serde_json::json!({
                    "request_id": request_id,
                    "domain": domain,
                    "request_account": request_account,
                    "mapped_tokens": mapped_tokens,
                }),
            );
            rx
        };

        let decision = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            Self::wait_for_decision(rx),
        )
        .await
        .unwrap_or(TokenMismatchDecision::PassThrough); // Timeout → pass through

        // Clean up pending entry
        self.pending.lock().await.remove(&request_id);

        matches!(decision, TokenMismatchDecision::PassThrough)
    }

    async fn wait_for_decision(mut rx: watch::Receiver<TokenMismatchDecision>) -> TokenMismatchDecision {
        loop {
            if rx.changed().await.is_err() {
                return TokenMismatchDecision::PassThrough;
            }
            let val = rx.borrow().clone();
            if val != TokenMismatchDecision::Pending {
                return val;
            }
        }
    }

    /// Called from Tauri command when the user selects a decision.
    pub async fn resolve(&self, request_id: &str, decision: TokenMismatchDecision) {
        if let Some(tx) = self.pending.lock().await.remove(request_id) {
            let _ = tx.send(decision);
        }
    }
}
