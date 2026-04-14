//! Store authentication — polling-based login + keys.db token storage

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::RwLock;
use anyhow::{Result, anyhow};
use serde::Deserialize;
use tracing::{warn, debug};

use crate::keystore::KeyStore;

const STORE_REFRESH_TOKEN: &str = "store:refresh_token";
const STORE_EMAIL: &str = "store:email";

#[derive(Debug, Deserialize)]
struct SessionResponse {
    session_id: String,
    login_url: String,
    #[allow(dead_code)]
    expires_in: Option<u64>,
    poll_interval: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    force_upgrade: Option<bool>,
    min_version: Option<String>,
    upgrade_message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PollResponse {
    status: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
    force_upgrade: Option<bool>,
    min_version: Option<String>,
    upgrade_message: Option<String>,
}

/// Force upgrade info emitted as Tauri event payload.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ForceUpgradeInfo {
    pub min_version: String,
    pub upgrade_message: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AuthStatus {
    pub authenticated: bool,
    pub email: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UserInfo {
    #[allow(dead_code)]
    #[serde(rename = "id", default)]
    user_id: String,
    email: String,
}

pub struct StoreAuth {
    access_token: Arc<RwLock<Option<String>>>,
    store_url: String,
    http: reqwest::Client,
    /// Ensures try_restore runs at most once (deferred from app startup).
    restore_once: tokio::sync::OnceCell<()>,
    keystore: Arc<dyn KeyStore>,
    /// Cached email (avoids network round-trip on every auth_status call).
    email: Arc<RwLock<Option<String>>>,
    /// Force upgrade info from last auth response.
    force_upgrade_info: Arc<RwLock<Option<ForceUpgradeInfo>>>,
    /// Set to true to cancel the current background login polling task.
    login_cancel: Arc<AtomicBool>,
}

impl StoreAuth {
    pub fn new(store_url: &str, keystore: Arc<dyn KeyStore>) -> Self {
        // Use native TLS so that system trust store (including mkcert dev certs) is respected.
        let http = reqwest::Client::builder()
            .use_native_tls()
            .build()
            .expect("Failed to build auth HTTP client");
        Self {
            access_token: Arc::new(RwLock::new(None)),
            store_url: store_url.trim_end_matches('/').to_string(),
            http,
            restore_once: tokio::sync::OnceCell::new(),
            keystore,
            email: Arc::new(RwLock::new(None)),
            force_upgrade_info: Arc::new(RwLock::new(None)),
            login_cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Restore session from keys.db on first call only.
    /// Call this before any auth check that needs persisted login state.
    pub async fn ensure_restored(&self) {
        self.restore_once.get_or_init(|| async {
            match self.try_restore().await {
                Ok(status) if status.authenticated => {
                    debug!("Store auth restored from keys.db");
                }
                _ => debug!("No stored auth session to restore"),
            }
        }).await;
    }

    /// Begin in-app login: create session, spawn background polling, return login_url immediately.
    /// The caller (React) should navigate the store iframe to this URL.
    /// Background task will store tokens once the user completes login.
    pub async fn begin_login(&self) -> Result<String> {
        let url = format!("{}/auth/sessions", self.store_url);
        debug!("begin_login: POST {}", url);

        let resp = self.http
            .post(&url)
            .send()
            .await
            .map_err(|e| {
                let err = anyhow!("Network error posting to {}: {}", url, e);
                warn!("{}", err);
                err
            })?;

        let status = resp.status();
        debug!("begin_login: response status {}", status);

        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            let err = anyhow!("Failed to create auth session: HTTP {} — {}", status, body);
            warn!("{}", err);
            return Err(err);
        }

        let session: SessionResponse = resp.json().await
            .map_err(|e| {
                let err = anyhow!("Failed to parse session response: {}", e);
                warn!("{}", err);
                err
            })?;

        debug!("Auth session created: {}, login_url: {}", session.session_id, session.login_url);

        // Spawn background polling task — stores tokens when user completes login
        let http = self.http.clone();
        let store_url = self.store_url.clone();
        let session_id = session.session_id.clone();
        let poll_interval = std::time::Duration::from_secs(session.poll_interval.unwrap_or(2).max(1));
        let access_token = Arc::clone(&self.access_token);
        let keystore = Arc::clone(&self.keystore);
        let email_cache = Arc::clone(&self.email);
        let force_upgrade_cache = Arc::clone(&self.force_upgrade_info);
        // Reset cancel flag for this new polling task
        self.login_cancel.store(false, Ordering::SeqCst);
        let login_cancel = Arc::clone(&self.login_cancel);

        tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(600);
            loop {
                tokio::time::sleep(poll_interval).await;

                if login_cancel.load(Ordering::SeqCst) {
                    debug!("Login polling cancelled");
                    break;
                }

                if tokio::time::Instant::now() > deadline {
                    warn!("Login polling timed out");
                    break;
                }

                let poll_resp = http
                    .post(format!("{}/auth/sessions/{}/token", store_url, session_id))
                    .send()
                    .await;

                let poll_resp = match poll_resp {
                    Ok(r) => r,
                    Err(e) => {
                        warn!("Poll request failed: {}, retrying...", e);
                        continue;
                    }
                };

                let status = poll_resp.status();
                if status.as_u16() == 404 {
                    warn!("Login session expired during background poll");
                    break;
                }
                if !status.is_success() {
                    continue;
                }

                let body: PollResponse = match poll_resp.json().await {
                    Ok(b) => b,
                    Err(e) => {
                        warn!("Failed to parse poll response: {}", e);
                        continue;
                    }
                };

                // Server returns { authorized: true, access_token, refresh_token }
                // or { authorized: false } — only break when tokens obtained or 404/timeout
                if let (Some(access), Some(refresh)) = (body.access_token, body.refresh_token) {
                    *access_token.write().await = Some(access.clone());
                    if let Err(e) = keystore.set(STORE_REFRESH_TOKEN, &refresh).await {
                        warn!("Failed to save refresh token to keys.db: {}", e);
                    }

                    // Store force upgrade info if present
                    if body.force_upgrade == Some(true) {
                        if let Some(ref min_ver) = body.min_version {
                            *force_upgrade_cache.write().await = Some(ForceUpgradeInfo {
                                min_version: min_ver.clone(),
                                upgrade_message: body.upgrade_message.clone().unwrap_or_default(),
                            });
                        }
                    }

                    // Fetch and cache email so auth_status returns it immediately
                    if let Ok(resp) = http
                        .get(format!("{}/auth/me", store_url))
                        .header("Authorization", format!("Bearer {}", access))
                        .send()
                        .await
                    {
                        if let Ok(info) = resp.json::<UserInfo>().await {
                            *email_cache.write().await = Some(info.email.clone());
                            let _ = keystore.set(STORE_EMAIL, &info.email).await;
                        }
                    }

                    debug!("Background login polling completed — tokens stored");
                    break;
                }
                // Not authorized yet — keep polling
            }
        });

        Ok(session.login_url)
    }

    /// begin_login() + open_browser(): 백그라운드 폴링 시작 후 시스템 브라우저로 열기.
    /// Tauri 데스크탑 앱용. 반환값 없음 — 토큰은 폴링 태스크가 저장함.
    pub async fn begin_login_with_browser(&self) -> Result<()> {
        let login_url = self.begin_login().await?;
        open_browser(&login_url)?;
        Ok(())
    }

    /// Cancel the current background login polling task (if any).
    pub fn cancel_login(&self) {
        self.login_cancel.store(true, Ordering::SeqCst);
    }

    /// Start polling-based login flow:
    /// 1. POST /auth/sessions → get session_id + login_url
    /// 2. Open browser with login_url
    /// 3. Poll /auth/sessions/{id}/token until authorized
    /// 4. Store tokens
    pub async fn login(&self) -> Result<AuthStatus> {
        // 1. Create session
        let resp = self.http
            .post(format!("{}/auth/sessions", self.store_url))
            .send()
            .await
            .map_err(|e| anyhow!("Failed to create auth session: {}", e))?;

        if !resp.status().is_success() {
            return Err(anyhow!("Failed to create auth session: HTTP {}", resp.status()));
        }

        let session: SessionResponse = resp.json().await
            .map_err(|e| anyhow!("Failed to parse session response: {}", e))?;

        debug!("Auth session created, opening browser for login");

        // 2. Open browser
        open_browser(&session.login_url)?;

        // 3. Poll for token
        let poll_interval = std::time::Duration::from_secs(session.poll_interval.unwrap_or(2).max(1));
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(600); // 10 min max

        loop {
            tokio::time::sleep(poll_interval).await;

            if tokio::time::Instant::now() > deadline {
                return Err(anyhow!("Login timed out — session expired"));
            }

            let poll_resp = self.http
                .post(format!("{}/auth/sessions/{}/token", self.store_url, session.session_id))
                .send()
                .await;

            let poll_resp = match poll_resp {
                Ok(r) => r,
                Err(e) => {
                    warn!("Poll request failed: {}, retrying...", e);
                    continue;
                }
            };

            let status = poll_resp.status();
            if status.as_u16() == 404 {
                return Err(anyhow!("Login session expired"));
            }
            if !status.is_success() && status.as_u16() != 200 {
                // 202 or other — keep polling
                debug!("Poll returned HTTP {}, continuing...", status);
                continue;
            }

            let body: PollResponse = poll_resp.json().await
                .map_err(|e| anyhow!("Failed to parse poll response: {}", e))?;

            if body.status.as_deref() == Some("pending") {
                continue;
            }

            // Got tokens
            if let (Some(access), Some(refresh)) = (body.access_token, body.refresh_token) {
                self.store_tokens(&access, &refresh).await?;

                // Store force upgrade info if present
                if body.force_upgrade == Some(true) {
                    if let Some(ref min_ver) = body.min_version {
                        *self.force_upgrade_info.write().await = Some(ForceUpgradeInfo {
                            min_version: min_ver.clone(),
                            upgrade_message: body.upgrade_message.clone().unwrap_or_default(),
                        });
                    }
                }

                let email = self.fetch_email().await.ok();
                if let Some(ref e) = email {
                    let _ = self.keystore.set(STORE_EMAIL, e).await;
                    *self.email.write().await = Some(e.clone());
                }
                debug!("Login successful");
                return Ok(AuthStatus { authenticated: true, email });
            }

            // Unexpected response shape — keep polling
        }
    }

    /// Refresh access token using stored refresh token.
    pub async fn refresh(&self) -> Result<()> {
        let refresh_token = self.load_refresh_token().await?;

        let resp = self.http
            .post(format!("{}/auth/refresh", self.store_url))
            .json(&serde_json::json!({ "refresh_token": refresh_token }))
            .send()
            .await
            .map_err(|e| anyhow!("Refresh request failed: {}", e))?;

        if !resp.status().is_success() {
            // Refresh token is invalid/expired — clear everything
            self.clear_tokens().await;
            return Err(anyhow!("Refresh token rejected (HTTP {})", resp.status()));
        }

        let tokens: TokenResponse = resp.json().await
            .map_err(|e| anyhow!("Failed to parse refresh response: {}", e))?;

        self.store_tokens(&tokens.access_token, &tokens.refresh_token).await?;

        // Update force upgrade info from refresh response
        if tokens.force_upgrade == Some(true) {
            if let Some(ref min_ver) = tokens.min_version {
                *self.force_upgrade_info.write().await = Some(ForceUpgradeInfo {
                    min_version: min_ver.clone(),
                    upgrade_message: tokens.upgrade_message.clone().unwrap_or_default(),
                });
            }
        } else {
            *self.force_upgrade_info.write().await = None;
        }

        debug!("Tokens refreshed successfully");
        Ok(())
    }

    /// Clear tokens from memory and keys.db.
    pub async fn logout(&self) {
        self.clear_tokens().await;
        debug!("Logged out from store");
    }

    /// Check if we have a valid access token.
    pub async fn is_authenticated(&self) -> bool {
        self.access_token.read().await.is_some()
    }

    /// Get the current access token (for HTTP requests).
    pub async fn access_token(&self) -> Option<String> {
        self.access_token.read().await.clone()
    }

    /// Try to restore session from keys.db on startup.
    pub async fn try_restore(&self) -> Result<AuthStatus> {
        // 1. Try keys.db
        let refresh_token = match self.load_refresh_token().await {
            Ok(t) => t,
            Err(_) => {
                // 2. Fallback: try legacy file
                match load_legacy_refresh_token() {
                    Some(t) => {
                        debug!("Migrating refresh token from legacy file to keys.db");
                        if let Err(e) = self.keystore.set(STORE_REFRESH_TOKEN, &t).await {
                            warn!("Failed to migrate refresh token to keys.db: {}", e);
                        }
                        remove_legacy_file();
                        t
                    }
                    None => return Ok(AuthStatus { authenticated: false, email: None }),
                }
            }
        };

        // Try refreshing
        let resp = self.http
            .post(format!("{}/auth/refresh", self.store_url))
            .json(&serde_json::json!({ "refresh_token": refresh_token }))
            .send()
            .await;

        let resp = match resp {
            Ok(r) if r.status().is_success() => r,
            _ => {
                // Can't refresh — silently stay unauthenticated
                debug!("Could not restore auth session from keys.db");
                return Ok(AuthStatus { authenticated: false, email: None });
            }
        };

        let tokens: TokenResponse = resp.json().await
            .map_err(|e| anyhow!("Failed to parse refresh response: {}", e))?;

        self.store_tokens(&tokens.access_token, &tokens.refresh_token).await?;

        // Update force upgrade info from restore
        if tokens.force_upgrade == Some(true) {
            if let Some(ref min_ver) = tokens.min_version {
                *self.force_upgrade_info.write().await = Some(ForceUpgradeInfo {
                    min_version: min_ver.clone(),
                    upgrade_message: tokens.upgrade_message.clone().unwrap_or_default(),
                });
            }
        }

        // Try to load cached email from keys.db first, then fetch from server
        let email = match self.keystore.get(STORE_EMAIL).await {
            Ok(e) => Some(e),
            Err(_) => self.fetch_email().await.ok(),
        };
        if let Some(ref e) = email {
            *self.email.write().await = Some(e.clone());
            let _ = self.keystore.set(STORE_EMAIL, e).await;
        }

        debug!("Auth session restored from keys.db");
        Ok(AuthStatus { authenticated: true, email })
    }

    /// Get auth status (authenticated + email).
    pub async fn auth_status(&self) -> AuthStatus {
        if !self.is_authenticated().await {
            return AuthStatus { authenticated: false, email: None };
        }
        // Use cached email first
        let cached = self.email.read().await.clone();
        if cached.is_some() {
            return AuthStatus { authenticated: true, email: cached };
        }
        // Fetch from server, cache it
        let email = self.fetch_email().await.ok();
        if let Some(ref e) = email {
            *self.email.write().await = Some(e.clone());
            let _ = self.keystore.set(STORE_EMAIL, e).await;
        }
        AuthStatus { authenticated: true, email }
    }

    /// Get force upgrade info (if server flagged it).
    pub async fn force_upgrade_info(&self) -> Option<ForceUpgradeInfo> {
        self.force_upgrade_info.read().await.clone()
    }

    // ── Internal helpers ─────────────────────────────────────

    async fn load_refresh_token(&self) -> Result<String> {
        self.keystore.get(STORE_REFRESH_TOKEN).await
    }

    async fn store_tokens(&self, access: &str, refresh: &str) -> Result<()> {
        *self.access_token.write().await = Some(access.to_string());
        self.keystore.set(STORE_REFRESH_TOKEN, refresh).await?;
        Ok(())
    }

    async fn clear_tokens(&self) {
        *self.access_token.write().await = None;
        *self.email.write().await = None;
        let _ = self.keystore.delete(STORE_REFRESH_TOKEN).await;
        let _ = self.keystore.delete(STORE_EMAIL).await;
    }

    async fn fetch_email(&self) -> Result<String> {
        let token = self.access_token.read().await;
        let token = token.as_deref().ok_or_else(|| anyhow!("Not authenticated"))?;

        let resp = self.http
            .get(format!("{}/auth/me", self.store_url))
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .map_err(|e| anyhow!("Failed to fetch user info: {}", e))?;

        if !resp.status().is_success() {
            return Err(anyhow!("Failed to fetch user info: HTTP {}", resp.status()));
        }

        let info: UserInfo = resp.json().await
            .map_err(|e| anyhow!("Failed to parse user info: {}", e))?;

        Ok(info.email)
    }
}

// ── Legacy file migration helpers ─────────────────────────────

fn legacy_token_file_path() -> Option<std::path::PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").ok()?;
        Some(std::path::PathBuf::from(home).join("Library/Application Support/nilbox/store_auth.json"))
    }
    #[cfg(target_os = "linux")]
    {
        let config = std::env::var("XDG_CONFIG_HOME")
            .unwrap_or_else(|_| format!("{}/.config", std::env::var("HOME").unwrap_or_default()));
        Some(std::path::PathBuf::from(config).join("nilbox/store_auth.json"))
    }
    #[cfg(target_os = "windows")]
    {
        let appdata = std::env::var("APPDATA").ok()?;
        Some(std::path::PathBuf::from(appdata).join("nilbox/store_auth.json"))
    }
}

fn load_legacy_refresh_token() -> Option<String> {
    let path = legacy_token_file_path()?;
    let content = std::fs::read_to_string(&path).ok()?;
    let val: serde_json::Value = serde_json::from_str(&content).ok()?;
    val["refresh_token"].as_str().map(|s| s.to_string())
}

fn remove_legacy_file() {
    if let Some(path) = legacy_token_file_path() {
        if path.exists() {
            let _ = std::fs::remove_file(&path);
            debug!("Removed legacy store_auth.json");
        }
    }
}

// ── Browser opener ───────────────────────────────────────────

fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map_err(|e| anyhow!("Failed to open browser: {}", e))?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map_err(|e| anyhow!("Failed to open browser: {}", e))?;
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        // raw_arg bypasses Rust's auto-escaping so cmd.exe sees the quotes as-is,
        // preventing '&' in URLs from being treated as a command separator.
        std::process::Command::new("cmd")
            .arg("/c")
            .arg("start")
            .raw_arg("\"\"")
            .raw_arg(format!("\"{}\"", url))
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| anyhow!("Failed to open browser: {}", e))?;
    }
    Ok(())
}
