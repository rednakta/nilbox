//! Ed25519 signature verification for signed manifest envelopes.

use anyhow::{anyhow, Context, Result};
use ed25519_dalek::{Signature, Verifier};
use sha2::{Sha256, Digest};
use serde_json::Value;
use tracing::{warn, error, debug};
#[cfg(test)]
use chrono;

use super::envelope::{ManifestEnvelope, SignedEnvelopeV2, SignedPayload};
use super::keys::get_store_public_key;
use crate::vm_install::canonical_json_value;

/// Verify a manifest envelope and return a reference to the verified manifest `Value`.
///
/// - **V3**: decrypt (done in parse), then verify Ed25519 + SHA256 on inner V2.
/// - **V2**: verify Ed25519 signature + SHA256 integrity.
/// - **Legacy**: SHA256 integrity check only (with warning log).
///
/// Timestamp freshness is intentionally not checked: pre-computed envelopes are
/// generated at write time and served as-is; AES-GCM decryption success already
/// proves the envelope came from a trusted source.
pub fn verify_envelope(envelope: &ManifestEnvelope) -> Result<&Value> {
    match envelope {
        ManifestEnvelope::V3(v3) => {
            // Decryption already happened in parse_v3; verify the inner V2 fields.
            verify_v2_inner(&v3.inner)
        }
        ManifestEnvelope::V2(v2) => verify_v2_inner(v2),
        ManifestEnvelope::Legacy(payload) => {
            warn!("Legacy unsigned manifest — SHA256-only verification");
            verify_sha256_integrity(payload)?;
            Ok(&payload.manifest)
        }
    }
}

/// Shared V2 verification logic used by both V2 and V3 paths.
fn verify_v2_inner(v2: &SignedEnvelopeV2) -> Result<&Value> {
    // 1. Look up public key
    debug!("[verify_envelope] store_public_key_id from server: '{}'", v2.store_public_key_id);
    let verifying_key = get_store_public_key(&v2.store_public_key_id)
        .map_err(|e| {
            error!("[verify_envelope] key lookup failed for '{}': {}", v2.store_public_key_id, e);
            anyhow!("Store public key lookup failed (key_id='{}', detail: {})", v2.store_public_key_id, e)
        })?;
    debug!("[verify_envelope] key lookup OK for '{}'", v2.store_public_key_id);

    // 2. Verify Ed25519 signature
    let signature = Signature::from_slice(&v2.store_signature)
        .map_err(|e| anyhow!("Invalid signature format: {}", e))?;
    debug!("[verify_envelope] verifying Ed25519 signature ({} canonical bytes)", v2.canonical_payload_bytes.len());
    verifying_key
        .verify(&v2.canonical_payload_bytes, &signature)
        .map_err(|e| {
            error!("[verify_envelope] Ed25519 signature verification failed for key '{}': {}", v2.store_public_key_id, e);
            error!("[verify_envelope] canonical_payload (first 200 bytes): {:?}",
                std::str::from_utf8(&v2.canonical_payload_bytes[..v2.canonical_payload_bytes.len().min(200)]).unwrap_or("(non-utf8)"));
            anyhow!(
                "Ed25519 signature verification failed for key '{}'",
                v2.store_public_key_id
            )
        })?;
    debug!("[verify_envelope] Ed25519 signature OK");

    // 3. SHA256 integrity check on manifest
    debug!("[verify_envelope] checking SHA256 integrity (expected: {})", v2.signed_payload.manifest_sha256);
    verify_sha256_integrity(&v2.signed_payload)
        .map_err(|e| { error!("[verify_envelope] SHA256 integrity check failed: {}", e); e })?;
    debug!("[verify_envelope] SHA256 integrity OK — verification complete");

    Ok(&v2.signed_payload.manifest)
}

/// Verify that SHA256(canonical JSON of manifest, excluding taskfile_content) matches expected hash.
fn verify_sha256_integrity(payload: &SignedPayload) -> Result<()> {
    let mut m = payload.manifest.clone();
    if let Value::Object(ref mut map) = m {
        map.remove("taskfile_content");
    }
    let canonical = canonical_json_value(m);
    let json_str = serde_json::to_string(&canonical)
        .context("Failed to serialize manifest for SHA256 verification")?;
    let actual = format!("{:x}", Sha256::digest(json_str.as_bytes()));

    if actual != payload.manifest_sha256 {
        return Err(anyhow!(
            "Manifest SHA256 mismatch: expected {}, got {}",
            payload.manifest_sha256,
            actual
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::envelope::parse_envelope;
    use serde_json::json;

    /// Helper: produce a real signed v2 envelope JSON using the dev key (all-zeros seed).
    fn make_signed_v2(manifest: Value) -> Value {
        use ed25519_dalek::SigningKey;

        let seed = [0u8; 32];
        let signing_key = SigningKey::from_bytes(&seed);

        let mut m = manifest.clone();
        if let Value::Object(ref mut map) = m {
            map.remove("taskfile_content");
        }
        let canonical = canonical_json_value(m);
        let json_str = serde_json::to_string(&canonical).unwrap();
        let sha256 = format!("{:x}", Sha256::digest(json_str.as_bytes()));

        let payload = json!({
            "manifest_sha256": sha256,
            "manifest": manifest,
        });

        let canonical_payload = canonical_json_value(payload.clone());
        let payload_bytes = serde_json::to_string(&canonical_payload).unwrap();

        use ed25519_dalek::Signer;
        let sig = signing_key.sign(payload_bytes.as_bytes());

        let sig_b64 = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            sig.to_bytes(),
        );

        let now = chrono::Utc::now().to_rfc3339();

        json!({
            "version": 2,
            "store_signature": sig_b64,
            "store_public_key_id": "nilbox-store-dev",
            "timestamp": now,
            "signed_payload": payload,
        })
    }

    /// Helper: wrap a v2 envelope JSON in a v3 encrypted envelope using the dev enc key.
    fn make_v3_from_v2(v2_json: Value) -> Value {
        use aes_gcm::{aead::{Aead, KeyInit}, Aes256Gcm, Nonce};
        use super::super::keys::DEV_ENC_KEY;

        let plaintext = serde_json::to_vec(&v2_json).unwrap();
        let cipher = Aes256Gcm::new_from_slice(&DEV_ENC_KEY).unwrap();
        // Use a fixed nonce for test reproducibility
        let nonce_bytes = [0xabu8; 12];
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = cipher.encrypt(nonce, plaintext.as_ref()).unwrap();

        let now = chrono::Utc::now().to_rfc3339();
        json!({
            "version": 3,
            "key_id": "nilbox-enc-dev",
            "nonce": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &nonce_bytes),
            "ciphertext": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &ciphertext),
            "timestamp": now,
        })
    }

    // ── V2 tests ──────────────────────────────────────────────────────────────

    #[test]
    fn test_v2_verify_success() {
        let manifest = json!({
            "name": "Test App",
            "source": {"image_url": "https://example.com/test.zip"}
        });
        let raw = make_signed_v2(manifest);
        let envelope = parse_envelope(&raw).unwrap();
        let result = verify_envelope(&envelope);
        assert!(result.is_ok(), "Verification should succeed: {:?}", result.err());
        assert_eq!(result.unwrap()["name"], "Test App");
    }

    #[test]
    fn test_v2_tampered_manifest_fails() {
        let manifest = json!({
            "name": "Test App",
            "source": {"image_url": "https://example.com/test.zip"}
        });
        let mut raw = make_signed_v2(manifest);
        // Tamper with the manifest name in signed_payload
        raw["signed_payload"]["manifest"]["name"] = json!("TAMPERED");
        let envelope = parse_envelope(&raw).unwrap();
        let result = verify_envelope(&envelope);
        assert!(result.is_err(), "Tampered manifest should fail verification");
    }

    #[test]
    fn test_v2_wrong_signature_fails() {
        let manifest = json!({
            "name": "Test App",
            "source": {"image_url": "https://example.com/test.zip"}
        });
        let mut raw = make_signed_v2(manifest);
        // Replace signature with garbage
        let bad_sig = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            &[0xFFu8; 64],
        );
        raw["store_signature"] = json!(bad_sig);
        let envelope = parse_envelope(&raw).unwrap();
        let result = verify_envelope(&envelope);
        assert!(result.is_err(), "Wrong signature should fail verification");
    }

    // ── V3 tests ──────────────────────────────────────────────────────────────

    #[test]
    fn test_v3_verify_success() {
        let manifest = json!({
            "name": "Encrypted App",
            "source": {"image_url": "https://example.com/enc.zip"}
        });
        let v2_json = make_signed_v2(manifest);
        let v3_json = make_v3_from_v2(v2_json);

        let envelope = parse_envelope(&v3_json).unwrap();
        assert!(matches!(envelope, ManifestEnvelope::V3(_)));
        let result = verify_envelope(&envelope);
        assert!(result.is_ok(), "V3 verification should succeed: {:?}", result.err());
        assert_eq!(result.unwrap()["name"], "Encrypted App");
    }

    #[test]
    fn test_v3_tampered_inner_manifest_fails() {
        let manifest = json!({
            "name": "Encrypted App",
            "source": {"image_url": "https://example.com/enc.zip"}
        });
        let mut v2_json = make_signed_v2(manifest);
        // Tamper with manifest inside the v2 envelope before encrypting
        v2_json["signed_payload"]["manifest"]["name"] = json!("TAMPERED");
        let v3_json = make_v3_from_v2(v2_json);

        let envelope = parse_envelope(&v3_json).unwrap();
        let result = verify_envelope(&envelope);
        // SHA256 mismatch should be caught during verify
        assert!(result.is_err(), "Tampered inner manifest should fail verification");
    }

    // ── Legacy tests ──────────────────────────────────────────────────────────

    #[test]
    fn test_legacy_verify_success() {
        let manifest = json!({"name": "Legacy", "source": {"image_url": "https://example.com/x.zip"}});
        let canonical = canonical_json_value(manifest.clone());
        let json_str = serde_json::to_string(&canonical).unwrap();
        let sha256 = format!("{:x}", Sha256::digest(json_str.as_bytes()));

        let raw = json!({
            "manifest_sha256": sha256,
            "manifest": manifest,
        });
        let envelope = parse_envelope(&raw).unwrap();
        let result = verify_envelope(&envelope);
        assert!(result.is_ok(), "Legacy verification should succeed: {:?}", result.err());
    }

    #[test]
    fn test_legacy_tampered_fails() {
        let raw = json!({
            "manifest_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
            "manifest": {"name": "Legacy"},
        });
        let envelope = parse_envelope(&raw).unwrap();
        let result = verify_envelope(&envelope);
        assert!(result.is_err(), "Tampered legacy should fail");
    }

}
