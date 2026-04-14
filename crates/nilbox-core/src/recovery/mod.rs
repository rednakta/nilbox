//! Recovery module — VM crash detection and auto-restart

use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use tokio::sync::RwLock;
use anyhow::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecoveryState {
    Disabled,
    Enabled,
    Recovering,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryConfig {
    pub max_retries: u32,
    pub retry_delay_secs: u64,
}

impl Default for RecoveryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            retry_delay_secs: 5,
        }
    }
}

struct VmRecoveryState {
    state: RecoveryState,
    retry_count: u32,
    config: RecoveryConfig,
}

pub struct RecoveryManager {
    vm_states: RwLock<HashMap<String, VmRecoveryState>>,
}

impl RecoveryManager {
    pub fn new() -> Self {
        Self {
            vm_states: RwLock::new(HashMap::new()),
        }
    }

    pub async fn enable(&self, vm_id: &str) -> Result<()> {
        let mut states = self.vm_states.write().await;
        states.insert(vm_id.to_string(), VmRecoveryState {
            state: RecoveryState::Enabled,
            retry_count: 0,
            config: RecoveryConfig::default(),
        });
        Ok(())
    }

    pub async fn disable(&self, vm_id: &str) {
        let mut states = self.vm_states.write().await;
        states.remove(vm_id);
    }

    pub async fn status(&self, vm_id: &str) -> RecoveryState {
        let states = self.vm_states.read().await;
        states.get(vm_id)
            .map(|s| s.state)
            .unwrap_or(RecoveryState::Disabled)
    }

    /// Called when a VM enters Error state. Returns true if recovery should be attempted.
    pub async fn on_vm_error(&self, vm_id: &str) -> bool {
        let mut states = self.vm_states.write().await;
        if let Some(state) = states.get_mut(vm_id) {
            if state.state == RecoveryState::Enabled && state.retry_count < state.config.max_retries {
                state.state = RecoveryState::Recovering;
                state.retry_count += 1;
                return true;
            }
        }
        false
    }

    /// Reset retry count after successful recovery.
    pub async fn on_vm_recovered(&self, vm_id: &str) {
        let mut states = self.vm_states.write().await;
        if let Some(state) = states.get_mut(vm_id) {
            state.state = RecoveryState::Enabled;
            state.retry_count = 0;
        }
    }
}

impl Default for RecoveryManager {
    fn default() -> Self {
        Self::new()
    }
}
