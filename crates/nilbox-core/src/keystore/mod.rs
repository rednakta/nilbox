//! KeyStore trait and factory

pub mod lazy_store;
pub mod provider;
pub mod sqlite_store;

use async_trait::async_trait;
use anyhow::Result;
use std::sync::Arc;

use crate::config_store::LlmProvider;

#[async_trait]
pub trait KeyStore: Send + Sync {
    async fn get(&self, account: &str) -> Result<String>;
    async fn set(&self, account: &str, value: &str) -> Result<()>;
    async fn delete(&self, account: &str) -> Result<()>;
    async fn list(&self) -> Result<Vec<String>>;
    async fn has(&self, account: &str) -> Result<bool>;
    async fn rename(&self, old_account: &str, new_account: &str) -> Result<()>;

    // ── SSH Keys ───────────────────────────────────────────
    async fn get_ssh_private_key(&self) -> Result<Option<String>>;
    async fn set_ssh_private_key(&self, openssh_pem: &str) -> Result<()>;

    // ── LLM Providers ────────────────────────────────────────
    async fn list_llm_providers(&self) -> Result<Vec<LlmProvider>>;
    async fn replace_llm_providers(&self, providers: &[LlmProvider], version: &str) -> Result<()>;
    async fn get_llm_providers_version(&self) -> Result<Option<String>>;
}

/// Create a lazy keystore: OS keyring access is deferred until the first
/// actual key operation (VM start, domain gate, API key prompt, etc.).
/// This prevents the macOS Keychain password dialog from appearing at app startup.
pub fn create_keystore(app_data_dir: &std::path::Path) -> Arc<dyn KeyStore> {
    let db_path = app_data_dir.join("nilbox").join("keys.db");
    Arc::new(lazy_store::LazyKeyStore::new(db_path))
}
