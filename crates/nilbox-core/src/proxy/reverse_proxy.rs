//! Reverse proxy handler — auth delegation via AuthRouter strategy pattern

use crate::config_store::ConfigStore;
use crate::events::{EventEmitter, emit_typed};
use crate::gateway::Gateway;
use crate::keystore::KeyStore;
use crate::monitoring::MonitoringCollector;
use crate::vsock::stream::VirtualStream;
use crate::vsock::VsockStream;
use crate::proxy::auth_delegator::AuthDelegator;
use crate::proxy::auth_delegator::aws_sigv4::AwsSigV4Delegator;
use crate::proxy::auth_delegator::telegram::TelegramDelegator;
use crate::proxy::domain_gate::DomainGate;
use crate::proxy::token_mismatch_gate::TokenMismatchGate;
use crate::proxy::auth_router::AuthRouter;
use crate::proxy::request_parser::parse_request_headers;
use crate::proxy::forward_proxy::handle_connect;
use crate::proxy::inspect::InspectCertAuthority;
use crate::proxy::oauth_script_engine::OAuthScriptEngine;
use crate::proxy::oauth_token_vault::OAuthTokenVault;
use crate::proxy::oauth_url_rewriter::OAuthUrlRewriter;
use crate::proxy::llm_detector::LlmProviderMatcher;
use crate::proxy::token_extractor::{
    extract_from_body, extract_from_sse_chunks,
    estimate_from_bytes, estimate_from_bytes_fallback,
};
use crate::token_monitor::TokenUsageLogger;
use crate::proxy::token_limit::TokenLimitChecker;
use crate::store::auth::StoreAuth;
use anyhow::{Result, anyhow};
use reqwest::Client;
use std::sync::Arc;

use tracing::{debug, warn, error};
use futures::StreamExt;

/// Detected authentication header info.
pub struct AuthHeader {
    /// Header name (e.g. "x-api-key", "authorization")
    pub name: String,
    /// Prefix to prepend to the real key (e.g. "Bearer ", "token ", or "")
    pub prefix: String,
    /// Account identifier (the header value after stripping the prefix)
    pub account: String,
}

/// Detect an auth header from request headers.
/// Returns None if no auth header is present.
pub fn detect_auth_header(headers: &std::collections::HashMap<String, String>) -> Option<AuthHeader> {
    // Check x-api-key first
    if let Some(value) = headers.get("x-api-key") {
        if !value.is_empty() {
            return Some(AuthHeader {
                name: "x-api-key".to_string(),
                prefix: String::new(),
                account: value.clone(),
            });
        }
    }

    // Check x-goog-api-key (Google Gemini / google-genai SDK)
    if let Some(value) = headers.get("x-goog-api-key") {
        if !value.is_empty() {
            return Some(AuthHeader {
                name: "x-goog-api-key".to_string(),
                prefix: String::new(),
                account: value.clone(),
            });
        }
    }

    // Check authorization header
    if let Some(value) = headers.get("authorization") {
        if !value.is_empty() {
            // Split known prefixes: "Bearer ", "token ", "Basic "
            for known_prefix in &["Bearer ", "token ", "Basic "] {
                if let Some(rest) = value.strip_prefix(known_prefix) {
                    if !rest.is_empty() {
                        return Some(AuthHeader {
                            name: "authorization".to_string(),
                            prefix: known_prefix.to_string(),
                            account: rest.to_string(),
                        });
                    }
                }
            }
            // No known prefix — use the full value as account
            return Some(AuthHeader {
                name: "authorization".to_string(),
                prefix: String::new(),
                account: value.clone(),
            });
        }
    }

    None
}

pub struct ReverseProxy {
    client: Client,
    gate: Arc<DomainGate>,
    token_mismatch_gate: Arc<TokenMismatchGate>,
    auth_router: Arc<AuthRouter>,
    inspect_ca: Arc<InspectCertAuthority>,
    config_store: Arc<ConfigStore>,
    keystore: Arc<dyn KeyStore>,
    emitter: Arc<dyn EventEmitter>,
    vm_id: String,
    gateway: Arc<Gateway>,
    oauth_engine: Arc<tokio::sync::RwLock<Arc<OAuthScriptEngine>>>,
    oauth_vault: Arc<OAuthTokenVault>,
    oauth_rewriter: OAuthUrlRewriter,
    llm_matcher:         Arc<LlmProviderMatcher>,
    token_logger:        Arc<TokenUsageLogger>,
    token_limit_checker: Arc<TokenLimitChecker>,
    monitoring:          Arc<MonitoringCollector>,
    store_auth:          Arc<StoreAuth>,
}

impl ReverseProxy {
    pub fn new(
        gate: Arc<DomainGate>,
        token_mismatch_gate: Arc<TokenMismatchGate>,
        auth_router: Arc<AuthRouter>,
        inspect_ca: Arc<InspectCertAuthority>,
        config_store: Arc<ConfigStore>,
        keystore: Arc<dyn KeyStore>,
        emitter: Arc<dyn EventEmitter>,
        vm_id: String,
        gateway: Arc<Gateway>,
        oauth_engine: Arc<tokio::sync::RwLock<Arc<OAuthScriptEngine>>>,
        oauth_vault: Arc<OAuthTokenVault>,
        llm_matcher:         Arc<LlmProviderMatcher>,
        token_logger:        Arc<TokenUsageLogger>,
        token_limit_checker: Arc<TokenLimitChecker>,
        monitoring:          Arc<MonitoringCollector>,
        store_auth:          Arc<StoreAuth>,
    ) -> Self {
        let oauth_rewriter = OAuthUrlRewriter::new(oauth_engine.clone());
        Self { client: Client::new(), gate, token_mismatch_gate, auth_router, inspect_ca, config_store, keystore, emitter, vm_id, gateway, oauth_engine, oauth_vault, oauth_rewriter, llm_matcher, token_logger, token_limit_checker, monitoring, store_auth }
    }

    pub async fn handle_request(&self, mut stream: VirtualStream) -> Result<()> {
        let payload = match stream.read().await {
            Ok(p) => p,
            Err(e) => {
                error!("Failed to read initial frame: {}", e);
                return Err(e);
            }
        };

        let parsed_req = match parse_request_headers(&payload) {
            Ok(req) => req,
            Err(e) => {
                warn!("Invalid HTTP request headers: {}", e);
                stream.close().await?;
                return Err(e);
            }
        };

        // Handle __nilbox__ control requests (e.g., open-url for OAuth browser delegation)
        if parsed_req.path.contains("/__nilbox__/") {
            return self.handle_nilbox_control(&parsed_req, &mut stream).await;
        }

        // Handle OAuth token endpoint (dynamic detection via script engine)
        if parsed_req.method == "POST" && self.oauth_engine.read().await.find_by_token_path(&parsed_req.path).is_some() {
            return self.handle_oauth_token(payload, &parsed_req, &mut stream).await;
        }

        // Block AWS IMDS (Instance Metadata Service) requests — not reachable from host
        {
            let target = if parsed_req.method == "CONNECT" {
                parsed_req.path.splitn(2, ':').next().unwrap_or(&parsed_req.path)
            } else {
                parsed_req.headers.get("host").map(|h| h.split(':').next().unwrap_or(h)).unwrap_or("")
            };
            if target == "169.254.169.254" {
                // debug!("Blocked IMDS request to 169.254.169.254");
                let _ = stream.write(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n").await;
                stream.close().await?;
                return Ok(());
            }
        }

        if parsed_req.method == "CONNECT" {
            let domain = parsed_req.path.splitn(2, ':').next().unwrap_or(&parsed_req.path);
            let is_allowed = self.gate.is_allowed(domain).await;

            let gate_opt = if is_allowed {
                None
            } else {
                Some(self.gate.clone())
            };

            return handle_connect(
                stream, parsed_req, gate_opt, &self.vm_id,
                is_allowed,
                self.inspect_ca.clone(),
                self.auth_router.clone(),
                self.config_store.clone(),
                self.keystore.clone(),
                self.emitter.clone(),
                self.token_mismatch_gate.clone(),
                self.client.clone(),
                self.oauth_engine.clone(),
                self.oauth_vault.clone(),
                self.llm_matcher.clone(),
                self.token_logger.clone(),
                self.token_limit_checker.clone(),
                self.monitoring.clone(),
                self.gateway.clone(),
            ).await;
        }

        let host = parsed_req.headers.get("host").ok_or(anyhow!("Missing Host header"))?;
        // `domain` = hostname only (no port) — used for allowlist lookups
        // `host` = full host:port — used for upstream URL construction
        let domain = host.split(':').next().unwrap_or(host);

        if !self.gate.is_allowed(domain).await
        {
            let port: u16 = host.splitn(2, ':').nth(1)
                .and_then(|p| p.parse().ok())
                .unwrap_or(80);
            if !self.gate.check(domain, port, &self.vm_id, "proxy").await {
                warn!("Blocked request to non-whitelisted domain: {}", domain);
                let _ = stream.write(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n").await;
                stream.close().await?;
                return Ok(());
            }
        }

        // Detect auth header from the request
        let auth_info = detect_auth_header(&parsed_req.headers);

        // For proxy-style requests the path is an absolute URI (e.g. "http://example.com/foo").
        // Strip the scheme + host so we forward only the path portion.
        // Always use HTTPS for upstream connections.
        let req_path: &str = if let Some(rest) = parsed_req.path.strip_prefix("https://") {
            let i = rest.find('/').unwrap_or(rest.len());
            let p = &rest[i..];
            if p.is_empty() { "/" } else { p }
        } else if let Some(rest) = parsed_req.path.strip_prefix("http://") {
            let i = rest.find('/').unwrap_or(rest.len());
            let p = &rest[i..];
            if p.is_empty() { "/" } else { p }
        } else {
            parsed_req.path.as_str()
        };
        // AWS proxy route path-based interception (path prefix → real AWS host + SigV4 re-sign)
        if let Ok(Some(route)) = self.config_store.find_aws_proxy_route(req_path) {
            let stripped = req_path.strip_prefix(&route.path_prefix).unwrap_or("");
            let real_path = if stripped.is_empty() { "/" } else { stripped };
            let aws_url = format!("https://{}{}", route.aws_host, real_path);

            // Strip fake SigV4 headers that the VM sent with dummy credentials
            let mut clean_headers = parsed_req.headers.clone();
            for h in &["authorization", "x-amz-date", "x-amz-content-sha256", "x-amz-security-token"] {
                clean_headers.remove(*h);
            }

            let method: reqwest::Method = parsed_req.method.parse().unwrap_or(reqwest::Method::GET);
            let mut rb = self.client.request(method, &aws_url);
            for (k, v) in &clean_headers {
                if k != "host" {
                    rb = rb.header(k, v);
                }
            }

            let content_length: usize = clean_headers.get("content-length")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let mut body = Vec::new();
            if parsed_req.body_offset < payload.len() {
                body.extend_from_slice(&payload[parsed_req.body_offset..]);
            }
            while body.len() < content_length {
                match stream.read().await {
                    Ok(chunk) => body.extend_from_slice(&chunk),
                    Err(_) => break,
                }
            }
            if !body.is_empty() {
                rb = rb.body(body);
            }

            let mut request = rb.build()
                .map_err(|e| anyhow!("Failed to build AWS proxy request: {}", e))?;

            let delegator = AwsSigV4Delegator::new(self.keystore.clone(), self.config_store.clone());
            if let Err(e) = delegator.apply_auth(&mut request, &route.aws_host, "").await {
                warn!("AWS SigV4 re-signing failed for {}: {}", route.aws_host, e);
                let _ = stream.write(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n").await;
                stream.close().await?;
                return Ok(());
            }

            debug!("AWS proxy: forwarding to {}", aws_url);
            match self.client.execute(request).await {
                Ok(response) => {
                    let status = response.status();
                    let content_length = response.content_length();
                    let mut head = format!(
                        "HTTP/1.1 {} {}\r\n",
                        status.as_u16(),
                        status.canonical_reason().unwrap_or("")
                    );
                    for (k, v) in response.headers() {
                        let name = k.as_str();
                        if name == "transfer-encoding"
                            || name == "content-length"
                            || name == "connection"
                        {
                            continue;
                        }
                        head.push_str(&format!("{}: {}\r\n", name, v.to_str().unwrap_or("")));
                    }
                    if let Some(len) = content_length {
                        head.push_str(&format!("Content-Length: {}\r\n", len));
                    }
                    head.push_str("Connection: close\r\n");
                    head.push_str("\r\n");

                    if let Err(e) = stream.write(head.as_bytes()).await {
                        error!("Failed to write AWS proxy response head: {}", e);
                        return Err(e);
                    }

                    let mut stream_body = response.bytes_stream();
                    while let Some(item) = stream_body.next().await {
                        match item {
                            Ok(chunk) => {
                                if let Err(e) = stream.write(&chunk).await {
                                    error!("Failed to write AWS proxy response body: {}", e);
                                    return Err(e);
                                }
                            }
                            Err(e) => {
                                error!("Error reading AWS upstream response: {}", e);
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("AWS upstream request failed: {}", e);
                    let _ = stream.write(b"HTTP/1.1 502 Bad Gateway\r\n\r\n").await;
                }
            }

            stream.close().await?;
            return Ok(());
        }

        let url = format!("https://{}{}", host, req_path);
        let method = parsed_req.method.parse().unwrap_or(reqwest::Method::GET);
        let mut request_builder = self.client.request(method, &url);

        // Copy all headers (auth header included — delegator will replace it)
        for (k, v) in &parsed_req.headers {
            if k == "host" { continue; }
            request_builder = request_builder.header(k, v);
        }

        let content_length: usize = parsed_req.headers
            .get("content-length")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        let mut body = Vec::new();
        if parsed_req.body_offset < payload.len() {
            body.extend_from_slice(&payload[parsed_req.body_offset..]);
        }
        while body.len() < content_length {
            match stream.read().await {
                Ok(chunk) => body.extend_from_slice(&chunk),
                Err(_) => break,
            }
        }
        let request_body_len = body.len();
        if !body.is_empty() {
            request_builder = request_builder.body(body);
        }

        // LLM detection (before auth delegation)
        let headers_vec: Vec<(String, String)> = parsed_req.headers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let llm_match = self.llm_matcher.match_request(domain, req_path, &headers_vec);

        // Pre-request token limit check
        {
            let provider_id_for_check = llm_match.as_ref()
                .map(|lm| lm.provider_id.clone())
                .or_else(|| self.llm_matcher.match_domain_only(domain));

            if let Some(ref provider_id) = provider_id_for_check {
                for pid in [provider_id.as_str(), "*"] {
                    match self.token_limit_checker.check_pre_request(&self.vm_id, pid) {
                        Ok(Some(limit_result)) if limit_result.action == "block" => {
                            warn!("Token limit exceeded for {} / {} — returning 429", self.vm_id, pid);
                            let body = b"{\"error\":\"token limit exceeded\"}";
                            let resp = format!(
                                "HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            );
                            let _ = stream.write(resp.as_bytes()).await;
                            let _ = stream.write(body).await;
                            stream.close().await?;
                            return Ok(());
                        }
                        Err(e) => warn!("Token limit check error for {} / {}: {}", self.vm_id, pid, e),
                        _ => {}
                    }
                }
            }
        }

        // Auth delegation via AuthRouter
        let mut request = request_builder.build()
            .map_err(|e| anyhow!("Failed to build request: {}", e))?;

        let (delegator, credential_account) = self.auth_router.resolve(domain).await;
        let (active_delegator, account): (Arc<dyn AuthDelegator>, Option<String>) =
            if let Some(acct) = credential_account {
                // 1. Explicit auth_route mapping takes priority
                (delegator, Some(acct))
            } else {
                let mut tokens = self.config_store
                    .list_domain_tokens(domain)
                    .unwrap_or_default();
                // Merge in-memory pending tokens from allow_once decisions
                let pending = self.gate.take_pending_tokens(domain).await;
                for t in pending {
                    if !tokens.contains(&t) {
                        tokens.push(t);
                    }
                }
                if !tokens.is_empty() {
                    // 2. domain_token_accounts present → auto-detect engine
                    let is_aws = tokens.iter().any(|t| t == "AWS_ACCESS_KEY_ID")
                        && tokens.iter().any(|t| t == "AWS_SECRET_ACCESS_KEY");
                    if is_aws {
                        let aws_del: Arc<dyn AuthDelegator> = Arc::new(
                            AwsSigV4Delegator::new(self.keystore.clone(), self.config_store.clone())
                        );
                        (aws_del, Some(domain.to_string()))
                    } else if tokens.iter().any(|t| t == "TELEGRAM_BOT_TOKEN") {
                        let tg_del: Arc<dyn AuthDelegator> = Arc::new(
                            TelegramDelegator::new(self.keystore.clone())
                        );
                        (tg_del, Some("TELEGRAM_BOT_TOKEN".to_string()))
                    } else if let Some(ref ah) = auth_info {
                        // Bearer: only substitute if the request's account matches a mapped token
                        if tokens.contains(&ah.account) {
                            (delegator, Some(ah.account.clone()))
                        } else {
                            // Token mismatch: request uses different account than mapped tokens
                            // Prompt user: pass through or block?
                            let pass_through = self.token_mismatch_gate
                                .check(domain, &ah.account, &tokens)
                                .await;
                            if pass_through {
                                // User chose to send anyway — pass through unchanged
                                (delegator, None)
                            } else {
                                // User chose to cancel — block the request
                                let _ = stream.write(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n").await;
                                stream.close().await?;
                                return Ok(());
                            }
                        }
                    } else {
                        (delegator, None)
                    }
                } else if let Some(ref ah) = auth_info {
                    if OAuthTokenVault::is_dummy_access_token(&ah.account) {
                        // OAuth dummy token: check domain binding instead of env-missing
                        use crate::proxy::oauth_token_vault::OAuthDomainCheck;
                        match self.oauth_vault.check_and_resolve(&ah.account, domain).await {
                            Ok(OAuthDomainCheck::FirstUse { .. }) => {
                                let _ = self.oauth_vault.bind_domain(&ah.account, domain).await;
                                debug!("reverse_proxy: bound OAuth token to domain={}", domain);
                            }
                            Ok(OAuthDomainCheck::Match { .. }) => {
                                debug!("reverse_proxy: OAuth token domain match for domain={}", domain);
                            }
                            Ok(OAuthDomainCheck::Mismatch { ref bound_domain }) => {
                                warn!("reverse_proxy: OAuth token domain mismatch: bound={} request={}", bound_domain, domain);
                                emit_typed(
                                    &self.emitter,
                                    "oauth-domain-mismatch",
                                    &serde_json::json!({
                                        "domain": domain,
                                        "bound_domain": bound_domain,
                                        "vm_id": self.vm_id,
                                    }),
                                );
                                let _ = stream.write(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n").await;
                                stream.close().await?;
                                return Ok(());
                            }
                            Ok(OAuthDomainCheck::NotFound) | Err(_) => {
                                if self.gate.is_allowed(domain).await {
                                    emit_typed(
                                        &self.emitter,
                                        "domain-env-missing",
                                        &serde_json::json!({
                                            "domain": domain,
                                            "account": ah.account,
                                        }),
                                    );
                                }
                            }
                        }
                        (delegator, None)
                    } else if self.gate.is_allowed(domain).await {
                        // 3. Domain in allowlist but no tokens mapped — notify user to configure
                        emit_typed(
                            &self.emitter,
                            "domain-env-missing",
                            &serde_json::json!({
                                "domain": domain,
                                "account": ah.account,
                            }),
                        );
                        // Deny this request (no token substitution)
                        (delegator, None)
                    } else {
                        // Not in allowlist — fall through to original bearer behavior
                        (delegator, Some(ah.account.clone()))
                    }
                } else {
                    (delegator, None)
                }
            };

        if let Some(ref account) = account {
            if let Err(e) = active_delegator.apply_auth(&mut request, domain, account).await {
                warn!("Auth delegation failed for {}: {}", domain, e);
                // Remove auth header on failure (user cancelled — don't leak dummy token)
                if let Some(ref ah) = auth_info {
                    request.headers_mut().remove(&ah.name);
                }
            }
        }

        // Resolve OAuth dummy access tokens (nilbox_oat:) when no explicit account mapping
        if account.is_none() {
            if let Some(ref ah) = auth_info {
                if OAuthTokenVault::is_dummy_access_token(&ah.account) {
                    match self.oauth_vault.resolve_access_token(&ah.account).await {
                        Ok(Some(real)) => {
                            if let Ok(hv) = format!("{}{}", ah.prefix, real).parse() {
                                request.headers_mut().insert(
                                    reqwest::header::HeaderName::from_bytes(ah.name.as_bytes())
                                        .unwrap_or(reqwest::header::AUTHORIZATION),
                                    hv,
                                );
                                debug!("reverse_proxy: resolved dummy OAuth access token for domain={}", domain);
                            }
                        }
                        Ok(None) => warn!("reverse_proxy: dummy OAuth access token not found in vault"),
                        Err(e) => warn!("reverse_proxy: failed to resolve OAuth access token: {}", e),
                    }
                }
            }
        }

        // Auto-inject store access token for store.nilbox.run when no auth present
        if domain == "store.nilbox.run" && account.is_none() && auth_info.is_none() {
            if let Some(token) = self.store_auth.access_token().await {
                if let Ok(hv) = format!("Bearer {}", token).parse() {
                    request.headers_mut().insert(reqwest::header::AUTHORIZATION, hv);
                    debug!("reverse_proxy: injected store access token for store.nilbox.run");
                }
            }
        }

        // debug!("Forwarding request to {}", url);
        match self.client.execute(request).await {
            Ok(response) => {
                let status      = response.status();
                let status_code = status.as_u16() as i32;
                let content_length = response.content_length();

                // Detect SSE streaming from response Content-Type
                let is_streaming = response.headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .map(|ct| ct.contains("text/event-stream"))
                    .unwrap_or(false);

                let mut head = format!(
                    "HTTP/1.1 {} {}\r\n",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or("")
                );
                for (k, v) in response.headers() {
                    let name = k.as_str();
                    if name == "transfer-encoding"
                        || name == "content-length"
                        || name == "connection"
                    {
                        continue;
                    }
                    head.push_str(&format!("{}: {}\r\n", name, v.to_str().unwrap_or("")));
                }
                if let Some(len) = content_length {
                    head.push_str(&format!("Content-Length: {}\r\n", len));
                }
                head.push_str("Connection: close\r\n");
                head.push_str("\r\n");

                tracing::trace!("[PROXY-DBG] writing response head ({} bytes) to stream={} for {}",
                    head.len(), stream.stream_id, domain);
                if let Err(e) = stream.write(head.as_bytes()).await {
                    error!("[PROXY-DBG] FAILED to write response head to stream={} for {}: {}",
                        stream.stream_id, domain, e);
                    return Err(e);
                }

                // Stream response to VM while buffering for token extraction (tee)
                let mut response_buf: Vec<u8> = Vec::new();
                let mut sse_chunks: Vec<Vec<u8>> = Vec::new();
                let mut response_body_len: usize = 0;
                let mut chunk_count: u32 = 0;

                let mut stream_body = response.bytes_stream();
                while let Some(item) = stream_body.next().await {
                    match item {
                        Ok(chunk) => {
                            chunk_count += 1;
                            response_body_len += chunk.len();
                            if is_streaming {
                                sse_chunks.push(chunk.to_vec());
                            } else {
                                response_buf.extend_from_slice(&chunk);
                            }
                            tracing::trace!("[PROXY-DBG] writing chunk #{} ({} bytes, total={}) to stream={} for {}",
                                chunk_count, chunk.len(), response_body_len, stream.stream_id, domain);
                            if let Err(e) = stream.write(&chunk).await {
                                error!("[PROXY-DBG] FAILED to write response body chunk #{} ({} bytes) to stream={} for {}: {} (total_sent={})",
                                    chunk_count, chunk.len(), stream.stream_id, domain, e, response_body_len);
                                return Err(e);
                            }
                        }
                        Err(e) => {
                            error!("[PROXY-DBG] Error reading upstream response for {} after {} chunks ({} bytes): {}",
                                domain, chunk_count, response_body_len, e);
                            break;
                        }
                    }
                }
                tracing::trace!("[PROXY-DBG] response streaming complete for {}: {} chunks, {} bytes total",
                    domain, chunk_count, response_body_len);

                // Token extraction and logging
                if let Some(ref lm) = llm_match {
                    if let Some(ref provider_info) = lm.provider_info {
                        // Provider configured: accurate token extraction from API response
                        let usage = if is_streaming {
                            extract_from_sse_chunks(&sse_chunks, provider_info)
                                .unwrap_or_else(|| estimate_from_bytes_fallback(response_body_len))
                        } else {
                            extract_from_body(&response_buf, provider_info)
                                .unwrap_or_else(|| estimate_from_bytes_fallback(response_body_len))
                        };
                        if let Err(e) = self.token_logger.log(
                            &self.vm_id, &lm.provider_id, &usage,
                            Some(req_path), Some(status_code), is_streaming,
                        ) {
                            warn!("Token logging failed: {}", e);
                        }
                        if let Err(e) = self.token_limit_checker.check_soft_warnings(&self.vm_id, &lm.provider_id) {
                            warn!("Soft-warning check failed: {}", e);
                        }
                        if let Err(e) = self.token_limit_checker.check_soft_warnings(&self.vm_id, "*") {
                            warn!("Wildcard soft-warning check failed: {}", e);
                        }
                    } else {
                        // Heuristic match (no provider config): byte estimation
                        let pid = self.llm_matcher.match_domain_only(domain).unwrap_or_else(|| lm.provider_id.clone());
                        let usage = estimate_from_bytes(request_body_len, response_body_len);
                        if let Err(e) = self.token_logger.log(
                            &self.vm_id, &pid, &usage,
                            Some(req_path), Some(status_code), is_streaming,
                        ) {
                            warn!("Token logging failed: {}", e);
                        }
                        if let Err(e) = self.token_limit_checker.check_soft_warnings(&self.vm_id, &pid) {
                            warn!("Soft-warning check failed: {}", e);
                        }
                        if let Err(e) = self.token_limit_checker.check_soft_warnings(&self.vm_id, "*") {
                            warn!("Wildcard soft-warning check failed: {}", e);
                        }
                    }
                } else if let Some(provider_id) = self.llm_matcher.match_domain_only(domain) {
                    // No provider configured, but known LLM domain: byte estimation
                    let usage = estimate_from_bytes(request_body_len, response_body_len);
                    if let Err(e) = self.token_logger.log(
                        &self.vm_id, &provider_id, &usage,
                        Some(req_path), Some(status_code), is_streaming,
                    ) {
                        warn!("Token logging failed: {}", e);
                    }
                    if let Err(e) = self.token_limit_checker.check_soft_warnings(&self.vm_id, &provider_id) {
                        warn!("Soft-warning check failed: {}", e);
                    }
                    if let Err(e) = self.token_limit_checker.check_soft_warnings(&self.vm_id, "*") {
                        warn!("Wildcard soft-warning check failed: {}", e);
                    }
                }

                // Track proxy bytes with domain info for StatusBar display
                self.monitoring.record_proxy_activity(
                    domain,
                    response_body_len as u64,
                    request_body_len as u64,
                );
            }
            Err(e) => {
                error!("Upstream request failed: {}", e);
                let _ = stream.write(b"HTTP/1.1 502 Bad Gateway\r\n\r\n").await;
            }
        }

        stream.close().await?;
        Ok(())
    }

    /// Handle OAuth token endpoint request (dynamic, script-engine driven).
    ///
    /// The VM's dummy client_secret.json sets token_uri to a proxy-local URL,
    /// so the request arrives here. ScriptedOAuthDelegator replaces dummy credentials
    /// with real values from KeyStore and rewrites the URL to the real endpoint.
    ///
    /// Zero-token: On 200 OK, intercepts the response body, stores real tokens in
    /// KeyStore, and returns dummy tokens (`nilbox_oat:` / `nilbox_ort:`) to the VM.
    async fn handle_oauth_token(
        &self,
        payload: bytes::Bytes,
        parsed_req: &crate::proxy::request_parser::ParsedRequest,
        stream: &mut VirtualStream,
    ) -> Result<()> {
        debug!("OAuth token request intercepted: {}", parsed_req.path);

        // Find the provider that matches this token path
        let oauth_engine = self.oauth_engine.read().await.clone();
        let provider = oauth_engine.find_by_token_path(&parsed_req.path)
            .ok_or_else(|| anyhow!("No OAuth provider for token path: {}", parsed_req.path))?;
        let provider_id = provider.info.name.clone();
        let credential_account = provider.info.token_path.clone();

        // Get provider-specific token response field mapping
        let token_fields = oauth_engine.call_token_response_fields(provider);

        // Collect full POST body
        let content_length: usize = parsed_req.headers
            .get("content-length")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        let mut body = Vec::new();
        if parsed_req.body_offset < payload.len() {
            body.extend_from_slice(&payload[parsed_req.body_offset..]);
        }
        while body.len() < content_length {
            match stream.read().await {
                Ok(chunk) => body.extend_from_slice(&chunk),
                Err(_) => break,
            }
        }

        // Detect refresh_token grant with dummy token → replace with real token
        let body_params: std::collections::HashMap<String, String> =
            url::form_urlencoded::parse(&body)
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();

        let mut body = body;
        let mut old_refresh_dummy: Option<String> = None;
        if body_params.get("grant_type").map(|s| s.as_str()) == Some("refresh_token") {
            if let Some(dummy_rt) = body_params.get("refresh_token") {
                if OAuthTokenVault::is_dummy_refresh_token(dummy_rt) {
                    old_refresh_dummy = Some(dummy_rt.clone());
                    match self.oauth_vault.resolve_refresh_token(dummy_rt).await {
                        Ok(Some(real_rt)) => {
                            debug!("Resolved dummy refresh token for provider {}", provider_id);
                            // Rebuild body with real refresh token
                            let mut new_params = body_params.clone();
                            new_params.insert("refresh_token".to_string(), real_rt);
                            body = url::form_urlencoded::Serializer::new(String::new())
                                .extend_pairs(new_params.iter())
                                .finish()
                                .into_bytes();
                        }
                        Ok(None) => {
                            warn!("Dummy refresh token not found in vault: {}", dummy_rt);
                        }
                        Err(e) => {
                            warn!("Failed to resolve dummy refresh token: {}", e);
                        }
                    }
                }
            }
        }

        // Cache optimization: return cached token if still valid
        if let Some(ref dummy_rt) = old_refresh_dummy {
            if let Some(cached_body) = self.oauth_vault.try_cached_token_response(dummy_rt, &token_fields).await {
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    cached_body.len()
                );
                let _ = stream.write(head.as_bytes()).await;
                let _ = stream.write(&cached_body).await;
                stream.close().await?;
                return Ok(());
            }
        }

        // Build reqwest::Request targeting a placeholder URL (delegator will rewrite)
        let placeholder_url = format!("http://placeholder{}", parsed_req.path);
        let mut request_builder = self.client.request(reqwest::Method::POST, &placeholder_url);
        for (k, v) in &parsed_req.headers {
            if k == "host" || k == "content-length" { continue; }
            request_builder = request_builder.header(k, v);
        }
        request_builder = request_builder.body(body);

        let mut request = request_builder.build()
            .map_err(|e| anyhow!("Failed to build OAuth token request: {}", e))?;

        // Apply ScriptedOAuthDelegator (substitutes credentials + rewrites URL)
        let delegator = crate::proxy::auth_delegator::scripted_oauth::ScriptedOAuthDelegator::new(
            oauth_engine.clone(),
            self.keystore.clone(),
        );
        if let Err(e) = delegator.apply_auth(&mut request, "", &credential_account).await {
            error!("ScriptedOAuthDelegator failed: {}", e);
            let _ = stream.write(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n").await;
            stream.close().await?;
            return Ok(());
        }

        // Execute the request to the real token endpoint
        match self.client.execute(request).await {
            Ok(response) => {
                let status = response.status();

                // Collect response headers (excluding hop-by-hop)
                let mut resp_headers = Vec::new();
                for (k, v) in response.headers() {
                    let name = k.as_str();
                    if name == "transfer-encoding" || name == "content-length" || name == "connection" {
                        continue;
                    }
                    resp_headers.push(format!("{}: {}", name, v.to_str().unwrap_or("")));
                }

                // Collect full response body
                let resp_body = response.bytes().await
                    .map_err(|e| anyhow!("Failed to read OAuth token response body: {}", e))?;

                // Zero-token interception: replace real tokens with dummy tokens on success
                // For refresh: reuse existing session UUID instead of creating new one
                let existing_uuid = old_refresh_dummy.as_deref()
                    .and_then(|d| super::oauth_token_vault::parse_dummy_prefix(d))
                    .map(|(_, uuid)| uuid.to_string());
                let cross_domains = provider.info.cross_domains.clone();
                let final_body = if status.is_success() {
                    match self.oauth_vault.intercept_token_response(
                        &provider_id, &self.vm_id, &resp_body, &token_fields,
                        existing_uuid.as_deref(),
                        cross_domains,
                    ).await {
                        Ok(modified) => {
                            debug!("OAuth token vault: intercepted tokens for provider {}", provider_id);
                            emit_typed(
                                &self.emitter,
                                "oauth-session-updated",
                                &serde_json::json!({
                                    "vm_id": self.vm_id,
                                    "provider_id": provider_id,
                                }),
                            );
                            modified
                        }
                        Err(e) => {
                            warn!("OAuth token vault intercept failed, passing original: {}", e);
                            resp_body.to_vec()
                        }
                    }
                } else {
                    resp_body.to_vec()
                };

                // Build and send response with correct Content-Length
                let mut head = format!(
                    "HTTP/1.1 {} {}\r\n",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or("")
                );
                for h in &resp_headers {
                    head.push_str(h);
                    head.push_str("\r\n");
                }
                head.push_str(&format!("Content-Length: {}\r\n", final_body.len()));
                head.push_str("Connection: close\r\n\r\n");

                if let Err(e) = stream.write(head.as_bytes()).await {
                    error!("Failed to write OAuth token response head: {}", e);
                    return Err(e);
                }
                if !final_body.is_empty() {
                    if let Err(e) = stream.write(&final_body).await {
                        error!("Failed to write OAuth token response body: {}", e);
                        return Err(e);
                    }
                }
            }
            Err(e) => {
                error!("OAuth token upstream request failed: {}", e);
                let _ = stream.write(b"HTTP/1.1 502 Bad Gateway\r\n\r\n").await;
            }
        }

        stream.close().await?;
        Ok(())
    }

    /// Handle `/__nilbox__/open-url?url=...` control requests.
    ///
    /// 1. Extract the `url` query parameter
    /// 2. Parse `redirect_uri` from the URL to extract the callback PORT
    /// 3. Register a temporary Gateway port mapping (host:PORT → VM:PORT)
    /// 4. Open the URL in the host system browser
    /// 5. Schedule automatic port mapping removal after TTL (5 min)
    async fn handle_nilbox_control(
        &self,
        parsed_req: &crate::proxy::request_parser::ParsedRequest,
        stream: &mut VirtualStream,
    ) -> Result<()> {
        // Strip proxy-style absolute URI prefix to get the path + query
        let path = if let Some(rest) = parsed_req.path.strip_prefix("http://") {
            let i = rest.find('/').unwrap_or(rest.len());
            &rest[i..]
        } else {
            parsed_req.path.as_str()
        };

        if !path.starts_with("/__nilbox__/open-url") {
            warn!("Unknown __nilbox__ control path: {}", path);
            let _ = stream.write(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n").await;
            stream.close().await?;
            return Ok(());
        }

        // Extract url= query parameter
        let url = path.splitn(2, '?')
            .nth(1)
            .and_then(|qs| {
                url::form_urlencoded::parse(qs.as_bytes())
                    .find(|(k, _)| k == "url")
                    .map(|(_, v)| v.into_owned())
            });

        let url = match url {
            Some(u) => u,
            None => {
                warn!("open-url: missing url parameter");
                let _ = stream.write(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n").await;
                stream.close().await?;
                return Ok(());
            }
        };

        debug!("open-url: received URL for host browser: {}", url);
        // Record the raw (pre-rewrite) URL so shell-output auto-detect dedupes against it.
        record_browser_open(&url);

        // Extract domain from URL and check via DomainGate
        let final_url = if let Ok(parsed) = url::Url::parse(&url) {
            if let Some(domain) = parsed.host_str() {
                let allowed = self.gate.check(domain, 443, &self.vm_id, "browser").await;
                if !allowed {
                    debug!("open-url: domain {} denied by user, not opening browser", domain);
                    let _ = stream.write(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").await;
                    stream.close().await?;
                    return Ok(());
                }

                // Take any pending token overrides from the gate decision
                let pending_tokens = self.gate.take_pending_tokens(domain).await;

                // Rewrite dummy credentials if present
                if url.contains("NILBOX_OAUTH_") {
                    match self.oauth_rewriter.rewrite(&url, domain, &self.config_store, self.keystore.as_ref(), &pending_tokens).await {
                        Ok(rewritten) => {
                            if rewritten != url {
                                debug!("open-url: rewrote OAuth dummy credentials for domain {}", domain);
                            }
                            rewritten
                        }
                        Err(e) => {
                            warn!("open-url: OAuth rewrite failed for {}: {}, using original URL", domain, e);
                            url.clone()
                        }
                    }
                } else {
                    url.clone()
                }
            } else {
                url.clone()
            }
        } else {
            url.clone()
        };

        // Extract redirect_uri PORT from the OAuth URL for dynamic port mapping
        if let Some(callback_port) = extract_redirect_port(&final_url) {
            debug!("open-url: registering temporary port mapping for OAuth callback port {}", callback_port);
            match self.gateway.add_mapping(&self.vm_id, callback_port, callback_port).await {
                Ok(_) => {
                    debug!("open-url: port mapping localhost:{} → VM:{} registered", callback_port, callback_port);
                    // Schedule automatic removal after 5 minutes
                    let gateway = self.gateway.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(tokio::time::Duration::from_secs(300)).await;
                        gateway.remove_mapping(callback_port).await;
                        debug!("open-url: expired OAuth callback port mapping {}", callback_port);
                    });
                }
                Err(e) => {
                    warn!("open-url: failed to register port mapping for port {}: {}", callback_port, e);
                }
            }
        }

        // Open URL in host system browser
        let open_result = open_in_browser(&final_url);

        let response = if open_result.is_ok() {
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"
        } else {
            error!("open-url: failed to open browser: {:?}", open_result.err());
            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n"
        };
        let _ = stream.write(response.as_bytes()).await;
        stream.close().await?;
        Ok(())
    }
}

/// Extract the port number from `redirect_uri` in an OAuth authorization URL.
/// Parses `redirect_uri=http://localhost:{PORT}/...` from the URL query string.
///
/// Only loopback hosts are accepted to prevent authorization code interception
/// via attacker-controlled redirect URIs. Accepted forms:
/// - `127.0.0.0/8` (covers SSH tunnel forwarded ports on any 127.x.y.z)
/// - IPv6 `::1`
/// - `localhost` and any `*.localhost` subdomain (RFC 6761)
pub(crate) fn extract_redirect_port(url: &str) -> Option<u16> {
    let parsed = url::Url::parse(url).ok()?;
    let redirect_uri = parsed.query_pairs()
        .find(|(k, _)| k == "redirect_uri")
        .map(|(_, v)| v.into_owned())?;
    let redirect_url = url::Url::parse(&redirect_uri).ok()?;
    let host = redirect_url.host_str()?;
    if !is_loopback_host(host) {
        return None;
    }
    redirect_url.port()
}

/// Check whether the given host string refers to a loopback address.
fn is_loopback_host(host: &str) -> bool {
    // Strip brackets from IPv6 literal forms (e.g. "[::1]" → "::1").
    let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    // IPv4: any 127.0.0.0/8 address
    if let Ok(v4) = host.parse::<std::net::Ipv4Addr>() {
        return v4.is_loopback();
    }
    // IPv6: ::1
    if let Ok(v6) = host.parse::<std::net::Ipv6Addr>() {
        return v6.is_loopback();
    }
    // DNS: `localhost` and `*.localhost` (RFC 6761 §6.3)
    let lower = host.to_ascii_lowercase();
    lower == "localhost" || lower.ends_with(".localhost")
}

/// Cache of recently opened OAuth URLs (raw, pre-rewrite) with open timestamps.
/// Used to dedupe browser opens when multiple paths (xdg-open hook + Shell terminal
/// auto-detect) fire for the same OAuth authorize URL.
static RECENT_BROWSER_OPENS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// Record that `url` was just opened in the host browser.
pub(crate) fn record_browser_open(url: &str) {
    if let Ok(mut map) = RECENT_BROWSER_OPENS.lock() {
        let now = std::time::Instant::now();
        map.insert(url.to_string(), now);
        // Garbage-collect entries older than 10 seconds.
        map.retain(|_, t| now.duration_since(*t) < std::time::Duration::from_secs(10));
    }
}

/// Returns true if `url` was recorded as opened in the browser within the last `within`.
pub(crate) fn was_recently_opened(url: &str, within: std::time::Duration) -> bool {
    if let Ok(map) = RECENT_BROWSER_OPENS.lock() {
        if let Some(t) = map.get(url) {
            return t.elapsed() < within;
        }
    }
    false
}

/// Open a URL in the host system browser.
pub(crate) fn open_in_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn()
            .map_err(|e| anyhow!("Failed to open browser: {}", e))?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(url).spawn()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_headers(pairs: &[(&str, &str)]) -> std::collections::HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn test_detect_x_api_key() {
        let h = make_headers(&[("x-api-key", "my-account")]);
        let ah = detect_auth_header(&h).unwrap();
        assert_eq!(ah.name, "x-api-key");
        assert_eq!(ah.prefix, "");
        assert_eq!(ah.account, "my-account");
    }

    #[test]
    fn test_detect_x_goog_api_key() {
        let h = make_headers(&[("x-goog-api-key", "gemini-account")]);
        let ah = detect_auth_header(&h).unwrap();
        assert_eq!(ah.name, "x-goog-api-key");
        assert_eq!(ah.prefix, "");
        assert_eq!(ah.account, "gemini-account");
    }

    #[test]
    fn test_detect_bearer() {
        let h = make_headers(&[("authorization", "Bearer my-token")]);
        let ah = detect_auth_header(&h).unwrap();
        assert_eq!(ah.name, "authorization");
        assert_eq!(ah.prefix, "Bearer ");
        assert_eq!(ah.account, "my-token");
    }

    #[test]
    fn test_detect_priority_x_api_key_over_x_goog() {
        // When both present, x-api-key wins (checked first)
        let h = make_headers(&[("x-api-key", "acct-a"), ("x-goog-api-key", "acct-b")]);
        let ah = detect_auth_header(&h).unwrap();
        assert_eq!(ah.name, "x-api-key");
    }

    #[test]
    fn test_detect_none() {
        let h = make_headers(&[("content-type", "application/json")]);
        assert!(detect_auth_header(&h).is_none());
    }

    #[test]
    fn test_is_loopback_host() {
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("127.1.2.3"));
        assert!(is_loopback_host("127.255.255.254"));
        assert!(is_loopback_host("::1"));
        assert!(is_loopback_host("localhost"));
        assert!(is_loopback_host("LocalHost"));
        assert!(is_loopback_host("app.localhost"));
        assert!(is_loopback_host("a.b.localhost"));

        assert!(!is_loopback_host("128.0.0.1"));
        assert!(!is_loopback_host("10.0.0.1"));
        assert!(!is_loopback_host("example.com"));
        assert!(!is_loopback_host("localhost.evil.com"));
        assert!(!is_loopback_host("::2"));
    }

    #[test]
    fn test_extract_redirect_port_loopback_variants() {
        let base = "https://github.com/login/oauth/authorize?response_type=code&state=x";
        let mk = |redir: &str| {
            let encoded: String = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("redirect_uri", redir)
                .finish();
            format!("{}&{}", base, encoded)
        };

        assert_eq!(extract_redirect_port(&mk("http://127.0.0.1:8976/cb")), Some(8976));
        assert_eq!(extract_redirect_port(&mk("http://127.5.6.7:9000/cb")), Some(9000));
        assert_eq!(extract_redirect_port(&mk("http://localhost:5000/cb")), Some(5000));
        assert_eq!(extract_redirect_port(&mk("http://app.localhost:5000/cb")), Some(5000));
        assert_eq!(extract_redirect_port(&mk("http://[::1]:7000/cb")), Some(7000));

        assert_eq!(extract_redirect_port(&mk("http://attacker.com:8976/cb")), None);
        assert_eq!(extract_redirect_port(&mk("http://localhost.evil.com:8976/cb")), None);
        assert_eq!(extract_redirect_port(&mk("http://10.0.0.1:8976/cb")), None);
    }
}
