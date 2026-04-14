//! TelegramDelegator — URL path token injection for Telegram Bot API.
//!
//! Telegram embeds the bot token in the URL path (`/bot<TOKEN>/method`),
//! not in headers. This delegator replaces a placeholder token name
//! (e.g., `TELEGRAM_BOT_TOKEN`) in the path with the real secret from
//! the keystore.

use super::AuthDelegator;
use crate::keystore::KeyStore;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::sync::Arc;

pub struct TelegramDelegator {
    keystore: Arc<dyn KeyStore>,
}

impl TelegramDelegator {
    pub fn new(keystore: Arc<dyn KeyStore>) -> Self {
        Self { keystore }
    }
}

#[async_trait]
impl AuthDelegator for TelegramDelegator {
    fn kind(&self) -> &str {
        "telegram"
    }

    async fn apply_auth(
        &self,
        request: &mut reqwest::Request,
        _domain: &str,
        credential_account: &str,
    ) -> Result<()> {
        let real_token = self
            .keystore
            .get(credential_account)
            .await
            .map_err(|e| anyhow!("Failed to load Telegram bot token '{}': {}", credential_account, e))?;

        let path = request.url().path().to_string();
        let placeholder = format!("/bot{}/", credential_account);

        if !path.contains(&placeholder) {
            return Ok(());
        }

        let new_path = path.replace(&placeholder, &format!("/bot{}/", real_token));
        request.url_mut().set_path(&new_path);

        // Remove any auth headers — Telegram uses path-based auth only
        request.headers_mut().remove("authorization");

        Ok(())
    }

    fn credential_description(&self) -> &str {
        "Telegram Bot Token (URL path injection)"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keystore::KeyStore;
    use crate::config_store::LlmProvider;
    use anyhow::Result;
    use async_trait::async_trait;
    use std::sync::Arc;

    struct MockKeyStore {
        value: String,
    }

    #[async_trait]
    impl KeyStore for MockKeyStore {
        async fn get(&self, _key: &str) -> Result<String> {
            Ok(self.value.clone())
        }
        async fn set(&self, _key: &str, _value: &str) -> Result<()> {
            Ok(())
        }
        async fn delete(&self, _key: &str) -> Result<()> {
            Ok(())
        }
        async fn has(&self, _account: &str) -> Result<bool> {
            Ok(true)
        }
        async fn list(&self) -> Result<Vec<String>> {
            Ok(vec![])
        }
        async fn rename(&self, _old: &str, _new: &str) -> Result<()> {
            Ok(())
        }
        async fn get_ssh_private_key(&self) -> Result<Option<String>> { Ok(None) }
        async fn set_ssh_private_key(&self, _: &str) -> Result<()> { Ok(()) }
        async fn list_llm_providers(&self) -> Result<Vec<LlmProvider>> { Ok(vec![]) }
        async fn replace_llm_providers(&self, _: &[LlmProvider], _: &str) -> Result<()> { Ok(()) }
        async fn get_llm_providers_version(&self) -> Result<Option<String>> { Ok(None) }
    }

    #[tokio::test]
    async fn test_replaces_placeholder_in_path() {
        let ks: Arc<dyn KeyStore> = Arc::new(MockKeyStore {
            value: "123456:ABC-DEF".to_string(),
        });
        let delegator = TelegramDelegator::new(ks);

        let client = reqwest::Client::new();
        let mut request = client
            .get("https://api.telegram.org/botTELEGRAM_BOT_TOKEN/sendMessage")
            .build()
            .unwrap();

        delegator
            .apply_auth(&mut request, "api.telegram.org", "TELEGRAM_BOT_TOKEN")
            .await
            .unwrap();

        assert_eq!(
            request.url().path(),
            "/bot123456:ABC-DEF/sendMessage"
        );
    }

    #[tokio::test]
    async fn test_no_placeholder_noop() {
        let ks: Arc<dyn KeyStore> = Arc::new(MockKeyStore {
            value: "123456:ABC-DEF".to_string(),
        });
        let delegator = TelegramDelegator::new(ks);

        let client = reqwest::Client::new();
        let mut request = client
            .get("https://api.telegram.org/bot123456:REAL/sendMessage")
            .build()
            .unwrap();

        delegator
            .apply_auth(&mut request, "api.telegram.org", "TELEGRAM_BOT_TOKEN")
            .await
            .unwrap();

        // Path unchanged — no placeholder found
        assert_eq!(
            request.url().path(),
            "/bot123456:REAL/sendMessage"
        );
    }

    #[tokio::test]
    async fn test_removes_authorization_header() {
        let ks: Arc<dyn KeyStore> = Arc::new(MockKeyStore {
            value: "123456:ABC-DEF".to_string(),
        });
        let delegator = TelegramDelegator::new(ks);

        let client = reqwest::Client::new();
        let mut request = client
            .get("https://api.telegram.org/botTELEGRAM_BOT_TOKEN/getMe")
            .header("authorization", "Bearer dummy")
            .build()
            .unwrap();

        delegator
            .apply_auth(&mut request, "api.telegram.org", "TELEGRAM_BOT_TOKEN")
            .await
            .unwrap();

        assert!(request.headers().get("authorization").is_none());
    }
}
