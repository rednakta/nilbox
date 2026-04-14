//! Lazy-initializing KeyStore — defers OS keyring access until first use.
//!
//! The OS keyring prompt (macOS Keychain password dialog) is triggered only
//! when the keystore is first accessed, not at app startup.

use super::provider;
use super::sqlite_store::SqliteKeyStore;
use super::KeyStore;

use crate::config_store::LlmProvider;
use async_trait::async_trait;
use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::OnceCell;

pub struct LazyKeyStore {
    db_path: PathBuf,
    inner: OnceCell<Arc<dyn KeyStore>>,
}

impl LazyKeyStore {
    pub fn new(db_path: PathBuf) -> Self {
        Self {
            db_path,
            inner: OnceCell::new(),
        }
    }

    async fn store(&self) -> Result<&Arc<dyn KeyStore>> {
        let db_path = self.db_path.clone();
        self.inner
            .get_or_try_init(|| async {
                // load_or_create_master_key blocks for Touch ID / password prompt;
                // run on a dedicated thread so the Tokio runtime stays responsive.
                let master_key = tokio::task::spawn_blocking(
                    provider::load_or_create_master_key
                )
                .await
                .map_err(|e| anyhow::anyhow!("keystore init thread panicked: {}", e))??;
                let store = SqliteKeyStore::new(db_path, master_key)?;
                Ok::<Arc<dyn KeyStore>, anyhow::Error>(Arc::new(store))
            })
            .await
    }
}

#[async_trait]
impl KeyStore for LazyKeyStore {
    async fn get(&self, account: &str) -> Result<String> {
        self.store().await?.get(account).await
    }

    async fn set(&self, account: &str, value: &str) -> Result<()> {
        self.store().await?.set(account, value).await
    }

    async fn delete(&self, account: &str) -> Result<()> {
        self.store().await?.delete(account).await
    }

    async fn list(&self) -> Result<Vec<String>> {
        self.store().await?.list().await
    }

    async fn has(&self, account: &str) -> Result<bool> {
        self.store().await?.has(account).await
    }

    async fn rename(&self, old_account: &str, new_account: &str) -> Result<()> {
        self.store().await?.rename(old_account, new_account).await
    }

    async fn get_ssh_private_key(&self) -> Result<Option<String>> {
        self.store().await?.get_ssh_private_key().await
    }

    async fn set_ssh_private_key(&self, openssh_pem: &str) -> Result<()> {
        self.store().await?.set_ssh_private_key(openssh_pem).await
    }

    async fn list_llm_providers(&self) -> Result<Vec<LlmProvider>> {
        self.store().await?.list_llm_providers().await
    }

    async fn replace_llm_providers(&self, providers: &[LlmProvider], version: &str) -> Result<()> {
        self.store().await?.replace_llm_providers(providers, version).await
    }

    async fn get_llm_providers_version(&self) -> Result<Option<String>> {
        self.store().await?.get_llm_providers_version().await
    }
}
