//! TokenUsageLogger — persists LLM token usage and emits Tauri events.

use std::sync::Arc;

use anyhow::Result;

use crate::config_store::{ConfigStore, TokenUsageLog};
use crate::events::{EventEmitter, emit_typed};
use crate::proxy::token_extractor::TokenUsageData;

pub struct TokenUsageLogger {
    config_store: Arc<ConfigStore>,
    emitter:      Arc<dyn EventEmitter>,
}

impl TokenUsageLogger {
    pub fn new(config_store: Arc<ConfigStore>, emitter: Arc<dyn EventEmitter>) -> Self {
        Self { config_store, emitter }
    }

    /// Persist one LLM request log entry and emit a Tauri event.
    pub fn log(
        &self,
        vm_id:        &str,
        provider_id:  &str,
        usage:        &TokenUsageData,
        request_path: Option<&str>,
        status_code:  Option<i32>,
        is_streaming: bool,
    ) -> Result<()> {
        let now        = chrono::Utc::now();
        let year_month = now.format("%Y-%m").to_string();

        let log = TokenUsageLog {
            id:              None,
            vm_id:           vm_id.to_string(),
            provider_id:     provider_id.to_string(),
            model:           usage.model.clone(),
            request_tokens:  usage.request_tokens as i64,
            response_tokens: usage.response_tokens as i64,
            total_tokens:    usage.total_tokens as i64,
            confidence:      usage.confidence.clone(),
            is_streaming,
            request_path:    request_path.map(|s| s.to_string()),
            status_code,
            created_at:      None,
            year_month:      Some(year_month.clone()),
        };

        self.config_store.insert_token_usage_log(&log)?;
        self.config_store.upsert_token_usage_monthly(
            vm_id,
            provider_id,
            &year_month,
            usage.request_tokens as i64,
            usage.response_tokens as i64,
        )?;

        emit_typed(
            &self.emitter,
            "token-usage-recorded",
            &serde_json::json!({
                "vm_id":    vm_id,
                "provider": provider_id,
                "tokens":   usage.total_tokens,
                "model":    usage.model,
            }),
        );

        Ok(())
    }
}
