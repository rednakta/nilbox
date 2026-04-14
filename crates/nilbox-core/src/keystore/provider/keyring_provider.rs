//! OS Keyring-based master key management (Linux / Windows)
#![cfg(not(target_os = "macos"))]

use anyhow::{Result, anyhow, Context};
use keyring::Entry;
use rand::RngCore;
use zeroize::Zeroizing;

const SERVICE: &str = "nilbox";
const ACCOUNT: &str = "master-key";

/// Load master key from OS Keyring, or generate and store a new one.
pub fn load_or_create_master_key() -> Result<Zeroizing<[u8; 32]>> {
    let entry = Entry::new(SERVICE, ACCOUNT)
        .map_err(|e| anyhow!("Failed to create keyring entry: {}", e))?;

    match entry.get_secret() {
        Ok(bytes) => {
            if bytes.len() != 32 {
                return Err(anyhow!(
                    "Invalid master key length in keyring: {} (expected 32)",
                    bytes.len()
                ));
            }
            let mut key = [0u8; 32];
            key.copy_from_slice(&bytes);
            Ok(Zeroizing::new(key))
        }
        Err(keyring::Error::NoEntry) => {
            let mut key = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut key);

            entry
                .set_secret(&key)
                .context("Failed to store master key in OS keyring")?;

            tracing::debug!("Generated and stored new master key in OS keyring");
            Ok(Zeroizing::new(key))
        }
        Err(e) => Err(anyhow!("OS keyring error: {}", e)),
    }
}
