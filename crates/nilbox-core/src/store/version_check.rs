//! Version check — Tauri updater wrapper + force upgrade logic

use serde::{Deserialize, Serialize};

/// Update check result returned to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub available: bool,
    pub version: String,
    pub notes: String,
    pub date: String,
}

/// Update settings stored in ConfigStore.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateSettings {
    pub auto_update_check: bool,
    pub last_update_check: Option<String>,
}

/// Compare two semver strings: a > b → true.
pub fn is_newer(a: &str, b: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.split('.')
            .map(|p| p.parse::<u64>().unwrap_or(0))
            .collect()
    };
    let va = parse(a);
    let vb = parse(b);
    for i in 0..3 {
        let a = va.get(i).copied().unwrap_or(0);
        let b = vb.get(i).copied().unwrap_or(0);
        if a > b {
            return true;
        }
        if a < b {
            return false;
        }
    }
    false
}

/// Check if current version is below min_version (force upgrade needed).
pub fn needs_force_upgrade(current: &str, min_version: &str) -> bool {
    is_newer(min_version, current)
}

impl UpdateInfo {
    pub fn none() -> Self {
        Self {
            available: false,
            version: String::new(),
            notes: String::new(),
            date: String::new(),
        }
    }
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            auto_update_check: true,
            last_update_check: None,
        }
    }
}
