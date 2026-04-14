//! SSH key management — Ed25519 keypair generation and persistence
//!
//! Private keys are stored in the encrypted KeyStore (SQLCipher DB) rather than
//! as plaintext files on disk. On first run after migration, any existing file-based
//! key is imported into the KeyStore and the plaintext file is deleted.

use anyhow::{Context, Result};
use russh::keys::{Algorithm, PrivateKey, decode_openssh, load_secret_key, ssh_key::LineEnding};
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, warn};

use crate::keystore::KeyStore;

const SSH_DIR: &str = "ssh";
const PRIVATE_KEY_FILE: &str = "id_ed25519";

/// Generate a fresh Ed25519 keypair and store it in the keystore.
async fn generate_and_store(keystore: &dyn KeyStore) -> Result<PrivateKey> {
    debug!("Generating new Ed25519 SSH keypair in encrypted keystore");
    let key = PrivateKey::random(&mut rand::thread_rng(), Algorithm::Ed25519)
        .context("Failed to generate Ed25519 keypair")?;

    let pem = key
        .to_openssh(LineEnding::LF)
        .context("Failed to encode private key as OpenSSH PEM")?;
    keystore.set_ssh_private_key(&pem).await
        .context("Failed to store SSH private key in keystore")?;

    Ok(key)
}

/// Ensure an Ed25519 keypair exists in the encrypted KeyStore.
/// Migrates any existing plaintext file into the DB on first call.
/// If the stored key is corrupted / unreadable, it is replaced with a new one.
/// Returns `(Arc<PrivateKey>, public_key_openssh_string)`.
pub async fn ensure_keypair(
    keystore: &dyn KeyStore,
    app_data_dir: &Path,
) -> Result<(Arc<PrivateKey>, String)> {
    let private_key = if let Some(pem) = keystore.get_ssh_private_key().await? {
        // Key already in encrypted DB
        debug!("Loading SSH keypair from encrypted keystore");
        match decode_openssh(pem.as_bytes(), None) {
            Ok(key) => key,
            Err(e) => {
                // Stored key is corrupted or in an incompatible format — regenerate
                warn!(
                    "Failed to decode SSH private key from keystore ({}). \
                     Replacing with a freshly generated keypair.",
                    e
                );
                generate_and_store(keystore).await?
            }
        }
    } else {
        // Check for legacy plaintext file to migrate
        let legacy_path = app_data_dir.join(SSH_DIR).join(PRIVATE_KEY_FILE);
        let key = if legacy_path.exists() {
            debug!("Migrating SSH keypair from plaintext file to encrypted keystore");
            let key = load_secret_key(&legacy_path, None)
                .context("Failed to load legacy SSH private key file")?;

            // Store in encrypted DB
            let pem = key
                .to_openssh(LineEnding::LF)
                .context("Failed to encode private key as OpenSSH PEM")?;
            keystore.set_ssh_private_key(&pem).await
                .context("Failed to store SSH private key in keystore")?;

            // Delete plaintext file
            std::fs::remove_file(&legacy_path)
                .context("Failed to remove legacy plaintext SSH key file")?;
            debug!("Deleted legacy plaintext SSH key file: {:?}", legacy_path);

            key
        } else {
            generate_and_store(keystore).await?
        };
        key
    };

    let public_key_str = private_key
        .public_key()
        .to_openssh()
        .context("Failed to encode public key as OpenSSH string")?;

    debug!("SSH public key: {}", public_key_str);

    Ok((Arc::new(private_key), public_key_str))
}
