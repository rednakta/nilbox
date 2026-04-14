//! OAuthTokenVault — intercepts OAuth token responses, stores real tokens in
//! Host KeyStore, and returns dummy tokens to the VM.
//!
//! Dummy token format:
//!   Access:  `nilbox_oat:{provider_id}:{session_uuid}`
//!   Refresh: `nilbox_ort:{provider_id}:{session_uuid}`
//!
//! KeyStore account: `OAUTH_TOKEN:{provider_id}:{session_uuid}`

use anyhow::{Result, anyhow};
use serde::{Serialize, Deserialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, warn};

use crate::keystore::KeyStore;
use super::oauth_script_engine::TokenResponseFields;

// ── Dummy token prefixes ────────────────────────────────────────────

const ACCESS_PREFIX: &str = "nilbox_oat:";
const REFRESH_PREFIX: &str = "nilbox_ort:";
const KEYSTORE_PREFIX: &str = "OAUTH_TOKEN:";

// ── Stored session ──────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
struct OAuthSession {
    provider_id: String,
    vm_id: String,
    access_token: String,
    refresh_token: Option<String>,
    token_type: Option<String>,
    expires_at: Option<u64>,
    scope: Option<String>,
    created_at: u64,
    #[serde(default)]
    allowed_domain: Option<String>,
    #[serde(default)]
    extra_domains: Vec<String>,
}

/// Public session metadata — excludes real token values for security.
#[derive(Serialize, Deserialize, Clone)]
pub struct OAuthSessionInfo {
    pub session_key: String,
    pub provider_id: String,
    pub vm_id: String,
    pub token_type: Option<String>,
    pub expires_at: Option<u64>,
    pub scope: Option<String>,
    pub created_at: u64,
    pub has_refresh_token: bool,
    pub allowed_domain: Option<String>,
    #[serde(default)]
    pub extra_domains: Vec<String>,
}

// ── Domain check result ────────────────────────────────────────────

/// Result of checking a dummy access token against a request domain.
pub enum OAuthDomainCheck {
    /// No session found for this dummy token.
    NotFound,
    /// First use — no domain bound yet.
    FirstUse,
    /// Domain matches the bound domain — proceed.
    Match,
    /// Domain mismatch — security warning needed.
    Mismatch { bound_domain: String },
}

// ── OAuthTokenVault ─────────────────────────────────────────────────

pub struct OAuthTokenVault {
    keystore: Arc<dyn KeyStore>,
}

/// Parse a dummy token (access or refresh) and return (provider_id, session_uuid).
pub fn parse_dummy_prefix(token: &str) -> Option<(&str, &str)> {
    parse_dummy_token(token, ACCESS_PREFIX)
        .or_else(|| parse_dummy_token(token, REFRESH_PREFIX))
}

impl OAuthTokenVault {
    pub fn new(keystore: Arc<dyn KeyStore>) -> Self {
        Self { keystore }
    }

    // ── Public: prefix detection (static) ───────────────────────────

    pub fn is_dummy_access_token(v: &str) -> bool {
        parse_dummy_access_token(v).is_some()
    }

    pub fn is_dummy_refresh_token(v: &str) -> bool {
        v.starts_with(REFRESH_PREFIX)
    }

    // ── Public: intercept token response ────────────────────────────

    /// Intercept an OAuth token response: store real tokens in KeyStore,
    /// replace them with dummy tokens, and return the modified body.
    ///
    /// If `existing_session_uuid` is Some, this is a refresh — update the
    /// existing session in-place and return the same dummy tokens.
    /// If None, create a new session.
    pub async fn intercept_token_response(
        &self,
        provider_id: &str,
        vm_id: &str,
        response_body: &[u8],
        fields: &TokenResponseFields,
        existing_session_uuid: Option<&str>,
        extra_domains: Vec<String>,
    ) -> Result<Vec<u8>> {
        // 1. Parse response
        let mut doc = match fields.response_format.as_str() {
            "form" => parse_form_to_json(response_body)?,
            _ => serde_json::from_slice(response_body)
                .map_err(|e| anyhow!("failed to parse token response JSON: {}", e))?,
        };

        // 2. Extract real tokens
        let access_token = extract_by_path(&doc, &fields.access_token_field)?;

        let refresh_token = if !fields.refresh_token_field.is_empty() {
            extract_by_path(&doc, &fields.refresh_token_field).ok()
        } else {
            None
        };
        debug!(
            "OAuth intercept: extracted tokens provider={} has_refresh={}",
            provider_id, refresh_token.is_some()
        );

        // 3. Extract optional metadata
        let expires_in: Option<u64> = extract_by_path(&doc, &fields.expires_in_field)
            .ok()
            .and_then(|v| v.parse().ok());
        let token_type = extract_by_path(&doc, &fields.token_type_field).ok();
        let scope = extract_by_path(&doc, "scope").ok();

        // 4. Session UUID: reuse existing (refresh) or generate new
        let session_uuid = match existing_session_uuid {
            Some(uuid) => uuid.to_string(),
            None => generate_hex_uuid(),
        };

        // 5. Build dummy tokens (JWT-aware: preserves identity claims for client parsing)
        let dummy_access = make_jwt_dummy_access_token(&access_token, provider_id, &session_uuid);
        let dummy_refresh = refresh_token.as_ref().map(|_|
            format!("{}{}:{}", REFRESH_PREFIX, provider_id, session_uuid)
        );

        // 6. Compute timestamps
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let expires_at = expires_in.map(|ei| now + ei);

        // 7. Store/update session in KeyStore
        // For refresh: preserve original created_at, refresh_token, allowed_domain, extra_domains
        let (created_at, refresh_token, preserved_allowed_domain, preserved_extra_domains) = if let Some(_) = existing_session_uuid {
            debug!(
                "OAuth intercept: refreshing existing session provider={} session={}",
                provider_id, session_uuid
            );
            let account = format!("{}{}:{}", KEYSTORE_PREFIX, provider_id, session_uuid);
            let old_session = self.keystore.get(&account).await.ok()
                .and_then(|json| serde_json::from_str::<OAuthSession>(&json).ok());
            let ca = old_session.as_ref().map(|s| s.created_at).unwrap_or(now);
            let ad = old_session.as_ref().and_then(|s| s.allowed_domain.clone());
            let ed = old_session.as_ref().map(|s| s.extra_domains.clone()).unwrap_or_else(|| extra_domains.clone());
            // Preserve old refresh_token when new response doesn't include one
            let rt = if refresh_token.is_none() {
                let preserved = old_session.and_then(|s| s.refresh_token);
                if preserved.is_some() {
                    debug!("Preserving existing refresh_token for session {}", session_uuid);
                }
                preserved
            } else {
                refresh_token
            };
            (ca, rt, ad, ed)
        } else {
            (now, refresh_token, None, extra_domains)
        };

        let session = OAuthSession {
            provider_id: provider_id.to_string(),
            vm_id: vm_id.to_string(),
            access_token,
            refresh_token,
            token_type,
            expires_at,
            scope,
            created_at,
            allowed_domain: preserved_allowed_domain,
            extra_domains: preserved_extra_domains,
        };

        let account = format!("{}{}:{}", KEYSTORE_PREFIX, provider_id, session_uuid);
        let session_json = serde_json::to_string(&session)?;
        self.keystore.set(&account, &session_json).await?;

        let action = if existing_session_uuid.is_some() { "Updated" } else { "Stored" };
        debug!(
            "{} OAuth session: provider={}, vm={}, session={}",
            action, provider_id, vm_id, session_uuid
        );

        // 8. Replace real tokens with dummy tokens in response
        replace_by_path(&mut doc, &fields.access_token_field, &dummy_access)?;
        if let Some(ref dummy_rt) = dummy_refresh {
            replace_by_path(&mut doc, &fields.refresh_token_field, dummy_rt)?;
        }

        // 9. Serialize back to original format
        match fields.response_format.as_str() {
            "form" => json_to_form(&doc),
            _ => serde_json::to_vec(&doc)
                .map_err(|e| anyhow!("failed to serialize modified response: {}", e)),
        }
    }

    // ── Public: resolve dummy → real ────────────────────────────────

    /// Resolve a dummy access token (plain or JWT format) to the real access token.
    pub async fn resolve_access_token(&self, dummy: &str) -> Result<Option<String>> {
        let (provider_id, session_uuid) = match parse_dummy_access_token(dummy) {
            Some(parts) => parts,
            None => return Ok(None),
        };
        debug!(
            "OAuth resolve_access_token: provider={} session={}",
            provider_id, session_uuid
        );

        let account = format!("{}{}:{}", KEYSTORE_PREFIX, provider_id, session_uuid);
        match self.keystore.get(&account).await {
            Ok(json_str) => {
                let session: OAuthSession = serde_json::from_str(&json_str)?;
                debug!(
                    "OAuth resolve_access_token: resolved provider={} session={}",
                    provider_id, session_uuid
                );
                Ok(Some(session.access_token))
            }
            Err(_) => {
                debug!(
                    "OAuth resolve_access_token: session not found provider={} session={}",
                    provider_id, session_uuid
                );
                Ok(None)
            }
        }
    }

    /// Resolve a dummy refresh token to the real refresh token.
    ///
    /// Returns:
    /// - `Ok(Some(token))` — session found with refresh_token
    /// - `Ok(None)` — dummy token format invalid
    /// - `Err("session not found")` — session does not exist in keystore
    /// - `Err("refresh_token is empty")` — session exists but has no refresh_token
    pub async fn resolve_refresh_token(&self, dummy: &str) -> Result<Option<String>> {
        let (provider_id, session_uuid) = match parse_dummy_token(dummy, REFRESH_PREFIX) {
            Some(parts) => parts,
            None => return Ok(None),
        };
        debug!(
            "OAuth resolve_refresh_token: provider={} session={}",
            provider_id, session_uuid
        );

        let account = format!("{}{}{}{}", KEYSTORE_PREFIX, provider_id, ":", session_uuid);
        match self.keystore.get(&account).await {
            Ok(json_str) => {
                let session: OAuthSession = serde_json::from_str(&json_str)?;
                match session.refresh_token {
                    Some(rt) => {
                        debug!(
                            "OAuth resolve_refresh_token: resolved provider={} session={}",
                            provider_id, session_uuid
                        );
                        Ok(Some(rt))
                    }
                    None => {
                        warn!(
                            "OAuth resolve_refresh_token: session has no refresh_token provider={} session={}",
                            provider_id, session_uuid
                        );
                        Err(anyhow!("session exists but refresh_token is empty (provider={}, session={})", provider_id, session_uuid))
                    }
                }
            }
            Err(_) => {
                warn!(
                    "OAuth resolve_refresh_token: session not found provider={} session={}",
                    provider_id, session_uuid
                );
                Err(anyhow!("session not found (provider={}, session={})", provider_id, session_uuid))
            }
        }
    }

    // ── Public: domain-aware resolve ──────────────────────────────

    /// Check a dummy access token against a request domain and resolve it.
    /// Returns domain-match status along with the real access token.
    pub async fn check_and_resolve(&self, dummy: &str, domain: &str) -> Result<OAuthDomainCheck> {
        let (provider_id, session_uuid) = match parse_dummy_access_token(dummy) {
            Some(parts) => parts,
            None => return Ok(OAuthDomainCheck::NotFound),
        };

        let account = format!("{}{}:{}", KEYSTORE_PREFIX, provider_id, session_uuid);
        let json_str = match self.keystore.get(&account).await {
            Ok(s) => s,
            Err(_) => return Ok(OAuthDomainCheck::NotFound),
        };

        let session: OAuthSession = serde_json::from_str(&json_str)?;
        match &session.allowed_domain {
            None => Ok(OAuthDomainCheck::FirstUse),
            Some(bound) if bound == domain => Ok(OAuthDomainCheck::Match),
            _ if session.extra_domains.iter().any(|d| d == domain) => Ok(OAuthDomainCheck::Match),
            Some(bound) => Ok(OAuthDomainCheck::Mismatch {
                bound_domain: bound.clone(),
            }),
        }
    }

    /// Bind a domain to an OAuth session (first-use binding).
    /// No-op if the session already has an `allowed_domain` (guards against TOCTOU races).
    pub async fn bind_domain(&self, dummy: &str, domain: &str) -> Result<()> {
        let (provider_id, session_uuid) = parse_dummy_access_token(dummy)
            .ok_or_else(|| anyhow!("invalid dummy token for bind_domain"))?;

        let account = format!("{}{}:{}", KEYSTORE_PREFIX, provider_id, session_uuid);
        let json_str = self.keystore.get(&account).await
            .map_err(|e| anyhow!("session not found for bind_domain: {}", e))?;

        let mut session: OAuthSession = serde_json::from_str(&json_str)?;
        if session.allowed_domain.is_some() {
            // Already bound by a concurrent request — do not overwrite
            debug!("OAuth session {}:{} already bound, skipping", provider_id, session_uuid);
            return Ok(());
        }
        session.allowed_domain = Some(domain.to_string());
        let updated = serde_json::to_string(&session)?;
        self.keystore.set(&account, &updated).await?;
        debug!("Bound OAuth session {}:{} to domain {}", provider_id, session_uuid, domain);
        Ok(())
    }

    // ── Public: cached token response (skip upstream refresh) ──────

    /// Safety margin in seconds before `expires_at` to trigger real refresh.
    const EXPIRY_BUFFER_SECS: u64 = 60;

    /// If the cached access token is still valid, build a synthetic token
    /// response with dummy tokens and return it.  Returns `None` when the
    /// caller should forward the refresh request to upstream.
    ///
    /// `dummy_refresh` is the dummy refresh token from the request body
    /// (e.g. `nilbox_ort:google:UUID`).
    pub async fn try_cached_token_response(
        &self,
        dummy_refresh: &str,
        fields: &TokenResponseFields,
    ) -> Option<Vec<u8>> {
        let (provider_id, session_uuid) = match parse_dummy_token(dummy_refresh, REFRESH_PREFIX) {
            Some(parts) => parts,
            None => {
                debug!("OAuth cache: dummy refresh token parse failed");
                return None;
            }
        };

        let account = format!("{}{}:{}", KEYSTORE_PREFIX, provider_id, session_uuid);
        let json_str = match self.keystore.get(&account).await {
            Ok(s) => s,
            Err(_) => {
                debug!(
                    "OAuth cache: session not found provider={} session={}",
                    provider_id, session_uuid
                );
                return None;
            }
        };
        let session: OAuthSession = serde_json::from_str(&json_str).ok()?;

        // Must have expires_at to judge validity
        let expires_at = match session.expires_at {
            Some(ea) => ea,
            None => {
                debug!(
                    "OAuth cache: no expires_at for provider={} session={}, skipping cache",
                    provider_id, session_uuid
                );
                return None;
            }
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        if now + Self::EXPIRY_BUFFER_SECS >= expires_at {
            // Token expired or about to expire — need real refresh
            debug!(
                "OAuth cache: token expired/expiring provider={} session={} expires_at={} now={}",
                provider_id, session_uuid, expires_at, now
            );
            return None;
        }

        let remaining = expires_at - now;

        // Build dummy tokens
        let dummy_access = format!("{}{}:{}", ACCESS_PREFIX, provider_id, session_uuid);
        let dummy_rt = format!("{}{}:{}", REFRESH_PREFIX, provider_id, session_uuid);

        // Build synthetic response matching the expected field paths
        let mut doc = serde_json::json!({});
        set_by_path(&mut doc, &fields.access_token_field, &dummy_access);
        if !fields.refresh_token_field.is_empty() && session.refresh_token.is_some() {
            set_by_path(&mut doc, &fields.refresh_token_field, &dummy_rt);
        }
        set_by_path_num(&mut doc, &fields.expires_in_field, remaining);
        if let Some(ref tt) = session.token_type {
            set_by_path(&mut doc, &fields.token_type_field, tt);
        }
        if let Some(ref sc) = session.scope {
            set_by_path(&mut doc, "scope", sc);
        }

        debug!(
            "Returning cached OAuth token ({}s remaining) for provider={}, session={}",
            remaining, provider_id, session_uuid
        );

        match fields.response_format.as_str() {
            "form" => json_to_form(&doc).ok(),
            _ => serde_json::to_vec(&doc).ok(),
        }
    }

    // ── Public: cleanup ─────────────────────────────────────────────

    /// Remove all OAuth sessions for a given VM.
    pub async fn cleanup_vm_sessions(&self, vm_id: &str) -> Result<()> {
        let accounts = self.keystore.list().await?;
        let oauth_accounts: Vec<&String> = accounts.iter()
            .filter(|a| a.starts_with(KEYSTORE_PREFIX))
            .collect();

        let mut removed = 0u32;
        for account in oauth_accounts {
            if let Ok(json_str) = self.keystore.get(account).await {
                if let Ok(session) = serde_json::from_str::<OAuthSession>(&json_str) {
                    if session.vm_id == vm_id {
                        if let Err(e) = self.keystore.delete(account).await {
                            warn!("Failed to delete OAuth session {}: {}", account, e);
                        } else {
                            removed += 1;
                        }
                    }
                }
            }
        }

        if removed > 0 {
            debug!("Cleaned up {} OAuth session(s) for VM {}", removed, vm_id);
        }
        Ok(())
    }

    // ── Public: list sessions (metadata only) ─────────────────────

    /// List OAuth sessions, optionally filtered by vm_id.
    /// Returns metadata only — real tokens are NOT included.
    pub async fn list_sessions(&self, vm_id_filter: Option<&str>) -> Result<Vec<OAuthSessionInfo>> {
        let accounts = self.keystore.list().await?;
        let oauth_accounts: Vec<&String> = accounts.iter().filter(|a| a.starts_with(KEYSTORE_PREFIX)).collect();
        let mut sessions = Vec::new();

        for account in &oauth_accounts {
            if let Ok(json_str) = self.keystore.get(account).await {
                if let Ok(session) = serde_json::from_str::<OAuthSession>(&json_str) {
                    if let Some(filter) = vm_id_filter {
                        if session.vm_id != filter {
                            continue;
                        }
                    }
                    sessions.push(OAuthSessionInfo {
                        session_key: account.to_string(),
                        provider_id: session.provider_id,
                        vm_id: session.vm_id,
                        token_type: session.token_type,
                        expires_at: session.expires_at,
                        scope: session.scope,
                        created_at: session.created_at,
                        has_refresh_token: session.refresh_token.is_some(),
                        allowed_domain: session.allowed_domain,
                        extra_domains: session.extra_domains,
                    });
                }
            }
        }

        sessions.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(sessions)
    }

    // ── Public: delete single session ─────────────────────────────

    /// Delete a single OAuth session by its keystore account key.
    pub async fn delete_session(&self, session_key: &str) -> Result<()> {
        if !session_key.starts_with(KEYSTORE_PREFIX) {
            return Err(anyhow!("Invalid session key"));
        }
        self.keystore.delete(session_key).await?;
        debug!("Deleted OAuth session: {}", session_key);
        Ok(())
    }
}

// ── Internal helpers ────────────────────────────────────────────────

/// Generate a 24-character hex UUID from 12 random bytes.
fn generate_hex_uuid() -> String {
    use rand::RngCore;
    use std::fmt::Write;
    let mut buf = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut buf);
    let mut hex = String::with_capacity(24);
    for b in &buf {
        let _ = write!(hex, "{:02x}", b);
    }
    hex
}

/// Parse a dummy token string, stripping the prefix and returning (provider_id, session_uuid).
fn parse_dummy_token<'a>(token: &'a str, prefix: &str) -> Option<(&'a str, &'a str)> {
    let rest = token.strip_prefix(prefix)?;
    let colon_pos = rest.find(':')?;
    let provider_id = &rest[..colon_pos];
    let session_uuid = &rest[colon_pos + 1..];
    if provider_id.is_empty() || session_uuid.is_empty() {
        return None;
    }
    Some((provider_id, session_uuid))
}

/// Extract the nilbox session marker from a synthetic JWT dummy token's payload.
fn extract_nilbox_jwt_session(token: &str) -> Option<String> {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

    let mut parts = token.splitn(3, '.');
    let _header = parts.next()?;
    let payload_b64 = parts.next()?;
    parts.next()?; // must have 3 segments

    let payload_bytes = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    let doc: serde_json::Value = serde_json::from_slice(&payload_bytes).ok()?;
    doc.get("nilbox")?.as_str().map(|s| s.to_string())
}

/// Parse a dummy access token in either plain (`nilbox_oat:…`) or synthetic JWT format.
/// Returns (provider_id, session_uuid) as owned strings.
fn parse_dummy_access_token(token: &str) -> Option<(String, String)> {
    // Fast path: plain prefix
    if let Some((pid, uuid)) = parse_dummy_token(token, ACCESS_PREFIX) {
        return Some((pid.to_string(), uuid.to_string()));
    }
    // Slow path: synthetic JWT with embedded nilbox claim
    let inner = extract_nilbox_jwt_session(token)?;
    let rest = inner.strip_prefix(ACCESS_PREFIX)?;
    let colon = rest.find(':')?;
    let pid = &rest[..colon];
    let uuid = &rest[colon + 1..];
    if pid.is_empty() || uuid.is_empty() {
        return None;
    }
    Some((pid.to_string(), uuid.to_string()))
}

/// Build a dummy access token.  When the real token is a JWT, produces a
/// synthetic JWT that preserves the original identity claims (e.g. accountId)
/// so client-side JWT parsing keeps working.  For non-JWT tokens the plain
/// `nilbox_oat:provider:session` format is returned.
fn make_jwt_dummy_access_token(
    real_access_token: &str,
    provider_id: &str,
    session_uuid: &str,
) -> String {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

    let plain_dummy = format!("{}{}:{}", ACCESS_PREFIX, provider_id, session_uuid);

    // Only wrap when the real token looks like a JWT (header.payload.signature)
    let parts: Vec<&str> = real_access_token.splitn(3, '.').collect();
    if parts.len() != 3 {
        return plain_dummy;
    }

    let payload_bytes = match URL_SAFE_NO_PAD.decode(parts[1]) {
        Ok(b) => b,
        Err(_) => return plain_dummy,
    };
    let mut payload: serde_json::Value = match serde_json::from_slice(&payload_bytes) {
        Ok(v) => v,
        Err(_) => return plain_dummy,
    };

    // Embed nilbox session marker; remove token-specific claim
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("nilbox".to_string(), serde_json::Value::String(plain_dummy));
        obj.remove("jti");
    }

    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"JWT"}"#);
    let new_payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap_or_default());
    format!("{}.{}.", header, new_payload)
}

/// Extract a string value from a JSON document using dot-notation path.
/// e.g. "data.access_token" → doc["data"]["access_token"]
fn extract_by_path(doc: &serde_json::Value, path: &str) -> Result<String> {
    let mut current = doc;
    for part in path.split('.') {
        current = current.get(part)
            .ok_or_else(|| anyhow!("path '{}' not found in response", path))?;
    }
    current.as_str()
        .map(|s| s.to_string())
        .or_else(|| {
            // Handle numeric values (e.g., expires_in might be a number)
            if current.is_number() {
                Some(current.to_string())
            } else {
                None
            }
        })
        .ok_or_else(|| anyhow!("value at '{}' is not a string or number", path))
}

/// Set a string value in a JSON document at a dot-notation path, creating
/// intermediate objects as needed.  Used by `try_cached_token_response` to
/// build synthetic responses.
fn set_by_path(doc: &mut serde_json::Value, path: &str, val: &str) {
    let parts: Vec<&str> = path.split('.').collect();
    let mut current = doc;
    for part in &parts[..parts.len() - 1] {
        if current.get(part).is_none() {
            current[*part] = serde_json::json!({});
        }
        current = current.get_mut(part).unwrap();
    }
    if let Some(last) = parts.last() {
        current[*last] = serde_json::Value::String(val.to_string());
    }
}

/// Set a numeric value in a JSON document at a dot-notation path, creating
/// intermediate objects as needed.
fn set_by_path_num(doc: &mut serde_json::Value, path: &str, val: u64) {
    let parts: Vec<&str> = path.split('.').collect();
    let mut current = doc;
    for part in &parts[..parts.len() - 1] {
        if current.get(part).is_none() {
            current[*part] = serde_json::json!({});
        }
        current = current.get_mut(part).unwrap();
    }
    if let Some(last) = parts.last() {
        current[*last] = serde_json::json!(val);
    }
}

/// Replace a value in a JSON document at a dot-notation path.
fn replace_by_path(doc: &mut serde_json::Value, path: &str, new_val: &str) -> Result<()> {
    let parts: Vec<&str> = path.split('.').collect();
    let mut current = doc;
    for part in &parts[..parts.len() - 1] {
        current = current.get_mut(part)
            .ok_or_else(|| anyhow!("path '{}' not found for replacement", path))?;
    }
    if let Some(last) = parts.last() {
        current[*last] = serde_json::Value::String(new_val.to_string());
    }
    Ok(())
}

/// Parse form-urlencoded body into a JSON object.
fn parse_form_to_json(body: &[u8]) -> Result<serde_json::Value> {
    let pairs: Vec<(String, String)> = url::form_urlencoded::parse(body)
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let mut map = serde_json::Map::new();
    for (k, v) in pairs {
        map.insert(k, serde_json::Value::String(v));
    }
    Ok(serde_json::Value::Object(map))
}

/// Serialize a JSON object back to form-urlencoded bytes.
fn json_to_form(doc: &serde_json::Value) -> Result<Vec<u8>> {
    let obj = doc.as_object()
        .ok_or_else(|| anyhow!("expected JSON object for form serialization"))?;
    let mut ser = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in obj {
        let val = match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        ser.append_pair(k, &val);
    }
    Ok(ser.finish().into_bytes())
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;
    use std::collections::HashMap;
    use crate::config_store::LlmProvider;

    /// In-memory mock KeyStore for tests.
    struct MockKeyStore {
        data: Mutex<HashMap<String, String>>,
    }

    impl MockKeyStore {
        fn new() -> Self {
            Self { data: Mutex::new(HashMap::new()) }
        }
    }

    #[async_trait]
    impl KeyStore for MockKeyStore {
        async fn get(&self, account: &str) -> Result<String> {
            self.data.lock().unwrap().get(account).cloned()
                .ok_or_else(|| anyhow!("not found: {}", account))
        }
        async fn set(&self, account: &str, value: &str) -> Result<()> {
            self.data.lock().unwrap().insert(account.to_string(), value.to_string());
            Ok(())
        }
        async fn delete(&self, account: &str) -> Result<()> {
            self.data.lock().unwrap().remove(account);
            Ok(())
        }
        async fn list(&self) -> Result<Vec<String>> {
            Ok(self.data.lock().unwrap().keys().cloned().collect())
        }
        async fn has(&self, account: &str) -> Result<bool> {
            Ok(self.data.lock().unwrap().contains_key(account))
        }
        async fn rename(&self, old_account: &str, new_account: &str) -> Result<()> {
            let mut data = self.data.lock().unwrap();
            if let Some(v) = data.remove(old_account) {
                data.insert(new_account.to_string(), v);
            }
            Ok(())
        }
        async fn get_ssh_private_key(&self) -> Result<Option<String>> { Ok(None) }
        async fn set_ssh_private_key(&self, _: &str) -> Result<()> { Ok(()) }
        async fn list_llm_providers(&self) -> Result<Vec<LlmProvider>> { Ok(vec![]) }
        async fn replace_llm_providers(&self, _: &[LlmProvider], _: &str) -> Result<()> { Ok(()) }
        async fn get_llm_providers_version(&self) -> Result<Option<String>> { Ok(None) }
    }

    fn default_fields() -> TokenResponseFields {
        TokenResponseFields::default()
    }

    fn make_token_response_json(access: &str, refresh: &str) -> Vec<u8> {
        serde_json::json!({
            "access_token": access,
            "refresh_token": refresh,
            "token_type": "Bearer",
            "expires_in": 3600,
            "scope": "openid email"
        }).to_string().into_bytes()
    }

    #[tokio::test]
    async fn test_intercept_and_resolve() {
        let ks = Arc::new(MockKeyStore::new());
        let vault = OAuthTokenVault::new(ks.clone());

        let body = make_token_response_json("ya29.real_access", "1//real_refresh");
        let modified = vault.intercept_token_response(
            "google", "vm-1", &body, &default_fields(), None, vec![],
        ).await.unwrap();

        // Parse modified response
        let doc: serde_json::Value = serde_json::from_slice(&modified).unwrap();
        let dummy_access = doc["access_token"].as_str().unwrap();
        let dummy_refresh = doc["refresh_token"].as_str().unwrap();

        // Dummy tokens should have correct prefixes
        assert!(dummy_access.starts_with("nilbox_oat:google:"));
        assert!(dummy_refresh.starts_with("nilbox_ort:google:"));

        // token_type, expires_in, scope should be preserved
        assert_eq!(doc["token_type"].as_str().unwrap(), "Bearer");
        assert_eq!(doc["expires_in"].as_u64().unwrap(), 3600);
        assert_eq!(doc["scope"].as_str().unwrap(), "openid email");

        // Resolve access token
        let real = vault.resolve_access_token(dummy_access).await.unwrap();
        assert_eq!(real, Some("ya29.real_access".to_string()));

        // Resolve refresh token
        let real_rt = vault.resolve_refresh_token(dummy_refresh).await.unwrap();
        assert_eq!(real_rt, Some("1//real_refresh".to_string()));
    }

    #[tokio::test]
    async fn test_resolve_nonexistent() {
        let ks = Arc::new(MockKeyStore::new());
        let vault = OAuthTokenVault::new(ks);

        let result = vault.resolve_access_token("nilbox_oat:google:nonexistent").await.unwrap();
        assert_eq!(result, None);

        // Nonexistent session → Err (session not found)
        let result = vault.resolve_refresh_token("nilbox_ort:google:nonexistent").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("session not found"));
    }

    #[tokio::test]
    async fn test_no_refresh_token() {
        let ks = Arc::new(MockKeyStore::new());
        let vault = OAuthTokenVault::new(ks);

        let body = serde_json::json!({
            "access_token": "gho_real_access",
            "token_type": "bearer",
            "scope": "repo"
        }).to_string().into_bytes();

        let fields = TokenResponseFields {
            refresh_token_field: "".into(), // GitHub: no refresh token
            ..default_fields()
        };

        let modified = vault.intercept_token_response(
            "github", "vm-1", &body, &fields, None, vec![],
        ).await.unwrap();

        let doc: serde_json::Value = serde_json::from_slice(&modified).unwrap();
        let dummy_access = doc["access_token"].as_str().unwrap();
        assert!(dummy_access.starts_with("nilbox_oat:github:"));

        // No refresh_token field should exist in the modified response
        // (it wasn't in the original either)
        assert!(doc.get("refresh_token").is_none());
    }

    #[tokio::test]
    async fn test_nested_json_fields() {
        let ks = Arc::new(MockKeyStore::new());
        let vault = OAuthTokenVault::new(ks);

        let body = serde_json::json!({
            "data": {
                "access_token": "nested_real_access",
                "refresh_token": "nested_real_refresh",
                "expires_in": 7200,
                "token_type": "Bearer"
            }
        }).to_string().into_bytes();

        let fields = TokenResponseFields {
            access_token_field: "data.access_token".into(),
            refresh_token_field: "data.refresh_token".into(),
            expires_in_field: "data.expires_in".into(),
            token_type_field: "data.token_type".into(),
            response_format: "json".into(),
        };

        let modified = vault.intercept_token_response(
            "custom", "vm-1", &body, &fields, None, vec![],
        ).await.unwrap();

        let doc: serde_json::Value = serde_json::from_slice(&modified).unwrap();
        let dummy_access = doc["data"]["access_token"].as_str().unwrap();
        let dummy_refresh = doc["data"]["refresh_token"].as_str().unwrap();

        assert!(dummy_access.starts_with("nilbox_oat:custom:"));
        assert!(dummy_refresh.starts_with("nilbox_ort:custom:"));

        // Resolve should work
        let real = vault.resolve_access_token(dummy_access).await.unwrap();
        assert_eq!(real, Some("nested_real_access".to_string()));
    }

    #[tokio::test]
    async fn test_form_urlencoded_response() {
        let ks = Arc::new(MockKeyStore::new());
        let vault = OAuthTokenVault::new(ks);

        let body = b"access_token=form_real_access&refresh_token=form_real_refresh&token_type=bearer&expires_in=3600";

        let fields = TokenResponseFields {
            response_format: "form".into(),
            ..default_fields()
        };

        let modified = vault.intercept_token_response(
            "legacy", "vm-1", body, &fields, None, vec![],
        ).await.unwrap();

        // Parse as form-urlencoded
        let pairs: HashMap<String, String> = url::form_urlencoded::parse(&modified)
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();

        assert!(pairs["access_token"].starts_with("nilbox_oat:legacy:"));
        assert!(pairs["refresh_token"].starts_with("nilbox_ort:legacy:"));
    }

    #[tokio::test]
    async fn test_cleanup_vm_sessions() {
        let ks = Arc::new(MockKeyStore::new());
        let vault = OAuthTokenVault::new(ks.clone());

        let body = make_token_response_json("access1", "refresh1");
        vault.intercept_token_response("google", "vm-1", &body, &default_fields(), None, vec![]).await.unwrap();
        vault.intercept_token_response("google", "vm-1", &body, &default_fields(), None, vec![]).await.unwrap();
        vault.intercept_token_response("google", "vm-2", &body, &default_fields(), None, vec![]).await.unwrap();

        // 3 sessions total, 2 for vm-1
        let accounts = ks.list().await.unwrap();
        let oauth_count = accounts.iter().filter(|a| a.starts_with(KEYSTORE_PREFIX)).count();
        assert_eq!(oauth_count, 3);

        // Cleanup vm-1
        vault.cleanup_vm_sessions("vm-1").await.unwrap();

        let accounts = ks.list().await.unwrap();
        let remaining: Vec<&String> = accounts.iter()
            .filter(|a| a.starts_with(KEYSTORE_PREFIX))
            .collect();
        assert_eq!(remaining.len(), 1);

        // Remaining session should be vm-2
        let session: OAuthSession = serde_json::from_str(
            &ks.get(remaining[0]).await.unwrap()
        ).unwrap();
        assert_eq!(session.vm_id, "vm-2");
    }

    #[test]
    fn test_is_dummy_token() {
        assert!(OAuthTokenVault::is_dummy_access_token("nilbox_oat:google:abc123"));
        assert!(!OAuthTokenVault::is_dummy_access_token("ya29.real_token"));
        assert!(!OAuthTokenVault::is_dummy_access_token("nilbox_ort:google:abc123"));

        assert!(OAuthTokenVault::is_dummy_refresh_token("nilbox_ort:google:abc123"));
        assert!(!OAuthTokenVault::is_dummy_refresh_token("1//real_refresh"));
        assert!(!OAuthTokenVault::is_dummy_refresh_token("nilbox_oat:google:abc123"));
    }

    #[test]
    fn test_parse_dummy_token() {
        let (pid, uuid) = parse_dummy_token("nilbox_oat:google:abc123", ACCESS_PREFIX).unwrap();
        assert_eq!(pid, "google");
        assert_eq!(uuid, "abc123");

        assert!(parse_dummy_token("not_a_dummy", ACCESS_PREFIX).is_none());
        assert!(parse_dummy_token("nilbox_oat:", ACCESS_PREFIX).is_none());
        assert!(parse_dummy_token("nilbox_oat::", ACCESS_PREFIX).is_none());
    }

    #[test]
    fn test_extract_by_path() {
        let doc = serde_json::json!({
            "access_token": "test",
            "data": { "nested": "value" },
            "expires_in": 3600
        });
        assert_eq!(extract_by_path(&doc, "access_token").unwrap(), "test");
        assert_eq!(extract_by_path(&doc, "data.nested").unwrap(), "value");
        assert_eq!(extract_by_path(&doc, "expires_in").unwrap(), "3600");
        assert!(extract_by_path(&doc, "nonexistent").is_err());
    }

    #[test]
    fn test_replace_by_path() {
        let mut doc = serde_json::json!({
            "access_token": "old",
            "data": { "token": "old_nested" }
        });
        replace_by_path(&mut doc, "access_token", "new").unwrap();
        assert_eq!(doc["access_token"].as_str().unwrap(), "new");

        replace_by_path(&mut doc, "data.token", "new_nested").unwrap();
        assert_eq!(doc["data"]["token"].as_str().unwrap(), "new_nested");
    }

    #[test]
    fn test_form_round_trip() {
        let body = b"access_token=abc&refresh_token=def&token_type=bearer";
        let json = parse_form_to_json(body).unwrap();
        assert_eq!(json["access_token"].as_str().unwrap(), "abc");
        assert_eq!(json["refresh_token"].as_str().unwrap(), "def");

        let back = json_to_form(&json).unwrap();
        let pairs: HashMap<String, String> = url::form_urlencoded::parse(&back)
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert_eq!(pairs["access_token"], "abc");
        assert_eq!(pairs["refresh_token"], "def");
    }

    #[tokio::test]
    async fn test_refresh_preserves_old_refresh_token() {
        let ks = Arc::new(MockKeyStore::new());
        let vault = OAuthTokenVault::new(ks);

        // 1. Initial grant: access_token + refresh_token
        let body = make_token_response_json("ya29.initial", "1//original_refresh");
        let modified = vault.intercept_token_response(
            "google", "vm-1", &body, &default_fields(), None, vec![],
        ).await.unwrap();

        let doc: serde_json::Value = serde_json::from_slice(&modified).unwrap();
        let dummy_access = doc["access_token"].as_str().unwrap().to_string();
        let dummy_refresh = doc["refresh_token"].as_str().unwrap().to_string();

        // Extract session UUID from dummy token
        let (_, session_uuid) = parse_dummy_token(&dummy_refresh, REFRESH_PREFIX).unwrap();

        // Verify initial refresh token is stored
        let real_rt = vault.resolve_refresh_token(&dummy_refresh).await.unwrap();
        assert_eq!(real_rt, Some("1//original_refresh".to_string()));

        // 2. Simulate refresh response: Google returns only access_token (no refresh_token)
        let refresh_response = serde_json::json!({
            "access_token": "ya29.refreshed",
            "token_type": "Bearer",
            "expires_in": 3600,
        }).to_string().into_bytes();

        let modified2 = vault.intercept_token_response(
            "google", "vm-1", &refresh_response, &default_fields(),
            Some(session_uuid), vec![],
        ).await.unwrap();

        // 3. Verify: new access_token is stored
        let doc2: serde_json::Value = serde_json::from_slice(&modified2).unwrap();
        let new_dummy_access = doc2["access_token"].as_str().unwrap();
        let real_access = vault.resolve_access_token(new_dummy_access).await.unwrap();
        assert_eq!(real_access, Some("ya29.refreshed".to_string()));

        // 4. KEY ASSERTION: old refresh_token is PRESERVED (not overwritten to None)
        let real_rt = vault.resolve_refresh_token(&dummy_refresh).await.unwrap();
        assert_eq!(real_rt, Some("1//original_refresh".to_string()),
            "refresh_token should be preserved when new response lacks one");
    }

    #[tokio::test]
    async fn test_refresh_updates_refresh_token_when_provided() {
        let ks = Arc::new(MockKeyStore::new());
        let vault = OAuthTokenVault::new(ks);

        // 1. Initial grant
        let body = make_token_response_json("ya29.initial", "1//old_refresh");
        let modified = vault.intercept_token_response(
            "google", "vm-1", &body, &default_fields(), None, vec![],
        ).await.unwrap();

        let doc: serde_json::Value = serde_json::from_slice(&modified).unwrap();
        let dummy_refresh = doc["refresh_token"].as_str().unwrap().to_string();
        let (_, session_uuid) = parse_dummy_token(&dummy_refresh, REFRESH_PREFIX).unwrap();

        // 2. Refresh response WITH new refresh_token (Google rotated it)
        let refresh_response = make_token_response_json("ya29.refreshed", "1//rotated_refresh");
        vault.intercept_token_response(
            "google", "vm-1", &refresh_response, &default_fields(),
            Some(session_uuid), vec![],
        ).await.unwrap();

        // 3. refresh_token should be UPDATED to the new one
        let real_rt = vault.resolve_refresh_token(&dummy_refresh).await.unwrap();
        assert_eq!(real_rt, Some("1//rotated_refresh".to_string()),
            "refresh_token should be updated when new response includes one");
    }

    #[tokio::test]
    async fn test_try_cached_token_response_valid() {
        let ks = Arc::new(MockKeyStore::new());
        let vault = OAuthTokenVault::new(ks);

        // Create session with expires_in=3600 (1 hour from now)
        let body = make_token_response_json("ya29.cached", "1//refresh_cached");
        let modified = vault.intercept_token_response(
            "google", "vm-1", &body, &default_fields(), None, vec![],
        ).await.unwrap();

        let doc: serde_json::Value = serde_json::from_slice(&modified).unwrap();
        let dummy_refresh = doc["refresh_token"].as_str().unwrap();

        // Token is valid (3600s - 60s buffer) → should return cached response
        let cached = vault.try_cached_token_response(dummy_refresh, &default_fields()).await;
        assert!(cached.is_some(), "should return cached response when token is valid");

        let cached_doc: serde_json::Value = serde_json::from_slice(&cached.unwrap()).unwrap();
        assert!(cached_doc["access_token"].as_str().unwrap().starts_with("nilbox_oat:google:"));
        assert!(cached_doc["refresh_token"].as_str().unwrap().starts_with("nilbox_ort:google:"));
        assert!(cached_doc["expires_in"].as_u64().unwrap() > 3500); // ~3540
        assert_eq!(cached_doc["token_type"].as_str().unwrap(), "Bearer");
    }

    #[tokio::test]
    async fn test_try_cached_token_response_expired() {
        let ks = Arc::new(MockKeyStore::new());
        let vault = OAuthTokenVault::new(ks.clone());

        // Create session with expires_in=30 (< 60s buffer → treated as expired)
        let body = serde_json::json!({
            "access_token": "ya29.short",
            "refresh_token": "1//refresh_short",
            "token_type": "Bearer",
            "expires_in": 30,
        }).to_string().into_bytes();

        let modified = vault.intercept_token_response(
            "google", "vm-1", &body, &default_fields(), None, vec![],
        ).await.unwrap();

        let doc: serde_json::Value = serde_json::from_slice(&modified).unwrap();
        let dummy_refresh = doc["refresh_token"].as_str().unwrap();

        // Token expires in 30s but buffer is 60s → should NOT return cached
        let cached = vault.try_cached_token_response(dummy_refresh, &default_fields()).await;
        assert!(cached.is_none(), "should not return cached when token expires within buffer");
    }
}
