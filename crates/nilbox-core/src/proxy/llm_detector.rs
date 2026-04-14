//! LLM Provider Matcher — detects whether an outbound request targets a known LLM API.
//!
//! If catalog has entries: match domain+path against catalog; heuristic fallback.
//! If catalog is empty: `match_request` returns None; callers fall back to byte estimation.

use std::sync::{Arc, RwLock};

use anyhow::Result;

use crate::config_store::{ConfigStore, LlmProvider};
use crate::keystore::KeyStore;

// ── Public types ─────────────────────────────────────────────

/// Result of a successful LLM provider match.
#[derive(Debug, Clone)]
pub struct LlmProviderMatch {
    pub provider_id:   String,
    pub confidence:    String, // "confirmed" | "estimated"
    pub provider_info: Option<LlmProvider>,
}

// ── LlmProviderMatcher ───────────────────────────────────────

pub struct LlmProviderMatcher {
    providers:    RwLock<Vec<LlmProvider>>,
    keystore:     Arc<dyn KeyStore>,
    config_store: Arc<ConfigStore>,
}

impl LlmProviderMatcher {
    /// Create and load providers from DB. Free mode when table is empty.
    pub async fn new(keystore: Arc<dyn KeyStore>, config_store: Arc<ConfigStore>) -> Result<Self> {
        let mut providers = keystore.list_llm_providers().await?;
        if let Ok(custom) = config_store.list_custom_llm_providers() {
            providers.extend(custom);
        }
        Ok(Self {
            providers: RwLock::new(providers),
            keystore,
            config_store,
        })
    }

    /// Create in free mode with an explicit keystore (empty catalog).
    /// Useful when the DB is available but the table is simply empty.
    pub fn new_free_mode(keystore: Arc<dyn KeyStore>, config_store: Arc<ConfigStore>) -> Self {
        Self {
            providers: RwLock::new(Vec::new()),
            keystore,
            config_store,
        }
    }

    /// Reload providers from DB. Called after manifest update or custom provider change.
    pub async fn reload(&self) -> Result<()> {
        let mut fresh = self.keystore.list_llm_providers().await?;
        if let Ok(custom) = self.config_store.list_custom_llm_providers() {
            fresh.extend(custom);
        }
        *self.providers.write().map_err(|_| anyhow::anyhow!("RwLock poisoned"))? = fresh;
        Ok(())
    }

    /// Match an outbound request against the LLM catalog.
    ///
    /// Returns `Some(confirmed)` if catalog match, `Some(estimated)` if heuristic match.
    /// Returns `None` when catalog is empty or no match found.
    pub fn match_request(
        &self,
        domain:  &str,
        path:    &str,
        headers: &[(String, String)],
    ) -> Option<LlmProviderMatch> {
        let providers = self.providers.read().ok()?;
        if providers.is_empty() {
            return None;
        }

        // 1. Catalog match — check domain_pattern and extra_domains
        for p in providers.iter() {
            if !p.enabled {
                continue;
            }
            let domain_hit = domain_matches(domain, &p.domain_pattern)
                || p.extra_domains.iter().any(|d| domain_matches(domain, d));
            if domain_hit {
                if let Some(ref prefix) = p.path_prefix {
                    if !path.starts_with(prefix.as_str()) {
                        continue;
                    }
                }
                return Some(LlmProviderMatch {
                    provider_id:   p.provider_id.clone(),
                    confidence:    "confirmed".into(),
                    provider_info: Some(p.clone()),
                });
            }
        }

        // 2. Heuristic fallback (paid mode only)
        if is_heuristic_llm(headers) {
            return Some(LlmProviderMatch {
                provider_id:   "unknown".into(),
                confidence:    "estimated".into(),
                provider_info: None,
            });
        }

        None
    }

    /// Domain-only fallback: match domain against catalog without path check.
    /// Returns provider_id when domain matches a known provider.
    pub fn match_domain_only(&self, domain: &str) -> Option<String> {
        let providers = self.providers.read().ok()?;
        for p in providers.iter() {
            if !p.enabled { continue; }
            let hit = domain_matches(domain, &p.domain_pattern)
                || p.extra_domains.iter().any(|d| domain_matches(domain, d));
            if hit {
                return Some(p.provider_id.clone());
            }
        }
        None
    }
}

// ── Helpers ──────────────────────────────────────────────────

/// Matches `domain` against a pattern that may start with `*.`.
fn domain_matches(domain: &str, pattern: &str) -> bool {
    if let Some(suffix) = pattern.strip_prefix("*.") {
        domain == suffix || domain.ends_with(&format!(".{}", suffix))
    } else {
        domain == pattern
    }
}

/// Returns true when the request looks like an LLM API call by headers alone.
fn is_heuristic_llm(headers: &[(String, String)]) -> bool {
    let has_auth = headers.iter().any(|(k, _)| {
        let k = k.to_lowercase();
        k == "authorization" || k == "x-api-key" || k == "x-goog-api-key"
    });
    let has_json = headers.iter().any(|(k, v)| {
        k.to_lowercase() == "content-type" && v.contains("application/json")
    });
    has_auth && has_json
}
