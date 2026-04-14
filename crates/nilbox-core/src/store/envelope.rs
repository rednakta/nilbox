//! Signed envelope parser — detect v3 (encrypted+signed), v2 (Ed25519-signed), or legacy (SHA256-only).

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use serde_json::Value;

use super::keys::get_enc_key;

/// Inner signed payload containing the manifest and its SHA256.
#[derive(Debug, Clone)]
pub struct SignedPayload {
    pub manifest_sha256: String,
    pub manifest: Value,
}

/// Signed envelope v2 with Ed25519 store signature.
#[derive(Debug, Clone)]
pub struct SignedEnvelopeV2 {
    pub store_signature: Vec<u8>,
    pub store_public_key_id: String,
    pub timestamp: String,
    pub signed_payload: SignedPayload,
    /// Canonical JSON bytes of the signed_payload (for signature verification).
    pub canonical_payload_bytes: Vec<u8>,
}

/// Signed envelope v3 — AES-256-GCM encrypted v2 envelope with Ed25519 inner signature.
#[derive(Debug, Clone)]
pub struct SignedEnvelopeV3 {
    pub key_id: String,
    pub nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
    /// Decrypted inner — identical to a V2 envelope after decryption.
    pub inner: SignedEnvelopeV2,
}

/// Parsed manifest envelope — v3 (encrypted+signed), v2 (signed), or legacy (SHA256-only).
#[derive(Debug)]
pub enum ManifestEnvelope {
    V3(SignedEnvelopeV3),
    V2(SignedEnvelopeV2),
    Legacy(SignedPayload),
}

/// Parse a raw JSON value into a `ManifestEnvelope`.
///
/// Detection by `version` field: 3 → v3, 2 → v2, else → legacy.
pub fn parse_envelope(raw: &Value) -> Result<ManifestEnvelope> {
    match raw.get("version").and_then(|v| v.as_u64()) {
        Some(3) => parse_v3(raw),
        Some(2) => parse_v2(raw),
        _ => parse_legacy(raw),
    }
}

fn parse_legacy(raw: &Value) -> Result<ManifestEnvelope> {
    let manifest_sha256 = raw["manifest_sha256"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing manifest_sha256 field"))?
        .to_string();

    let manifest = raw["manifest"].clone();
    if manifest.is_null() {
        return Err(anyhow!("Missing manifest field"));
    }

    Ok(ManifestEnvelope::Legacy(SignedPayload {
        manifest_sha256,
        manifest,
    }))
}

fn parse_v2(raw: &Value) -> Result<ManifestEnvelope> {
    let sig_b64 = raw["store_signature"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing store_signature field"))?;
    let store_signature = base64::engine::general_purpose::STANDARD
        .decode(sig_b64)
        .context("Invalid base64 in store_signature")?;

    let store_public_key_id = raw["store_public_key_id"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing store_public_key_id field"))?
        .to_string();

    let timestamp = raw["timestamp"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing timestamp field"))?
        .to_string();

    let payload_value = raw.get("signed_payload")
        .ok_or_else(|| anyhow!("Missing signed_payload field"))?;

    let manifest_sha256 = payload_value["manifest_sha256"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing manifest_sha256 in signed_payload"))?
        .to_string();

    let manifest = payload_value["manifest"].clone();
    if manifest.is_null() {
        return Err(anyhow!("Missing manifest in signed_payload"));
    }

    // Compute canonical JSON of the signed_payload for signature verification
    let canonical = crate::vm_install::canonical_json_value(payload_value.clone());
    let canonical_bytes = serde_json::to_string(&canonical)
        .context("Failed to serialize canonical payload")?
        .into_bytes();

    Ok(ManifestEnvelope::V2(SignedEnvelopeV2 {
        store_signature,
        store_public_key_id,
        timestamp,
        signed_payload: SignedPayload {
            manifest_sha256,
            manifest,
        },
        canonical_payload_bytes: canonical_bytes,
    }))
}

fn parse_v3(raw: &Value) -> Result<ManifestEnvelope> {
    let key_id = raw["key_id"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing key_id field in v3 envelope"))?
        .to_string();

    let nonce_b64 = raw["nonce"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing nonce field in v3 envelope"))?;
    let nonce_bytes = base64::engine::general_purpose::STANDARD
        .decode(nonce_b64)
        .context("Invalid base64 in v3 nonce")?;
    if nonce_bytes.len() != 12 {
        return Err(anyhow!("V3 nonce must be 12 bytes, got {}", nonce_bytes.len()));
    }

    let ciphertext_b64 = raw["ciphertext"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing ciphertext field in v3 envelope"))?;
    let ciphertext = base64::engine::general_purpose::STANDARD
        .decode(ciphertext_b64)
        .context("Invalid base64 in v3 ciphertext")?;

    // Decrypt: get key → AES-256-GCM → plaintext v2 envelope JSON
    let key_bytes = get_enc_key(&key_id)?;
    let cipher = Aes256Gcm::new_from_slice(&key_bytes)
        .map_err(|e| anyhow!("Failed to create cipher for key '{}': {}", key_id, e))?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let plaintext = cipher
        .decrypt(nonce, ciphertext.as_ref())
        .map_err(|_| anyhow!("Manifest decryption failed — ciphertext tampered or wrong key"))?;

    // Plaintext is the v2 envelope JSON bytes
    let v2_json: Value = serde_json::from_slice(&plaintext)
        .context("Decrypted v3 ciphertext is not valid JSON")?;

    // Parse as v2 (must be version 2)
    if v2_json.get("version").and_then(|v| v.as_u64()) != Some(2) {
        return Err(anyhow!("V3 inner ciphertext must contain a version-2 envelope"));
    }
    let inner = match parse_v2(&v2_json)? {
        ManifestEnvelope::V2(v2) => v2,
        _ => return Err(anyhow!("V3 inner parse unexpectedly returned non-V2")),
    };

    Ok(ManifestEnvelope::V3(SignedEnvelopeV3 {
        key_id,
        nonce: nonce_bytes,
        ciphertext,
        inner,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_legacy() {
        let raw = json!({
            "manifest_sha256": "abc123",
            "manifest": {"name": "test"}
        });
        let env = parse_envelope(&raw).unwrap();
        match env {
            ManifestEnvelope::Legacy(p) => {
                assert_eq!(p.manifest_sha256, "abc123");
                assert_eq!(p.manifest["name"], "test");
            }
            _ => panic!("Expected Legacy"),
        }
    }

    #[test]
    fn test_parse_v2() {
        let sig_b64 = base64::engine::general_purpose::STANDARD.encode(&[0u8; 64]);
        let raw = json!({
            "version": 2,
            "store_signature": sig_b64,
            "store_public_key_id": "nilbox-store-dev",
            "timestamp": "2025-01-01T00:00:00+00:00",
            "signed_payload": {
                "manifest_sha256": "def456",
                "manifest": {"name": "test-v2"}
            }
        });
        let env = parse_envelope(&raw).unwrap();
        match env {
            ManifestEnvelope::V2(v2) => {
                assert_eq!(v2.store_public_key_id, "nilbox-store-dev");
                assert_eq!(v2.signed_payload.manifest_sha256, "def456");
                assert_eq!(v2.signed_payload.manifest["name"], "test-v2");
                assert_eq!(v2.store_signature.len(), 64);
                assert!(!v2.canonical_payload_bytes.is_empty());
            }
            _ => panic!("Expected V2"),
        }
    }

    #[test]
    fn test_parse_v3() {
        use aes_gcm::aead::Aead;
        use super::super::keys::DEV_ENC_KEY;

        // Build a real v2 envelope JSON (signature can be garbage for parse test)
        let sig_b64 = base64::engine::general_purpose::STANDARD.encode(&[0u8; 64]);
        let v2_json = json!({
            "version": 2,
            "store_signature": sig_b64,
            "store_public_key_id": "nilbox-store-dev",
            "timestamp": "2025-01-01T00:00:00+00:00",
            "signed_payload": {
                "manifest_sha256": "aabbcc",
                "manifest": {"name": "test-v3"}
            }
        });
        let plaintext = serde_json::to_vec(&v2_json).unwrap();

        // Encrypt with dev key
        let cipher = Aes256Gcm::new_from_slice(&DEV_ENC_KEY).unwrap();
        let nonce_bytes = [0u8; 12];
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = cipher.encrypt(nonce, plaintext.as_ref()).unwrap();

        let outer = json!({
            "version": 3,
            "key_id": "nilbox-enc-dev",
            "nonce": base64::engine::general_purpose::STANDARD.encode(&nonce_bytes),
            "ciphertext": base64::engine::general_purpose::STANDARD.encode(&ciphertext),
            "timestamp": "2025-01-01T00:00:00+00:00"
        });

        let env = parse_envelope(&outer).unwrap();
        match env {
            ManifestEnvelope::V3(v3) => {
                assert_eq!(v3.key_id, "nilbox-enc-dev");
                assert_eq!(v3.nonce.len(), 12);
                assert_eq!(v3.inner.store_public_key_id, "nilbox-store-dev");
                assert_eq!(v3.inner.signed_payload.manifest["name"], "test-v3");
            }
            _ => panic!("Expected V3"),
        }
    }

    #[test]
    fn test_parse_v3_tampered_ciphertext_fails() {
        use aes_gcm::aead::Aead;
        use super::super::keys::DEV_ENC_KEY;

        let v2_json = json!({
            "version": 2,
            "store_signature": base64::engine::general_purpose::STANDARD.encode(&[0u8; 64]),
            "store_public_key_id": "nilbox-store-dev",
            "timestamp": "2025-01-01T00:00:00+00:00",
            "signed_payload": {"manifest_sha256": "x", "manifest": {}}
        });
        let plaintext = serde_json::to_vec(&v2_json).unwrap();
        let cipher = Aes256Gcm::new_from_slice(&DEV_ENC_KEY).unwrap();
        let nonce_bytes = [0u8; 12];
        let nonce = Nonce::from_slice(&nonce_bytes);
        let mut ciphertext = cipher.encrypt(nonce, plaintext.as_ref()).unwrap();

        // Flip one bit in the ciphertext
        ciphertext[0] ^= 0x01;

        let outer = json!({
            "version": 3,
            "key_id": "nilbox-enc-dev",
            "nonce": base64::engine::general_purpose::STANDARD.encode(&nonce_bytes),
            "ciphertext": base64::engine::general_purpose::STANDARD.encode(&ciphertext),
            "timestamp": "2025-01-01T00:00:00+00:00"
        });

        let result = parse_envelope(&outer);
        assert!(result.is_err(), "Tampered ciphertext should fail decryption");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("decryption failed") || err.contains("tampered"), "Error: {}", err);
    }

    #[test]
    fn test_parse_v3_unknown_key_id_fails() {
        let outer = json!({
            "version": 3,
            "key_id": "nilbox-enc-unknown",
            "nonce": base64::engine::general_purpose::STANDARD.encode(&[0u8; 12]),
            "ciphertext": base64::engine::general_purpose::STANDARD.encode(&[0u8; 32]),
            "timestamp": "2025-01-01T00:00:00+00:00"
        });

        let result = parse_envelope(&outer);
        assert!(result.is_err(), "Unknown key_id should fail before decryption");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown enc key id"), "Error: {}", err);
    }
}
