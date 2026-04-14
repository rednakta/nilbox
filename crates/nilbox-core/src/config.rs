//! Configuration DTOs — lightweight data transfer objects
//!
//! Persistence is now handled by `config_store::ConfigStore` (SQLite).
//! These structs remain as shared DTOs used across service/command layers.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortMappingConfig {
    #[serde(default)]
    pub vm_id: String,
    pub host_port: u16,
    pub vm_port: u16,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMappingConfig {
    pub vm_id: String,
    pub host_path: String,
    pub vm_mount: String,
    pub read_only: bool,
    pub label: String,
}
