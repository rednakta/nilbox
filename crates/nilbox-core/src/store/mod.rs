//! Store module — App/MCP catalog, install/uninstall

pub mod keys;
pub mod envelope;
pub mod verify;
pub mod auth;
pub mod client;
pub mod pinning;
pub mod challenge;
pub mod version_check;

/// Hardcoded store URL — no user-configurable override in release builds.
pub const STORE_BASE_URL: &str = "https://store.nilbox.run";

use std::collections::HashMap;
use serde::{Serialize, Deserialize};
use anyhow::{Result, anyhow};
use tokio::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Category {
    AIAgent,
    McpServer,
    DevTool,
    Utility,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Source {
    BuiltIn,
    GitUrl(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenRequirement {
    pub domain: String,
    pub keychain_account: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreItem {
    pub id: String,
    pub name: String,
    pub category: Category,
    pub description: String,
    pub source: Source,
    pub install_script: String,
    pub required_tokens: Vec<TokenRequirement>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledItem {
    pub item_id: String,
    pub name: String,
    pub version: String,
    pub installed_at: Option<String>,
}

pub struct StoreManager {
    catalog: RwLock<Vec<StoreItem>>,
    installed: RwLock<HashMap<String, InstalledItem>>,
}

impl StoreManager {
    pub fn new() -> Self {
        Self {
            catalog: RwLock::new(Vec::new()),
            installed: RwLock::new(HashMap::new()),
        }
    }

    pub async fn list_catalog(&self) -> Vec<StoreItem> {
        self.catalog.read().await.clone()
    }

    pub async fn install(&self, item_id: &str) -> Result<()> {
        let catalog = self.catalog.read().await;
        let item = catalog.iter().find(|i| i.id == item_id)
            .ok_or_else(|| anyhow!("Store item not found: {}", item_id))?;

        let installed = InstalledItem {
            item_id: item.id.clone(),
            name: item.name.clone(),
            version: "latest".into(),
            installed_at: None,
        };

        self.installed.write().await.insert(item_id.to_string(), installed);
        // Actual VSOCK install command will be implemented with full integration
        Ok(())
    }

    pub async fn uninstall(&self, item_id: &str) -> Result<()> {
        self.installed.write().await.remove(item_id)
            .ok_or_else(|| anyhow!("Item not installed: {}", item_id))?;
        Ok(())
    }

    pub async fn list_installed(&self) -> Vec<InstalledItem> {
        self.installed.read().await.values().cloned().collect()
    }
}

impl Default for StoreManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Optional verification config passed with install_app VSOCK command.
/// When present, VM agent will POST the install result to store callback URL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallVerifyConfig {
    pub verify_token: String,
    pub callback_url: String,
}

/// Output line streamed from `task install` in the VM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppInstallOutput {
    pub uuid: String,
    pub line: String,
    pub is_stderr: bool,
}

/// Completion event after `task install` finishes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppInstallDone {
    pub uuid: String,
    pub success: bool,
    pub exit_code: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
