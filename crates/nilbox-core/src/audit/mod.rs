//! Audit log — token exchange/block event recording

use serde::{Serialize, Deserialize};
use std::time::SystemTime;
use tokio::sync::RwLock;
use anyhow::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuditAction {
    TokenExchange { domain: String, account: String },
    TokenBlocked { domain: String, reason: String },
    VmStarted { vm_id: String },
    VmStopped { vm_id: String },
    PortMappingAdded { host_port: u16, vm_port: u16 },
    PortMappingRemoved { host_port: u16 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: u64,
    pub action: AuditAction,
    pub timestamp: SystemTime,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuditFilter {
    pub action_type: Option<String>,
    pub limit: Option<usize>,
}

pub struct AuditLog {
    entries: RwLock<Vec<AuditEntry>>,
    next_id: RwLock<u64>,
}

impl AuditLog {
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(Vec::new()),
            next_id: RwLock::new(1),
        }
    }

    /// Record an audit event.
    pub async fn record(&self, action: AuditAction) {
        let mut id_guard = self.next_id.write().await;
        let entry = AuditEntry {
            id: *id_guard,
            action,
            timestamp: SystemTime::now(),
        };
        *id_guard += 1;
        drop(id_guard);
        self.entries.write().await.push(entry);
    }

    /// Query audit entries with optional filter.
    pub async fn query(&self, filter: &AuditFilter) -> Vec<AuditEntry> {
        let entries = self.entries.read().await;
        let limit = filter.limit.unwrap_or(entries.len());
        entries.iter().rev().take(limit).cloned().collect()
    }

    /// Export entries as JSON bytes.
    pub async fn export_json(&self) -> Result<Vec<u8>> {
        let entries = self.entries.read().await;
        let json = serde_json::to_vec_pretty(&*entries)?;
        Ok(json)
    }
}

impl Default for AuditLog {
    fn default() -> Self {
        Self::new()
    }
}
