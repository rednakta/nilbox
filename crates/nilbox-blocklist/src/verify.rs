//! Ed25519 signature verification for blocklist.bin.

use anyhow::{bail, Result};
use ed25519_dalek::{Signature, Verifier};

use crate::keys::default_public_key;

/// Verify an Ed25519 signature over `data`.
///
/// `signature` is the raw 64-byte signature appended to the file.
/// Returns `Ok(())` if valid, error otherwise.
///
/// If the signature is all zeros the file is unsigned — callers that pass
/// `verify_signature: false` to `BloomBlocklist::load()` skip this entirely.
pub fn verify_signature(data: &[u8], signature: &[u8; 64]) -> Result<()> {
    if signature.iter().all(|&b| b == 0) {
        bail!("blocklist is unsigned (signature is all zeros)");
    }
    let key = default_public_key()?;
    let sig = Signature::from_bytes(signature);
    key.verify(data, &sig)
        .map_err(|e| anyhow::anyhow!("blocklist signature invalid: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsigned_file_rejected() {
        let data = b"some data";
        let sig = [0u8; 64];
        assert!(verify_signature(data, &sig).is_err());
    }
}
