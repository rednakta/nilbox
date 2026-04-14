//! Pinned store public keys for Ed25519 manifest signature verification,
//! and symmetric encryption keys for v3 envelope decryption.

use anyhow::{anyhow, Result};
use ed25519_dalek::VerifyingKey;

/// Production store signing key — Ed25519 public key matching STORE_SIGNING_KEY_SEED in nilbox-store.
const NILBOX_STORE_PUB_KEYID: [u8; 32] = [
    0x05, 0x46, 0xd5, 0x92, 0x95, 0x63, 0x09, 0xc3,
    0x1f, 0x6a, 0x38, 0x7f, 0x6d, 0xac, 0x81, 0x04,
    0xe1, 0xcb, 0x79, 0x9a, 0x40, 0x1a, 0xfa, 0x7e,
    0x57, 0x70, 0x5c, 0xf6, 0xcb, 0xce, 0xb8, 0x89
];

/// Dev store key — public key derived from all-zeros seed.
/// Hex: 3b6a27bcceb6a42d62a3a8d02a6f0d73653215771de243a63ac048a18b59da29
#[cfg(any(test, feature = "dev-store"))]
const DEV_STORE_KEY: [u8; 32] = [
    0x3b, 0x6a, 0x27, 0xbc, 0xce, 0xb6, 0xa4, 0x2d,
    0x62, 0xa3, 0xa8, 0xd0, 0x2a, 0x6f, 0x0d, 0x73,
    0x65, 0x32, 0x15, 0x77, 0x1d, 0xe2, 0x43, 0xa6,
    0x3a, 0xc0, 0x48, 0xa1, 0x8b, 0x59, 0xda, 0x29,
];

/// Look up a store public key by its identifier.
pub fn get_store_public_key(key_id: &str) -> Result<VerifyingKey> {
    let bytes = match key_id {
        "nilbox-store-2026" => &NILBOX_STORE_PUB_KEYID,
        #[cfg(any(test, feature = "dev-store"))]
        "nilbox-store-dev" => &DEV_STORE_KEY,
        _ => return Err(anyhow!("Unknown store public key id: {}", key_id)),
    };
    VerifyingKey::from_bytes(bytes)
        .map_err(|e| anyhow!("Invalid store public key for '{}': {}", key_id, e))
}

/// Production manifest encryption key — matches STORE_ENC_KEY in nilbox-store.
const NILBOX_ENC_KEY: [u8; 32] = [
    0xc7, 0xc4, 0xac, 0xba, 0x95, 0x2b, 0x33, 0x21,
    0x20, 0x30, 0xa9, 0x44, 0xcb, 0x6d, 0xf0, 0xff,
    0xd4, 0x54, 0x81, 0x83, 0x18, 0x31, 0x20, 0xe1,
    0x83, 0x29, 0x81, 0x8d, 0x5c, 0x0b, 0xc6, 0x2f
];

/// Dev encryption key — ascending byte values 0x01..=0x20, easy to reproduce in tests.
#[cfg(any(test, feature = "dev-store"))]
pub(crate) const DEV_ENC_KEY: [u8; 32] = [
    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08,
    0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10,
    0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18,
    0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f, 0x20,
];

/// Look up a symmetric encryption key by its identifier.
pub fn get_enc_key(key_id: &str) -> Result<[u8; 32]> {
    match key_id {
        "nilbox-enc-2026" => Ok(NILBOX_ENC_KEY),
        #[cfg(any(test, feature = "dev-store"))]
        "nilbox-enc-dev" => Ok(DEV_ENC_KEY),
        _ => Err(anyhow!("Unknown enc key id: {}", key_id)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dev_key_loads() {
        let key = get_store_public_key("nilbox-store-dev").unwrap();
        assert_eq!(key.as_bytes(), &DEV_STORE_KEY);
    }

    #[test]
    fn test_production_key_loads() {
        // Placeholder zeros — will fail verify but should parse
        let key = get_store_public_key("nilbox-store-2026").unwrap();
        assert_eq!(key.as_bytes(), &NILBOX_STORE_PUB_KEYID);
    }

    #[test]
    fn test_unknown_key_errors() {
        assert!(get_store_public_key("unknown-key").is_err());
    }

    #[test]
    fn test_dev_enc_key_loads() {
        let key = get_enc_key("nilbox-enc-dev").unwrap();
        assert_eq!(key, DEV_ENC_KEY);
    }

    #[test]
    fn test_unknown_enc_key_errors() {
        assert!(get_enc_key("unknown-enc-key").is_err());
    }
}
