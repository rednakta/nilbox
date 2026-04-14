//! AuthRouter — domain-based routing to auth delegators.
//!
//! Maps domain patterns (glob-style) to `AuthDelegator` implementations.
//! Falls back to `BearerDelegator` for domains without explicit mapping.

use super::auth_delegator::AuthDelegator;

use std::sync::Arc;
use tokio::sync::RwLock;

/// A single routing rule: domain pattern → delegator + credential account.
pub struct AuthRoute {
    /// Glob pattern matching domain (e.g., "*.amazonaws.com", "api.openai.com")
    pub domain_pattern: String,
    /// Which auth engine to use
    pub delegator: Arc<dyn AuthDelegator>,
    /// KeyStore account name holding the credential for this route
    pub credential_account: String,
}

pub struct AuthRouter {
    routes: RwLock<Vec<AuthRoute>>,
    default_delegator: Arc<dyn AuthDelegator>,
}

impl AuthRouter {
    pub fn new(default_delegator: Arc<dyn AuthDelegator>) -> Self {
        Self {
            routes: RwLock::new(Vec::new()),
            default_delegator,
        }
    }

    /// Find the delegator + credential for a given domain.
    /// Returns (delegator, credential_account) or falls back to default delegator.
    ///
    /// Routing priority: exact domain matches take precedence over wildcard patterns.
    /// Among wildcards, longer (more specific) patterns match first.
    pub async fn resolve(&self, domain: &str) -> (Arc<dyn AuthDelegator>, Option<String>) {
        let routes = self.routes.read().await;

        // First pass: check exact domain matches
        for route in routes.iter() {
            if !route.domain_pattern.starts_with("*.") && domain_matches(&route.domain_pattern, domain) {
                return (
                    route.delegator.clone(),
                    Some(route.credential_account.clone()),
                );
            }
        }

        // Second pass: check wildcard patterns (longer patterns first for specificity)
        let mut wildcard_match: Option<&AuthRoute> = None;
        for route in routes.iter() {
            if route.domain_pattern.starts_with("*.") && domain_matches(&route.domain_pattern, domain) {
                match wildcard_match {
                    Some(prev) if route.domain_pattern.len() > prev.domain_pattern.len() => {
                        wildcard_match = Some(route);
                    }
                    None => {
                        wildcard_match = Some(route);
                    }
                    _ => {}
                }
            }
        }

        if let Some(route) = wildcard_match {
            return (
                route.delegator.clone(),
                Some(route.credential_account.clone()),
            );
        }

        (self.default_delegator.clone(), None)
    }

    /// Add a routing rule.
    pub async fn add_route(
        &self,
        domain_pattern: String,
        delegator: Arc<dyn AuthDelegator>,
        credential_account: String,
    ) {
        self.routes.write().await.push(AuthRoute {
            domain_pattern,
            delegator,
            credential_account,
        });
    }

    /// Remove all routes matching a domain pattern.
    pub async fn remove_route(&self, domain_pattern: &str) {
        self.routes
            .write()
            .await
            .retain(|r| r.domain_pattern != domain_pattern);
    }

    /// Clear and reload routes.
    pub async fn clear_routes(&self) {
        self.routes.write().await.clear();
    }
}

/// Simple glob-style domain matching.
/// Supports leading `*` wildcard (e.g., "*.amazonaws.com" matches "s3.amazonaws.com").
fn domain_matches(pattern: &str, domain: &str) -> bool {
    if pattern == domain {
        return true;
    }
    if let Some(suffix) = pattern.strip_prefix("*.") {
        // Match the suffix itself or any subdomain
        domain == suffix || domain.ends_with(&format!(".{}", suffix))
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use async_trait::async_trait;

    #[test]
    fn test_exact_match() {
        assert!(domain_matches("api.openai.com", "api.openai.com"));
        assert!(!domain_matches("api.openai.com", "openai.com"));
    }

    #[test]
    fn test_wildcard_match() {
        assert!(domain_matches("*.amazonaws.com", "s3.amazonaws.com"));
        assert!(domain_matches("*.amazonaws.com", "bedrock-runtime.us-east-1.amazonaws.com"));
        assert!(domain_matches("*.amazonaws.com", "amazonaws.com"));
        assert!(!domain_matches("*.amazonaws.com", "example.com"));
    }

    /// Stub delegator for testing routing priority.
    struct StubDelegator { name: String }

    #[async_trait]
    impl AuthDelegator for StubDelegator {
        fn kind(&self) -> &str { &self.name }
        async fn apply_auth(&self, _req: &mut reqwest::Request, _domain: &str, _account: &str) -> Result<()> { Ok(()) }
        fn credential_description(&self) -> &str { "" }
    }

    #[tokio::test]
    async fn test_exact_beats_wildcard() {
        let default = Arc::new(StubDelegator { name: "default".into() });
        let router = AuthRouter::new(default);

        let wildcard_del = Arc::new(StubDelegator { name: "gcp-adc".into() });
        let exact_del = Arc::new(StubDelegator { name: "bearer".into() });

        // Add wildcard first, then exact — exact should still win
        router.add_route("*.googleapis.com".into(), wildcard_del, "gcp-sa".into()).await;
        router.add_route("generativelanguage.googleapis.com".into(), exact_del, "gemini-key".into()).await;

        let (del, acct) = router.resolve("generativelanguage.googleapis.com").await;
        assert_eq!(del.kind(), "bearer");
        assert_eq!(acct.as_deref(), Some("gemini-key"));

        // Other googleapis subdomains still go to wildcard
        let (del, acct) = router.resolve("storage.googleapis.com").await;
        assert_eq!(del.kind(), "gcp-adc");
        assert_eq!(acct.as_deref(), Some("gcp-sa"));
    }

    #[tokio::test]
    async fn test_longer_wildcard_preferred() {
        let default = Arc::new(StubDelegator { name: "default".into() });
        let router = AuthRouter::new(default);

        let broad = Arc::new(StubDelegator { name: "broad".into() });
        let narrow = Arc::new(StubDelegator { name: "narrow".into() });

        router.add_route("*.com".into(), broad, "broad-acct".into()).await;
        router.add_route("*.googleapis.com".into(), narrow, "narrow-acct".into()).await;

        let (del, _) = router.resolve("storage.googleapis.com").await;
        assert_eq!(del.kind(), "narrow");
    }

    #[tokio::test]
    async fn test_fallback_to_default() {
        let default = Arc::new(StubDelegator { name: "default".into() });
        let router = AuthRouter::new(default);

        let (del, acct) = router.resolve("unknown.example.com").await;
        assert_eq!(del.kind(), "default");
        assert!(acct.is_none());
    }
}
