//! Pinned Ed25519 public keys for blocklist signature verification.
//! Follows the same pattern as nilbox-core/src/store/keys.rs.

use anyhow::{anyhow, Result};
use ed25519_dalek::VerifyingKey;

/// Production blocklist signing key — Ed25519 public key.
/// Replace with the real key before shipping Tier 2.
const NILBOX_BLOCKLIST_2025: [u8; 32] = [
    0x3b, 0x6a, 0x27, 0xbc, 0xce, 0xb6, 0xa4, 0x2d,
    0x62, 0xa3, 0xa8, 0xd0, 0x2a, 0x6f, 0x0d, 0x73,
    0x65, 0x32, 0x15, 0x77, 0x1d, 0xe2, 0x43, 0xa6,
    0x3a, 0xc0, 0x48, 0xa1, 0x8b, 0x59, 0xda, 0x29,
];

/// Dev blocklist key — derived from all-zeros seed, for tests and dev builds.
#[cfg(any(test, feature = "dev-store"))]
pub(crate) const DEV_BLOCKLIST_KEY: [u8; 32] = [
    0x3b, 0x6a, 0x27, 0xbc, 0xce, 0xb6, 0xa4, 0x2d,
    0x62, 0xa3, 0xa8, 0xd0, 0x2a, 0x6f, 0x0d, 0x73,
    0x65, 0x32, 0x15, 0x77, 0x1d, 0xe2, 0x43, 0xa6,
    0x3a, 0xc0, 0x48, 0xa1, 0x8b, 0x59, 0xda, 0x29,
];

pub fn get_blocklist_public_key(key_id: &str) -> Result<VerifyingKey> {
    let bytes = match key_id {
        "nilbox-blocklist-2025" => &NILBOX_BLOCKLIST_2025,
        #[cfg(any(test, feature = "dev-store"))]
        "nilbox-blocklist-dev" => &DEV_BLOCKLIST_KEY,
        _ => return Err(anyhow!("Unknown blocklist public key id: {}", key_id)),
    };
    VerifyingKey::from_bytes(bytes)
        .map_err(|e| anyhow!("Invalid blocklist public key for '{}': {}", key_id, e))
}

/// Returns the key to use for verifying CDN-distributed blocklists.
pub fn default_public_key() -> Result<VerifyingKey> {
    #[cfg(any(test, feature = "dev-store"))]
    return get_blocklist_public_key("nilbox-blocklist-dev");
    #[cfg(not(any(test, feature = "dev-store")))]
    get_blocklist_public_key("nilbox-blocklist-2025")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_key_loads() {
        let key = get_blocklist_public_key("nilbox-blocklist-dev").unwrap();
        assert_eq!(key.as_bytes(), &DEV_BLOCKLIST_KEY);
    }

    #[test]
    fn prod_key_loads() {
        let key = get_blocklist_public_key("nilbox-blocklist-2025").unwrap();
        assert_eq!(key.as_bytes(), &NILBOX_BLOCKLIST_2025);
    }

    #[test]
    fn unknown_key_errors() {
        assert!(get_blocklist_public_key("unknown").is_err());
    }
}
