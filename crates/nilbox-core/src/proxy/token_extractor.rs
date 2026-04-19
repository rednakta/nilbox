//! Token extraction from LLM API request/response bodies.
//!
//! - Non-streaming: parse JSON body → sum response_token_field paths
//! - Streaming (SSE): scan data: lines for the last usage-bearing chunk
//! - Byte estimate: fallback when no provider config or parse failures

use crate::config_store::LlmProvider;
use serde::Serialize;

// ── Public data type ─────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct TokenUsageData {
    pub request_tokens:  u32,
    pub response_tokens: u32,
    pub total_tokens:    u32,
    pub model:           Option<String>,
    /// "confirmed" | "estimated" | "byte_estimate" | "unknown"
    pub confidence:      String,
}

// ── JSON body extraction ──────────────────────────────────────

/// Extract token counts from a non-streaming JSON response body.
pub fn extract_from_body(body: &[u8], provider: &LlmProvider) -> Option<TokenUsageData> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;

    let response_tokens = sum_token_paths(
        &value,
        provider.response_token_field.as_deref().unwrap_or(""),
    );
    let request_tokens = sum_token_paths(
        &value,
        provider.request_token_field.as_deref().unwrap_or(""),
    );
    let model = provider
        .model_field
        .as_deref()
        .and_then(|path| resolve_json_string(&value, path));

    let total = request_tokens + response_tokens;
    Some(TokenUsageData {
        request_tokens,
        response_tokens,
        total_tokens: total,
        model,
        confidence: "confirmed".into(),
    })
}

/// Extract token counts from collected SSE chunks (streaming response).
///
/// Scans data: lines in reverse to find the last chunk that contains usage data.
pub fn extract_from_sse_chunks(chunks: &[Vec<u8>], provider: &LlmProvider) -> Option<TokenUsageData> {
    // Concatenate all chunks into a single text buffer
    let mut buf = Vec::new();
    for c in chunks {
        buf.extend_from_slice(c);
    }
    let text = String::from_utf8_lossy(&buf);

    // Collect all `data: {...}` lines (skip `data: [DONE]`)
    let data_lines: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("data: ") && !l.contains("[DONE]"))
        .collect();

    // Walk backwards to find the last chunk that has token usage
    let resp_field = provider.response_token_field.as_deref().unwrap_or("");
    let req_field  = provider.request_token_field.as_deref().unwrap_or("");
    let mdl_field  = provider.model_field.as_deref().unwrap_or("model");

    for line in data_lines.iter().rev() {
        let json_str = line.trim_start_matches("data: ");
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(json_str) {
            let response_tokens = sum_token_paths(&value, resp_field);
            let request_tokens  = sum_token_paths(&value, req_field);

            if response_tokens > 0 || request_tokens > 0 {
                let model = resolve_json_string(&value, mdl_field);
                let total = request_tokens + response_tokens;
                return Some(TokenUsageData {
                    request_tokens,
                    response_tokens,
                    total_tokens: total,
                    model,
                    confidence: "confirmed".into(),
                });
            }
        }
    }
    None
}

/// Estimate token counts from raw byte lengths when no provider config (1 token ≈ 4 bytes).
pub fn estimate_from_bytes(request_body_len: usize, response_body_len: usize) -> TokenUsageData {
    let req  = (request_body_len  / 4) as u32;
    let resp = (response_body_len / 4) as u32;
    TokenUsageData {
        request_tokens:  req,
        response_tokens: resp,
        total_tokens:    req + resp,
        model:           None,
        confidence:      "byte_estimate".into(),
    }
}

/// Byte-based fallback for parse failures when provider is configured.
pub fn estimate_from_bytes_fallback(body_len: usize) -> TokenUsageData {
    let tokens = (body_len / 4) as u32;
    TokenUsageData {
        request_tokens:  0,
        response_tokens: tokens,
        total_tokens:    tokens,
        model:           None,
        confidence:      "unknown".into(),
    }
}

// ── Helpers ──────────────────────────────────────────────────

/// Pick the first non-zero value from a comma-separated list of JSON dot-paths.
///
/// Comma-separated paths act as fallback alternatives, not a sum.
/// Example: `"response.usage.output_tokens,usage.completion_tokens"` → try each
/// path in order and return the first value > 0.
fn sum_token_paths(value: &serde_json::Value, paths: &str) -> u32 {
    if paths.is_empty() {
        return 0;
    }
    for p in paths.split(',') {
        if let Some(v) = resolve_json_u32(value, p.trim()) {
            if v > 0 {
                return v;
            }
        }
    }
    0
}

/// Resolve a dot-separated JSON path to a u32 value.
pub fn resolve_json_u32(value: &serde_json::Value, path: &str) -> Option<u32> {
    let mut cur = value;
    for key in path.split('.') {
        cur = cur.get(key)?;
    }
    cur.as_u64().map(|n| n as u32)
}

/// Resolve a dot-separated JSON path to a String value.
fn resolve_json_string(value: &serde_json::Value, path: &str) -> Option<String> {
    let mut cur = value;
    for key in path.split('.') {
        cur = cur.get(key)?;
    }
    cur.as_str().map(|s| s.to_string())
}
