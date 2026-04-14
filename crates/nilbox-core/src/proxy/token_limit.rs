//! TokenLimitChecker — pre-request token limit enforcement.
//!
//! warn OR block (HTTP 429) depending on the limit's `action`.
//! Soft-warning thresholds: 80 % and 95 % of the configured limit.

use std::sync::Arc;

use anyhow::Result;

use crate::config_store::{ConfigStore, LimitCheckResult};
use crate::events::{EventEmitter, emit_typed};

pub struct TokenLimitChecker {
    config_store: Arc<ConfigStore>,
    emitter:      Arc<dyn EventEmitter>,
}

impl TokenLimitChecker {
    pub fn new(config_store: Arc<ConfigStore>, emitter: Arc<dyn EventEmitter>) -> Self {
        Self { config_store, emitter }
    }

    /// Check token limits before forwarding a request.
    ///
    /// Returns:
    /// - `Ok(None)` — no limit exceeded, request may proceed.
    /// - `Ok(Some(result))` where `result.action == "block"` — caller must return HTTP 429.
    /// - `Ok(Some(result))` where `result.action == "warn"` — already emitted event, proceed.
    pub fn check_pre_request(
        &self,
        vm_id:       &str,
        provider_id: &str,
    ) -> Result<Option<LimitCheckResult>> {
        let result = self.config_store.check_token_limit(vm_id, provider_id)?;

        if let Some(ref r) = result {
            if r.action == "warn" {
                emit_typed(
                    &self.emitter,
                    "token-limit-warning",
                    &serde_json::json!({
                        "vm_id":     vm_id,
                        "provider":  provider_id,
                        "usage_pct": r.usage_pct,
                        "current":   r.current,
                        "limit":     r.limit,
                    }),
                );
                // Warn-only: let the request pass
                return Ok(None);
            }

            // action == "block"
            emit_typed(
                &self.emitter,
                "token-limit-exceeded",
                &serde_json::json!({
                    "vm_id":    vm_id,
                    "provider": provider_id,
                    "current":  r.current,
                    "limit":    r.limit,
                }),
            );
            return Ok(Some(LimitCheckResult {
                action:    "block".into(),
                usage_pct: r.usage_pct,
                current:   r.current,
                limit:     r.limit,
            }));
        }

        // Check soft-warning thresholds even when not exceeding the hard limit.
        self.check_soft_warnings(vm_id, provider_id)?;

        Ok(None)
    }

    /// Emit 80 % / 95 % soft-warning events if approaching a limit.
    pub fn check_soft_warnings(&self, vm_id: &str, provider_id: &str) -> Result<()> {
        if let Some(pct) = self.config_store.get_token_usage_pct(vm_id, provider_id)? {
            if pct >= 95.0 {
                emit_typed(
                    &self.emitter,
                    "token-limit-warning-95pct",
                    &serde_json::json!({
                        "vm_id":    vm_id,
                        "provider": provider_id,
                        "pct":      pct,
                    }),
                );
            } else if pct >= 80.0 {
                emit_typed(
                    &self.emitter,
                    "token-limit-warning-80pct",
                    &serde_json::json!({
                        "vm_id":    vm_id,
                        "provider": provider_id,
                        "pct":      pct,
                    }),
                );
            }
        }
        Ok(())
    }
}
