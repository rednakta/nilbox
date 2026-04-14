//! Challenge-response verification — no-op implementation.
//!
//! Server identity is no longer separately verified via challenge-response.
//! AES-256-GCM decryption success of V3 envelopes proves the server holds the correct key.

use anyhow::Result;

/// Server identity verifier — no-op. V3 envelope decryption success is sufficient proof.
pub struct ChallengeVerifier;

impl ChallengeVerifier {
    pub fn new() -> Self {
        Self
    }

    /// No-op: V3 envelope decryption already proves server identity.
    pub async fn verify(&self, _http: &reqwest::Client, _store_url: &str) -> Result<()> {
        Ok(())
    }

    /// No-op reset.
    #[allow(dead_code)]
    pub async fn reset(&self) {}
}

