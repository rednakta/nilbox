//! MCP Bridge Manager — register/unregister MCP servers, generate Claude Desktop config

use std::collections::HashMap;
use serde::{Serialize, Deserialize};
use anyhow::{Result, anyhow};
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub vm_port: u16,
    pub host_port: u16,
    pub transport: McpTransport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum McpTransport {
    Stdio,
    Sse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerInfo {
    pub id: String,
    pub name: String,
    pub vm_port: u16,
    pub host_port: u16,
    pub transport: McpTransport,
}

pub struct McpBridgeManager {
    servers: RwLock<HashMap<String, McpServerInfo>>,
}

impl McpBridgeManager {
    pub fn new() -> Self {
        Self {
            servers: RwLock::new(HashMap::new()),
        }
    }

    pub async fn register(&self, config: McpServerConfig) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let info = McpServerInfo {
            id: id.clone(),
            name: config.name,
            vm_port: config.vm_port,
            host_port: config.host_port,
            transport: config.transport,
        };
        self.servers.write().await.insert(id.clone(), info);
        Ok(id)
    }

    pub async fn unregister(&self, id: &str) -> Result<()> {
        self.servers.write().await.remove(id)
            .ok_or_else(|| anyhow!("MCP server not found: {}", id))?;
        Ok(())
    }

    pub async fn list(&self) -> Vec<McpServerInfo> {
        self.servers.read().await.values().cloned().collect()
    }

    /// Generate Claude Desktop MCP configuration JSON.
    ///
    /// Uses the resolved path to `nilbox-mcp-bridge` bundled alongside the app binary.
    pub async fn generate_claude_config(&self) -> serde_json::Value {
        let bridge_path = Self::resolve_bridge_path();
        let servers = self.servers.read().await;
        let mut mcp_servers = serde_json::Map::new();

        for (_, info) in servers.iter() {
            mcp_servers.insert(info.name.clone(), serde_json::json!({
                "command": bridge_path,
                "args": ["--port", info.host_port.to_string()]
            }));
        }

        serde_json::json!({ "mcpServers": mcp_servers })
    }

    /// Resolve the absolute path to the bundled `nilbox-mcp-bridge` binary.
    ///
    /// Tauri bundles externalBin binaries next to the main executable.
    fn resolve_bridge_path() -> String {
        let binary_name = if cfg!(target_os = "windows") {
            "nilbox-mcp-bridge.exe"
        } else {
            "nilbox-mcp-bridge"
        };

        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let path = dir.join(binary_name);
                if path.exists() {
                    return path.to_string_lossy().into_owned();
                }
            }
        }

        // Fallback: assume it's in PATH
        binary_name.to_string()
    }
}

impl Default for McpBridgeManager {
    fn default() -> Self {
        Self::new()
    }
}
