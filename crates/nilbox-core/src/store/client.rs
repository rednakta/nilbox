//! Authenticated HTTP client for nilbox Store API

use std::sync::Arc;
use anyhow::{Result, anyhow};
use reqwest::{Method, RequestBuilder};
use serde::Deserialize;
use tracing::debug;

use super::auth::StoreAuth;
use super::challenge::ChallengeVerifier;

/// Response from `GET /apps/{app_id}/download` — signed download URL + integrity info.
#[derive(Debug, Deserialize)]
pub struct DownloadUrlResponse {
    pub download_url: String,
    pub sha256: Option<String>,
    pub store_signature: Option<String>,
    pub publisher_signature: Option<String>,
}

pub struct StoreClient {
    http: reqwest::Client,
    store_url: String,
    auth: Arc<StoreAuth>,
    challenge: Arc<ChallengeVerifier>,
}

impl StoreClient {
    pub fn new(store_url: &str, auth: Arc<StoreAuth>) -> Self {
        Self {
            http: super::pinning::build_pinned_http_client(),
            store_url: store_url.trim_end_matches('/').to_string(),
            auth,
            challenge: Arc::new(ChallengeVerifier::new()),
        }
    }

    /// Make an authenticated request. On 401, attempts a single token refresh + retry.
    /// Verifies server identity via challenge-response on first call.
    pub async fn request(&self, method: Method, path: &str) -> Result<reqwest::Response> {
        self.ensure_challenge_verified().await?;

        let url = format!("{}{}", self.store_url, path);

        // First attempt
        let resp = self.build_request(method.clone(), &url).await?
            .send()
            .await
            .map_err(|e| anyhow!("Request failed: {}", e))?;

        if resp.status().as_u16() != 401 {
            return Ok(resp);
        }

        // 401 — try refresh
        debug!("Got 401, attempting token refresh");
        self.auth.refresh().await?;

        // Retry
        let resp = self.build_request(method, &url).await?
            .send()
            .await
            .map_err(|e| anyhow!("Retry request failed: {}", e))?;

        Ok(resp)
    }

    /// Make an authenticated GET request.
    pub async fn get(&self, path: &str) -> Result<reqwest::Response> {
        self.request(Method::GET, path).await
    }

    /// Make an authenticated POST request with JSON body.
    pub async fn post_json<T: serde::Serialize>(
        &self,
        path: &str,
        body: &T,
    ) -> Result<reqwest::Response> {
        self.ensure_challenge_verified().await?;

        let url = format!("{}{}", self.store_url, path);

        let resp = self.build_request(Method::POST, &url).await?
            .json(body)
            .send()
            .await
            .map_err(|e| anyhow!("Request failed: {}", e))?;

        if resp.status().as_u16() != 401 {
            return Ok(resp);
        }

        debug!("Got 401, attempting token refresh");
        self.auth.refresh().await?;

        let resp = self.build_request(Method::POST, &url).await?
            .json(body)
            .send()
            .await
            .map_err(|e| anyhow!("Retry request failed: {}", e))?;

        Ok(resp)
    }

    async fn build_request(&self, method: Method, url: &str) -> Result<RequestBuilder> {
        let mut builder = self.http.request(method, url);
        if let Some(token) = self.auth.access_token().await {
            builder = builder.header("Authorization", format!("Bearer {}", token));
        }
        Ok(builder)
    }

    /// Request a signed download URL for an app's VM image.
    pub async fn request_download_url(&self, app_id: &str) -> Result<DownloadUrlResponse> {
        let path = format!("/apps/{}/download", app_id);
        let resp = self.get(&path).await?;

        if !resp.status().is_success() {
            return Err(anyhow!(
                "Download URL request failed: HTTP {}",
                resp.status()
            ));
        }

        resp.json::<DownloadUrlResponse>()
            .await
            .map_err(|e| anyhow!("Failed to parse download URL response: {}", e))
    }

    /// Expose the pinned HTTP client for streaming downloads.
    pub fn http_client(&self) -> &reqwest::Client {
        &self.http
    }

    async fn ensure_challenge_verified(&self) -> Result<()> {
        self.challenge.verify(&self.http, &self.store_url).await
    }
}
