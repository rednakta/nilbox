//! DomainGate — runtime domain access control for HTTPS CONNECT proxy.
//!
//! When the VM makes an HTTPS request to an unknown domain:
//! - Emits `domain-access-request` event to frontend
//! - Blocks the connection until user picks Allow Once / Allow Always / Deny (60s timeout)
//! - Deduplicates concurrent requests: all waiters on the same domain share one watch channel

use crate::config_store::{ConfigStore, parent_domains};
use crate::events::{EventEmitter, emit_typed};

/// Check if domain matches an entry in the set.
/// Supports: exact match, parent-domain suffix match, and wildcard `*.suffix` patterns.
/// e.g. "*.amazonaws.com" matches "bedrock-runtime.us-east-1.amazonaws.com"
fn matches_domain_set(set: &std::collections::HashSet<String>, domain: &str) -> bool {
    if set.contains(domain) {
        return true;
    }
    if parent_domains(domain).iter().any(|p| set.contains(*p)) {
        return true;
    }
    // Wildcard: "*.amazonaws.com" matches "bedrock-runtime.us-east-1.amazonaws.com"
    set.iter().any(|entry| {
        if let Some(suffix) = entry.strip_prefix("*.") {
            domain == suffix || domain.ends_with(&format!(".{}", suffix))
        } else {
            false
        }
    })
}

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{watch, Mutex, RwLock};
use tracing::{debug};

use nilbox_blocklist::{BloomBlocklist, BlocklistInfo};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainDecision {
    Pending,
    AllowOnce,
    AllowAlways,
    Deny,
}

pub struct DomainGate {
    config_store: Arc<ConfigStore>,
    emitter: Arc<dyn EventEmitter>,
    /// Persistent allowlist (stored in domain_allowlist table)
    allow_always: Mutex<HashSet<String>>,
    /// Persistent denylist (stored in domain_denylist table)
    deny_always: Mutex<HashSet<String>>,
    /// Pending user decisions — deduplicated by domain
    pending: Mutex<HashMap<String, watch::Sender<DomainDecision>>>,
    /// In-memory token overrides for "allow_once" (consumed by take_pending_tokens)
    pending_tokens: Mutex<HashMap<String, Vec<String>>>,
    /// Bloom filter blocklist — hot-swappable after download/build
    blocklist: RwLock<Option<Arc<BloomBlocklist>>>,
}

impl DomainGate {
    pub fn new(config_store: Arc<ConfigStore>, emitter: Arc<dyn EventEmitter>) -> Self {
        let allow_always = config_store
            .list_allowlist_domains()
            .unwrap_or_default()
            .into_iter()
            .collect();

        let deny_always = config_store
            .list_denylist_domains()
            .unwrap_or_default()
            .into_iter()
            .collect();

        Self {
            config_store,
            emitter,
            allow_always: Mutex::new(allow_always),
            deny_always: Mutex::new(deny_always),
            pending: Mutex::new(HashMap::new()),
            pending_tokens: Mutex::new(HashMap::new()),
            blocklist: RwLock::new(None),
        }
    }

    /// Returns true if the domain may connect.
    /// May block up to 60 seconds waiting for user decision.
    pub async fn check(&self, domain: &str, port: u16, vm_id: &str, source: &str) -> bool {
        // 0. system domains — always allowed, cannot be denied
        if crate::config_store::SYSTEM_ALLOWLIST_DOMAINS.contains(&domain) {
            return true;
        }
        // 1. user-deny — immediate reject (exact + subdomain + wildcard match)
        {
            let deny_set = self.deny_always.lock().await;
            if matches_domain_set(&deny_set, domain) {
                return false;
            }
        }
        // 2. user-allow — immediate pass (exact + subdomain + wildcard match)
        {
            let allow_set = self.allow_always.lock().await;
            if matches_domain_set(&allow_set, domain) {
                return true;
            }
        }
        // 3. blocklist bloom filter — block and log to DB
        if let Some(ref bl) = *self.blocklist.read().await {
            if bl.contains(domain) {
                debug!("domain_gate: blocklist denied {}", domain);
                emit_typed(
                    &self.emitter,
                    "domain-blocked",
                    &serde_json::json!({
                        "domain": domain,
                        "port": port,
                        "vm_id": vm_id,
                    }),
                );
                let _ = self.config_store.insert_block_log(vm_id, domain, port);
                return false;
            }
        }

        // 4. Need user decision — deduplicate concurrent requests for the same domain
        let rx = {
            let mut pending = self.pending.lock().await;
            if let Some(tx) = pending.get(domain) {
                tx.subscribe()
            } else {
                let (tx, rx) = watch::channel(DomainDecision::Pending);
                pending.insert(domain.to_string(), tx);
                drop(pending);
                emit_typed(
                    &self.emitter,
                    "domain-access-request",
                    &serde_json::json!({
                        "domain": domain,
                        "port": port,
                        "vm_id": vm_id,
                        "source": source,
                    }),
                );
                rx
            }
        };

        let decision = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            Self::wait_for_decision(rx),
        )
        .await
        .unwrap_or(DomainDecision::Deny);

        // On timeout, clean up the pending entry
        if decision == DomainDecision::Deny {
            self.pending.lock().await.remove(domain);
        }

        matches!(decision, DomainDecision::AllowOnce | DomainDecision::AllowAlways)
    }

    async fn wait_for_decision(mut rx: watch::Receiver<DomainDecision>) -> DomainDecision {
        loop {
            if rx.changed().await.is_err() {
                return DomainDecision::Deny;
            }
            let val = rx.borrow().clone();
            if val != DomainDecision::Pending {
                return val;
            }
        }
    }

    /// Called from Tauri command when the user selects a decision.
    /// env_names are stored in-memory for consumption by take_pending_tokens().
    pub async fn resolve(&self, domain: &str, decision: DomainDecision, env_names: Vec<String>) {
        if !env_names.is_empty() {
            debug!("domain_gate: stored {} pending_tokens for {}", env_names.len(), domain);
            self.pending_tokens.lock().await.insert(domain.to_string(), env_names);
        }
        match &decision {
            DomainDecision::AllowOnce => {
                // Do NOT add to allow_once set — the decision is delivered via the
                // watch channel to only the currently pending waiter(s).
                // Adding to the set would cause subsequent requests to auto-pass.
            }
            DomainDecision::AllowAlways => {
                self.allow_always.lock().await.insert(domain.to_string());
                let _ = self.config_store.insert_allowlist_domain_with_token(domain, None);
            }
            DomainDecision::Deny => {
                self.deny_always.lock().await.insert(domain.to_string());
                let _ = self.config_store.insert_denylist_domain(domain);
            }
            _ => {}
        }
        if let Some(tx) = self.pending.lock().await.remove(domain) {
            let _ = tx.send(decision);
        }
    }

    /// Check if there are pending token overrides for a domain (non-consuming).
    pub async fn has_pending_tokens(&self, domain: &str) -> bool {
        let result = self.pending_tokens.lock().await.get(domain).map(|v| !v.is_empty()).unwrap_or(false);
        debug!("domain_gate: has_pending_tokens({}) = {}", domain, result);
        result
    }

    /// Take pending token overrides for a domain (consumed once).
    pub async fn take_pending_tokens(&self, domain: &str) -> Vec<String> {
        let result = self.pending_tokens.lock().await.remove(domain).unwrap_or_default();
        // debug!("domain_gate: take_pending_tokens({}) → {} tokens", domain, result.len());
        result
    }

    /// Add a domain to the persistent allowlist.
    pub async fn add_always(&self, domain: &str, token_account: Option<&str>) {
        self.allow_always.lock().await.insert(domain.to_string());
        let _ = self.config_store.insert_allowlist_domain_with_token(domain, token_account);
    }

    /// Add a domain to the persistent allowlist with a specific TLS inspection mode.
    pub async fn add_always_with_mode(
        &self,
        domain: &str,
        token_account: Option<&str>,
        mode: crate::config_store::InspectMode,
    ) {
        self.allow_always.lock().await.insert(domain.to_string());
        let _ = self.config_store.insert_allowlist_domain_with_mode(domain, token_account, mode);
    }

    /// Add a domain to the persistent denylist.
    pub async fn add_deny(&self, domain: &str) {
        self.deny_always.lock().await.insert(domain.to_string());
        let _ = self.config_store.insert_denylist_domain(domain);
    }

    /// Remove a domain from the persistent allowlist.
    pub async fn remove_always(&self, domain: &str) {
        self.allow_always.lock().await.remove(domain);
        let _ = self.config_store.delete_allowlist_domain(domain);
    }

    /// List all persistently allowed domains, sorted.
    pub async fn list_allowlist(&self) -> Vec<String> {
        let mut v: Vec<_> = self.allow_always.lock().await.iter().cloned().collect();
        v.sort();
        v
    }

    /// Remove a domain from the persistent denylist.
    pub async fn remove_deny(&self, domain: &str) {
        self.deny_always.lock().await.remove(domain);
        let _ = self.config_store.delete_denylist_domain(domain);
    }

    /// List all persistently denied domains, sorted.
    pub async fn list_denylist(&self) -> Vec<String> {
        let mut v: Vec<_> = self.deny_always.lock().await.iter().cloned().collect();
        v.sort();
        v
    }

    /// Non-blocking check: is this domain in the persistent allowlist?
    /// Used by inspect logic to decide whether to intercept TLS.
    /// Supports exact match, parent-domain suffix match, and wildcard `*.suffix` patterns.
    pub async fn is_allowed(&self, domain: &str) -> bool {
        let set = self.allow_always.lock().await;
        matches_domain_set(&set, domain)
    }

    /// Hot-swap the blocklist (call after download or local build completes).
    pub async fn set_blocklist(&self, blocklist: Option<Arc<BloomBlocklist>>) {
        *self.blocklist.write().await = blocklist;
    }

    /// Metadata for UI display. Returns None if no blocklist is loaded.
    pub async fn blocklist_info(&self) -> Option<BlocklistInfo> {
        self.blocklist.read().await.as_ref().map(|bl| BlocklistInfo::from(bl.clone()))
    }
}
