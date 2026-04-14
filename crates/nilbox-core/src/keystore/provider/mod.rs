#[cfg(target_os = "macos")]
pub mod macos_biometric;

pub mod keyring_provider;

/// Load (or create) the 32-byte SQLCipher master key from the OS-native secret
/// store.
///
/// * **macOS** — Keychain item with `kSecAccessControlBiometryAny`; prompts
///   Touch ID / Face ID on every cold start.  Falls back to
///   `kSecAccessControlUserPresence` (passcode) if biometry is not enrolled.
/// * **Linux / Windows** — standard `keyring` crate (Secret Service / Credential
///   Manager).
pub fn load_or_create_master_key() -> anyhow::Result<zeroize::Zeroizing<[u8; 32]>> {
    #[cfg(target_os = "macos")]
    {
        macos_biometric::load_or_create_master_key()
    }
    #[cfg(not(target_os = "macos"))]
    {
        keyring_provider::load_or_create_master_key()
    }
}
