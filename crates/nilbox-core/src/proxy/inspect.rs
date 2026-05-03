//! TLS inspection proxy for HTTPS CONNECT interception.
//!
//! For whitelisted domains the proxy terminates TLS on the host side,
//! reads the plaintext HTTP request, performs auth-header token exchange
//! via `ApiKeyGate`, then forwards to the real upstream via `reqwest`.

use crate::config_store::ConfigStore;
use crate::events::{EventEmitter, emit_typed};
use crate::gateway::Gateway;
use crate::keystore::KeyStore;
use crate::monitoring::MonitoringCollector;
use crate::proxy::auth_delegator::AuthDelegator;
use crate::proxy::auth_delegator::aws_sigv4::AwsSigV4Delegator;
use crate::proxy::auth_delegator::telegram::TelegramDelegator;
use crate::proxy::auth_router::AuthRouter;
use crate::proxy::domain_gate::DomainGate;
use crate::proxy::token_mismatch_gate::TokenMismatchGate;
use crate::proxy::request_parser::parse_request_headers;
use crate::proxy::reverse_proxy::detect_auth_header;
use crate::proxy::llm_detector::LlmProviderMatcher;
use crate::proxy::oauth_script_engine::{OAuthScriptEngine, resolve_json_path_with_fallback};
use crate::proxy::oauth_token_vault::OAuthTokenVault;
use crate::proxy::token_extractor::{TokenUsageData, extract_from_body, extract_from_sse_chunks, estimate_from_bytes, estimate_from_bytes_fallback};
use crate::proxy::token_limit::TokenLimitChecker;
use crate::token_monitor::TokenUsageLogger;
use crate::vsock::async_adapter::wrap_virtual_stream;
use crate::vsock::stream::VirtualStream;
use crate::vsock::VsockStream;

use anyhow::{Result, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rcgen::{CertificateParams, KeyPair, IsCa, BasicConstraints};
use reqwest::Client;
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use std::collections::HashMap;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;
use tokio_rustls::TlsAcceptor;
use tracing::{debug, warn, error};

/// Bundles the CA and per-domain cert cache for TLS inspection.
pub struct InspectCertAuthority {
    ca_key: KeyPair,
    /// Used only for signing leaf certs (same public key as the stored CA).
    ca_cert: rcgen::Certificate,
    /// The original CA cert DER — sent in the TLS chain to the VM.
    ca_cert_der: CertificateDer<'static>,
    /// The original CA cert PEM — injected into the VM trust store.
    ca_cert_pem_str: String,
    cache: Mutex<HashMap<String, Arc<ServerConfig>>>,
}

impl InspectCertAuthority {
    /// Load a persisted CA from keystore, or generate a new one and save it.
    ///
    /// The same CA is reused across VM restarts so the VM trust store does not
    /// need to be updated every time the host process restarts.
    pub async fn load_or_create(keystore: &Arc<dyn crate::keystore::KeyStore>) -> Result<Self> {
        const STORE_KEY: &str = "nilbox:inspect_ca_v1";

        if let Ok(json) = keystore.get(STORE_KEY).await {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) {
                let key_pem  = v["key_pem"].as_str().unwrap_or("");
                let cert_pem = v["cert_pem"].as_str().unwrap_or("");
                if !key_pem.is_empty() && !cert_pem.is_empty() {
                    match Self::from_stored(key_pem, cert_pem) {
                        Ok(ca) => {
                            debug!("Inspect CA loaded from keystore (reusing existing cert)");
                            return Ok(ca);
                        }
                        Err(e) => warn!("Inspect CA restore failed: {} — generating new", e),
                    }
                }
            }
        }

        let ca = Self::generate()?;
        let blob = serde_json::json!({
            "key_pem":  ca.ca_key.serialize_pem(),
            "cert_pem": ca.ca_cert_pem_str,
        });
        match keystore.set(STORE_KEY, &blob.to_string()).await {
            Ok(_)  => debug!("New Inspect CA generated and saved to keystore"),
            Err(e) => warn!("Failed to save Inspect CA to keystore: {}", e),
        }
        Ok(ca)
    }

    /// Generate a brand-new self-signed root CA.
    fn generate() -> Result<Self> {
        let ca_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
            .map_err(|e| anyhow!("CA key gen: {}", e))?;

        let ca_cert = Self::build_ca_cert_params()
            .self_signed(&ca_key)
            .map_err(|e| anyhow!("CA self-sign: {}", e))?;

        let ca_cert_pem_str = ca_cert.pem();
        let ca_cert_der = CertificateDer::from(ca_cert.der().to_vec());

        Ok(Self { ca_key, ca_cert, ca_cert_der, ca_cert_pem_str, cache: Mutex::new(HashMap::new()) })
    }

    /// Restore from stored key PEM + cert PEM.
    fn from_stored(key_pem: &str, cert_pem: &str) -> Result<Self> {
        let ca_key = KeyPair::from_pem(key_pem)
            .map_err(|e| anyhow!("CA key restore: {}", e))?;

        // Reconstruct an rcgen::Certificate for signing leaf certs.
        // It uses the same private key, so leaf certs it signs are valid against
        // the original CA cert that is in the VM's trust store.
        let ca_cert = Self::build_ca_cert_params()
            .self_signed(&ca_key)
            .map_err(|e| anyhow!("CA cert reconstruct: {}", e))?;

        // Parse the original DER from the stored PEM — this is what we send in the TLS chain.
        let ca_cert_der = Self::pem_to_der(cert_pem)?;
        let ca_cert_pem_str = cert_pem.to_string();

        Ok(Self { ca_key, ca_cert, ca_cert_der, ca_cert_pem_str, cache: Mutex::new(HashMap::new()) })
    }

    fn build_ca_cert_params() -> CertificateParams {
        let mut params = CertificateParams::new(Vec::<String>::new())
            .expect("empty SAN list is valid");
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.distinguished_name.push(
            rcgen::DnType::CommonName,
            rcgen::DnValue::Utf8String("NilBox Inspect CA".into()),
        );
        params.key_usages = vec![
            rcgen::KeyUsagePurpose::KeyCertSign,
            rcgen::KeyUsagePurpose::CrlSign,
        ];
        params
    }

    fn pem_to_der(pem: &str) -> Result<CertificateDer<'static>> {
        let b64: String = pem.lines()
            .filter(|l| !l.starts_with("-----"))
            .collect();
        let der = STANDARD.decode(b64.trim())
            .map_err(|e| anyhow!("PEM base64 decode: {}", e))?;
        Ok(CertificateDer::from(der))
    }

    /// Return the CA certificate in PEM format (for VM trust-store injection).
    pub fn ca_cert_pem(&self) -> String {
        self.ca_cert_pem_str.clone()
    }

    /// Get (or create+cache) a TLS acceptor for `domain`.
    pub async fn get_tls_acceptor(&self, domain: &str) -> Result<TlsAcceptor> {
        // Fast path: check cache without holding mutex during cert generation
        {
            let cache = self.cache.lock().await;
            if let Some(cfg) = cache.get(domain) {
                return Ok(TlsAcceptor::from(cfg.clone()));
            }
        }

        // Slow path: generate cert outside of mutex using block_in_place
        // to avoid blocking the tokio executor (which would delay "200" delivery to VM)
        let server_config = tokio::task::block_in_place(|| self.generate_server_config(domain))?;
        let arc = Arc::new(server_config);
        let mut cache = self.cache.lock().await;
        // Use entry to handle concurrent cert generation for same domain
        let arc = cache.entry(domain.to_string()).or_insert(arc).clone();
        Ok(TlsAcceptor::from(arc))
    }

    fn generate_server_config(&self, domain: &str) -> Result<ServerConfig> {
        let leaf_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
            .map_err(|e| anyhow!("leaf key gen: {}", e))?;

        let mut params = CertificateParams::new(vec![domain.to_string()])
            .map_err(|e| anyhow!("leaf params: {}", e))?;
        params.distinguished_name.push(
            rcgen::DnType::CommonName,
            rcgen::DnValue::Utf8String(domain.into()),
        );
        params.use_authority_key_identifier_extension = true;
        params.key_usages = vec![rcgen::KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];

        let leaf_cert = params
            .signed_by(&leaf_key, &self.ca_cert, &self.ca_key)
            .map_err(|e| anyhow!("leaf sign: {}", e))?;

        let leaf_cert_der = CertificateDer::from(leaf_cert.der().to_vec());
        let leaf_key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            leaf_key.serialize_der(),
        ));

        let mut cfg = ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|e| anyhow!("TLS protocol versions: {}", e))?
            .with_no_client_auth()
            .with_single_cert(
                vec![leaf_cert_der, self.ca_cert_der.clone()],
                leaf_key_der,
            )
            .map_err(|e| anyhow!("ServerConfig: {}", e))?;

        // The proxy parses the incoming request as HTTP/1.1, so advertise only
        // http/1.1 in ALPN. Clients that strictly require ALPN negotiation
        // (some HTTP/2-only libraries) close the TLS handshake without an alert
        // when the server doesn't echo back a protocol they support.
        cfg.alpn_protocols = vec![b"http/1.1".to_vec()];

        Ok(cfg)
    }
}

/// Context passed to the CONNECT handler for whitelisted domains.
#[derive(Clone)]
pub struct InspectContext {
    pub ca: Arc<InspectCertAuthority>,
    pub auth_router: Arc<AuthRouter>,
    pub config_store: Arc<ConfigStore>,
    pub keystore: Arc<dyn KeyStore>,
    pub emitter: Arc<dyn EventEmitter>,
    pub token_mismatch_gate: Arc<TokenMismatchGate>,
    pub client: Client,
    pub gate: Option<Arc<DomainGate>>,
    pub vm_id: String,
    pub oauth_engine: Arc<tokio::sync::RwLock<Arc<OAuthScriptEngine>>>,
    pub oauth_vault: Arc<OAuthTokenVault>,
    pub llm_matcher: Arc<LlmProviderMatcher>,
    pub token_logger: Arc<TokenUsageLogger>,
    pub token_limit_checker: Arc<TokenLimitChecker>,
    pub monitoring: Arc<MonitoringCollector>,
    pub gateway: Arc<Gateway>,
}

/// Handle an inspect CONNECT: terminate TLS, read plaintext HTTP, do token exchange, forward upstream.
pub async fn handle_inspect_connect(
    mut stream: VirtualStream,
    domain: String,
    _port: u16,
    ctx: InspectContext,
) -> Result<()> {
    // 1. Send "200 Connection Established" back to the VM
    stream
        .write(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await?;

    // 2. Bridge VirtualStream → DuplexStream for tokio-rustls
    let duplex = wrap_virtual_stream(stream);

    // 3. TLS accept (proxy acts as the server to the VM)
    let tls_acceptor = ctx.ca.get_tls_acceptor(&domain).await?;
    let mut tls_stream = tls_acceptor.accept(duplex).await
        .map_err(|e| anyhow!("TLS accept for {}: {}", domain, e))?;

    // 4. Read plaintext HTTP request from TLS stream
    let mut buf = vec![0u8; 65536];
    let n = tls_stream.read(&mut buf).await
        .map_err(|e| anyhow!("TLS read: {}", e))?;
    if n == 0 {
        return Ok(());
    }
    let payload = &buf[..n];

    // 5. Parse HTTP headers
    let parsed_req = parse_request_headers(payload)?;
    let auth_info = detect_auth_header(&parsed_req.headers);

    // Build upstream URL
    let host_header = parsed_req.headers.get("host")
        .map(|h| h.as_str())
        .unwrap_or(&domain);
    let url = format!("https://{}{}",
        host_header,
        if parsed_req.path.starts_with('/') { &parsed_req.path } else { "/" },
    );
    let method: reqwest::Method = parsed_req.method.parse().unwrap_or(reqwest::Method::GET);

    let client = ctx.client.clone();
    let mut request_builder = client.request(method, &url);

    // Copy all headers (auth header included — delegator will replace it)
    for (k, v) in &parsed_req.headers {
        if k == "host" || k == "content-length" { continue; }
        request_builder = request_builder.header(k, v);
    }

    // Collect request body
    let content_length: usize = parsed_req.headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    let mut body = Vec::new();
    if parsed_req.body_offset < payload.len() {
        body.extend_from_slice(&payload[parsed_req.body_offset..]);
    }
    // Read remaining body if needed
    while body.len() < content_length {
        let mut tmp = vec![0u8; 8192];
        match tls_stream.read(&mut tmp).await {
            Ok(0) => break,
            Ok(n) => body.extend_from_slice(&tmp[..n]),
            Err(_) => break,
        }
    }
    let request_body_len = body.len();
    if !body.is_empty() {
        request_builder = request_builder.body(body);
    }


    // 6. Auth delegation via AuthRouter
    let mut request = request_builder.build()
        .map_err(|e| anyhow!("Failed to build request: {}", e))?;

    let (delegator, credential_account) = ctx.auth_router.resolve(&domain).await;
    let (active_delegator, account): (Arc<dyn AuthDelegator>, Option<String>) =
        if let Some(acct) = credential_account {
            // 1. Explicit auth_route mapping takes priority
            (delegator, Some(acct))
        } else {
            let mut tokens = ctx.config_store
                .list_domain_tokens(&domain)
                .unwrap_or_default();
            // Merge in-memory pending tokens from allow_once decisions
            if let Some(ref g) = ctx.gate {
                let pending = g.take_pending_tokens(&domain).await;
                for t in pending {
                    if !tokens.contains(&t) {
                        tokens.push(t);
                    }
                }
            }
            // debug!("inspect: domain={} merged tokens={:?}", domain, tokens);
            if !tokens.is_empty() {
                // 2. domain_token_accounts present → auto-detect engine
                let is_aws = tokens.iter().any(|t| t == "AWS_ACCESS_KEY_ID")
                    && tokens.iter().any(|t| t == "AWS_SECRET_ACCESS_KEY");
                if is_aws {
                    // Pass merged token names so allow_once works without DB lookup
                    let aws_del: Arc<dyn AuthDelegator> = Arc::new(
                        AwsSigV4Delegator::with_token_names(ctx.keystore.clone(), ctx.config_store.clone(), tokens.clone())
                    );
                    (aws_del, Some(domain.clone()))
                } else if tokens.iter().any(|t| t == "TELEGRAM_BOT_TOKEN") {
                    let tg_del: Arc<dyn AuthDelegator> = Arc::new(
                        TelegramDelegator::new(ctx.keystore.clone())
                    );
                    (tg_del, Some("TELEGRAM_BOT_TOKEN".to_string()))
                } else if let Some(ref ah) = auth_info {
                    // Bearer: only substitute if the request's account matches a mapped token
                    if tokens.contains(&ah.account) {
                        (delegator, Some(ah.account.clone()))
                    } else {
                        // Token mismatch: request uses different account than mapped tokens
                        // Prompt user: pass through or block?
                        let pass_through = ctx.token_mismatch_gate
                            .check(&domain, &ah.account, &tokens)
                            .await;
                        if pass_through {
                            // User chose to send anyway — pass through unchanged
                            (delegator, None)
                        } else {
                            // User chose to cancel — block the request
                            let _ = tls_stream.write_all(b"HTTP/1.1 403 Forbidden\r\nConnection: close\r\n\r\n").await;
                            let _ = tls_stream.shutdown().await;
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
                    match ctx.oauth_vault.check_and_resolve(&ah.account, &domain).await {
                        Ok(OAuthDomainCheck::FirstUse { .. }) => {
                            let _ = ctx.oauth_vault.bind_domain(&ah.account, &domain).await;
                            debug!("inspect: bound OAuth token to domain={}", domain);
                        }
                        Ok(OAuthDomainCheck::Match { .. }) => {
                            debug!("inspect: OAuth token domain match for domain={}", domain);
                        }
                        Ok(OAuthDomainCheck::Mismatch { ref bound_domain }) => {
                            warn!("inspect: OAuth token domain mismatch: bound={} request={}", bound_domain, domain);
                            emit_typed(
                                &ctx.emitter,
                                "oauth-domain-mismatch",
                                &serde_json::json!({
                                    "domain": domain,
                                    "bound_domain": bound_domain,
                                    "vm_id": ctx.vm_id,
                                }),
                            );
                            let _ = tls_stream.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                            let _ = tls_stream.shutdown().await;
                            return Ok(());
                        }
                        Ok(OAuthDomainCheck::NotFound) | Err(_) => {
                            emit_typed(
                                &ctx.emitter,
                                "domain-env-missing",
                                &serde_json::json!({
                                    "domain": domain,
                                    "account": ah.account,
                                }),
                            );
                        }
                    }
                    (delegator, None)
                } else {
                    // 3. Domain in allowlist but no tokens mapped — notify user to configure
                    emit_typed(
                        &ctx.emitter,
                        "domain-env-missing",
                        &serde_json::json!({
                            "domain": domain,
                            "account": ah.account,
                        }),
                    );
                    (delegator, None)
                }
            } else {
                (delegator, None)
            }
        };

    // debug!("inspect: domain={} delegator={} account={:?}", domain, active_delegator.kind(), account);

    // Resolve OAuth dummy access tokens (nilbox_oat:) regardless of domain mapping
    if account.is_none() {
        if let Some(existing) = request.headers().get("authorization").cloned() {
            let val = existing.to_str().unwrap_or("");
            let bearer_value = val.strip_prefix("Bearer ")
                .or_else(|| val.strip_prefix("token "));
            if let Some(token) = bearer_value {
                if OAuthTokenVault::is_dummy_access_token(token) {
                    match ctx.oauth_vault.resolve_access_token(token).await {
                        Ok(Some(real)) => {
                            let prefix = if val.starts_with("Bearer ") { "Bearer " } else { "token " };
                            if let Ok(hv) = format!("{}{}", prefix, real).parse() {
                                request.headers_mut().insert("authorization", hv);
                                debug!("inspect: resolved dummy OAuth access token for domain={}", domain);
                            }
                        }
                        Ok(None) => warn!("inspect: dummy OAuth access token not found in vault"),
                        Err(e) => warn!("inspect: failed to resolve OAuth access token: {}", e),
                    }
                }
            }
        }
    }

    if let Some(ref account) = account {
        if let Err(e) = active_delegator.apply_auth(&mut request, &domain, account).await {
            warn!("Auth delegation failed for {}: {}", domain, e);
            if let Some(ref ah) = auth_info {
                request.headers_mut().remove(&ah.name);
            }
        }
    }

    // --- Token limit pre-check (block before forwarding) ---
    let llm_match = ctx.llm_matcher.match_request(&domain, &parsed_req.path, &[]);
    let provider_id_for_check = llm_match.as_ref()
        .map(|lm| lm.provider_id.clone())
        .or_else(|| ctx.llm_matcher.match_domain_only(&domain));

    if let Some(ref provider_id) = provider_id_for_check {
        for pid in [provider_id.as_str(), "*"] {
            match ctx.token_limit_checker.check_pre_request(&ctx.vm_id, pid) {
                Ok(Some(ref r)) if r.action == "block" => {
                    let log_pid = provider_id_for_check.as_deref().unwrap_or(pid);
                    let blocked_usage = TokenUsageData {
                        request_tokens: 0, response_tokens: 0, total_tokens: 0,
                        model: None, confidence: "blocked".to_string(),
                    };
                    let _ = ctx.token_logger.log(
                        &ctx.vm_id, log_pid, &blocked_usage,
                        Some(&parsed_req.path), Some(429), false,
                    );
                    let body = b"{\"error\":\"token limit exceeded\"}";
                    let resp = format!(
                        "HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = tls_stream.write_all(resp.as_bytes()).await;
                    let _ = tls_stream.write_all(body).await;
                    let _ = tls_stream.shutdown().await;
                    return Ok(());
                }
                Err(e) => warn!("inspect pre-request check failed for {}: {}", pid, e),
                _ => {}
            }
        }
    }

    // OAuth token request: detect NILBOX_OAUTH_ placeholders in POST body and substitute
    // with real credentials. This works regardless of URL path matching.
    let mut oauth_matched_provider: Option<String> = None;
    let mut oauth_old_refresh_dummy: Option<String> = None;

    // If active_delegator is scripted-oauth and it already ran successfully (substituting
    // placeholders in the body), the subsequent NILBOX_OAUTH_ scan below will not find
    // anything. Pre-populate oauth_matched_provider from the credential account so the
    // response interceptor downstream still matches and stores the session.
    // Also resolve any dummy refresh_token in the request body to the real value,
    // since scripted_oauth::apply_auth only substitutes client_id/client_secret.
    if active_delegator.kind() == "scripted-oauth" {
        if let Some(ref acct) = account {
            let engine = ctx.oauth_engine.read().await.clone();
            let matched = engine.providers()
                .find(|p| p.info.token_path == *acct)
                .or_else(|| engine.providers().find(|p| {
                    let prefix = format!("OAUTH_{}_", p.info.name.to_uppercase());
                    acct.starts_with(&prefix)
                }));
            if let Some(provider) = matched {
                debug!(
                    "inspect: pre-matched scripted-oauth provider={} via account={}",
                    provider.info.name, acct
                );
                oauth_matched_provider = Some(provider.info.name.clone());
            }
        }

        // Resolve dummy refresh_token in request body (scripted-oauth leaves it untouched).
        if parsed_req.method == "POST" {
            let body_bytes_opt = request.body()
                .and_then(|b| b.as_bytes())
                .map(|b| b.to_vec());
            if let Some(body_bytes) = body_bytes_opt {
                let params: Vec<(String, String)> = url::form_urlencoded::parse(&body_bytes)
                    .map(|(k, v)| (k.into_owned(), v.into_owned()))
                    .collect();
                let mut needs_rewrite = false;
                let mut new_params: Vec<(String, String)> = Vec::with_capacity(params.len());
                for (k, v) in params {
                    if k == "refresh_token" && OAuthTokenVault::is_dummy_refresh_token(&v) {
                        oauth_old_refresh_dummy = Some(v.clone());
                        match ctx.oauth_vault.resolve_refresh_token(&v).await {
                            Ok(Some(real)) => {
                                debug!("inspect: resolved dummy refresh_token for scripted-oauth");
                                new_params.push((k, real));
                                needs_rewrite = true;
                                continue;
                            }
                            Ok(None) => {
                                warn!("inspect scripted-oauth: dummy refresh_token not found in vault");
                            }
                            Err(e) => {
                                warn!("inspect scripted-oauth: refresh_token resolve failed: {}", e);
                            }
                        }
                    }
                    new_params.push((k, v));
                }
                if needs_rewrite {
                    let new_body = url::form_urlencoded::Serializer::new(String::new())
                        .extend_pairs(&new_params)
                        .finish();
                    if let Ok(hv) = reqwest::header::HeaderValue::from_str(&new_body.len().to_string()) {
                        request.headers_mut().insert(reqwest::header::CONTENT_LENGTH, hv);
                    }
                    *request.body_mut() = Some(reqwest::Body::from(new_body));
                }
            }
        }
    }
    if parsed_req.method == "POST" {
        if let Some(body) = request.body() {
            if let Some(body_bytes) = body.as_bytes() {
                let body_str = String::from_utf8_lossy(body_bytes);
                if body_str.contains("NILBOX_OAUTH_") {
                    let engine = ctx.oauth_engine.read().await.clone();
                    // Find provider by matching placeholder_prefix in the body
                    let matched_provider = engine.providers()
                        .find(|p| body_str.contains(&p.info.placeholder_prefix));

                    if let Some(provider) = matched_provider {

                        if let Ok(extraction) = engine.call_placeholder_extraction_instructions(provider) {
                            if !extraction.is_empty() {
                                let oauth_key = format!("oauth:{}", provider.info.name);
                                let candidates = [provider.info.token_path.as_str(), oauth_key.as_str()];

                                'cred_search: for key in &candidates {
                                    if let Ok(secret_json) = ctx.keystore.get(key).await {
                                        let secret_doc: serde_json::Value = match serde_json::from_str(&secret_json) {
                                            Ok(v) => v,
                                            Err(_) => {
                                                match serde_json::from_str::<String>(&secret_json)
                                                    .ok()
                                                    .and_then(|s| serde_json::from_str(&s).ok())
                                                {
                                                    Some(v) => v,
                                                    None => continue,
                                                }
                                            }
                                        };

                                        let mut placeholders: HashMap<String, String> = HashMap::new();
                                        let mut all_found = true;
                                        for (name, path) in &extraction {
                                            match resolve_json_path_with_fallback(&secret_doc, path) {
                                                Some(value) => { placeholders.insert(name.clone(), value); }
                                                None => { all_found = false; break; }
                                            }
                                        }
                                        if !all_found { continue; }

                                        // Parse form body and substitute placeholder values + resolve dummy refresh tokens
                                        let mut resolved_refresh = false;
                                        let mut old_refresh_dummy: Option<String> = None;
                                        let mut params: Vec<(String, String)> = Vec::new();
                                        for (k, v) in url::form_urlencoded::parse(body_bytes) {
                                            let mut val = v.into_owned();
                                            for (ph_name, real_value) in &placeholders {
                                                let placeholder = format!("{}_{}", provider.info.placeholder_prefix, ph_name);
                                                if val == placeholder {
                                                    val = real_value.clone();
                                                }
                                            }
                                            // Also resolve dummy refresh tokens in the same pass
                                            if OAuthTokenVault::is_dummy_refresh_token(&val) {
                                                old_refresh_dummy = Some(val.clone());
                                                match ctx.oauth_vault.resolve_refresh_token(&val).await {
                                                    Ok(Some(real)) => {
                                                        debug!("inspect: resolved dummy refresh token for key={}", k);
                                                        val = real;
                                                        resolved_refresh = true;
                                                    }
                                                    Ok(None) => {
                                                        warn!("inspect: invalid dummy refresh token format");
                                                        val = String::new();
                                                    }
                                                    Err(e) => {
                                                        warn!("inspect: dummy refresh token resolution failed: {}", e);
                                                        val = String::new();
                                                    }
                                                }
                                            }
                                            params.push((k.into_owned(), val));
                                        }

                                        let new_body = url::form_urlencoded::Serializer::new(String::new())
                                            .extend_pairs(&params)
                                            .finish();

                                        debug!("inspect: substituted OAuth credentials in token request for provider={} (refresh_resolved={})", provider.info.name, resolved_refresh);
                                        oauth_matched_provider = Some(provider.info.name.clone());
                                        oauth_old_refresh_dummy = old_refresh_dummy;
                                        *request.body_mut() = Some(reqwest::Body::from(new_body));
                                        break 'cred_search;
                                    }
                                }
                            }
                        }
                    } else {
                        warn!("inspect: NILBOX_OAUTH_ found in body but no matching provider");
                    }
                }
            }
        }
    }

    // 6a-2. PKCE domain+path detection (when placeholder detection did not match)
    if oauth_matched_provider.is_none() && parsed_req.method == "POST" {
        let engine = ctx.oauth_engine.read().await.clone();
        if let Some(provider) = engine.find_by_token_domain_and_path(&domain, &parsed_req.path) {
            if provider.info.flow_type == "pkce" {
                // Check if this POST is actually a token exchange request
                let body_params: HashMap<String, String> = request.body()
                    .and_then(|b| b.as_bytes())
                    .map(|bytes| url::form_urlencoded::parse(bytes)
                        .map(|(k, v)| (k.into_owned(), v.into_owned()))
                        .collect())
                    .unwrap_or_default();

                if engine.call_is_token_exchange_request(provider, &body_params) {
                    // Resolve dummy refresh tokens in body
                    if let Some(body) = request.body() {
                        if let Some(body_bytes) = body.as_bytes() {
                            let params: Vec<(String, String)> = url::form_urlencoded::parse(body_bytes)
                                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                                .collect();

                            let mut needs_rewrite = false;
                            let mut old_refresh: Option<String> = None;
                            let mut new_params = Vec::new();

                            for (k, v) in &params {
                                let mut val = v.clone();
                                if OAuthTokenVault::is_dummy_refresh_token(&val) {
                                    old_refresh = Some(val.clone());
                                    match ctx.oauth_vault.resolve_refresh_token(&val).await {
                                        Ok(Some(real)) => {
                                            debug!("inspect PKCE: resolved dummy refresh token for key={}", k);
                                            val = real;
                                            needs_rewrite = true;
                                        }
                                        Ok(None) => {
                                            warn!("inspect PKCE: invalid dummy refresh token format");
                                            val = String::new();
                                        }
                                        Err(e) => {
                                            warn!("inspect PKCE: dummy refresh token resolution failed: {}", e);
                                            val = String::new();
                                        }
                                    }
                                }
                                new_params.push((k.clone(), val));
                            }

                            if needs_rewrite {
                                let new_body = url::form_urlencoded::Serializer::new(String::new())
                                    .extend_pairs(&new_params).finish();
                                if let Ok(hv) = reqwest::header::HeaderValue::from_str(&new_body.len().to_string()) {
                                    request.headers_mut().insert(reqwest::header::CONTENT_LENGTH, hv);
                                }
                                *request.body_mut() = Some(reqwest::Body::from(new_body));
                            }
                            oauth_old_refresh_dummy = old_refresh;
                        }
                    }
                    debug!("inspect PKCE: token exchange detected for provider={}", provider.info.name);
                    oauth_matched_provider = Some(provider.info.name.clone());
                }
            }
        }
    }

    // 6b. Cache optimization: if the access token is still valid, return cached
    //     response without hitting upstream.  Only applies to refresh_token grants.
    if let Some(ref dummy_rt) = oauth_old_refresh_dummy {
        if let Some(ref matched_name) = oauth_matched_provider {
            debug!(
                "inspect OAuth cache: checking provider={} dummy_refresh={}",
                matched_name, dummy_rt
            );
            let oauth_engine_snap = ctx.oauth_engine.read().await.clone();
            if let Some(provider) = oauth_engine_snap.get_provider(matched_name) {
                let fields = oauth_engine_snap.call_token_response_fields(provider);
                if let Some(cached_body) = ctx.oauth_vault.try_cached_token_response(dummy_rt, &fields).await {
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        cached_body.len()
                    );
                    let _ = tls_stream.write_all(resp.as_bytes()).await;
                    let _ = tls_stream.write_all(&cached_body).await;
                    let _ = tls_stream.shutdown().await;
                    return Ok(());
                } else {
                    debug!(
                        "inspect OAuth cache: miss for provider={}, forwarding upstream",
                        matched_name
                    );
                }
            } else {
                debug!(
                    "inspect OAuth cache: provider={} not found, skipping",
                    matched_name
                );
            }
        }
    }

    // 6c. OAuth authorize auto-detect: open host browser + register callback port mapping
    // Only trigger for real OAuth flows (must have `state` param — preflight checks omit it).
    if parsed_req.method == "GET" {
        if let Ok(parsed_url) = url::Url::parse(&url) {
            let path = parsed_url.path();
            let has_oauth_path = path.split('/').any(|seg| seg == "authorize" || seg == "auth");
            if has_oauth_path {
                let pairs: Vec<(String, String)> = parsed_url.query_pairs()
                    .map(|(k, v)| (k.into_owned(), v.into_owned()))
                    .collect();
                let has_response_type_code = pairs.iter().any(|(k, v)| k == "response_type" && v == "code");
                let has_state = pairs.iter().any(|(k, _)| k == "state");
                if has_response_type_code && has_state {
                    if let Some(callback_port) = crate::proxy::reverse_proxy::extract_redirect_port(&url) {
                        debug!("inspect: OAuth authorize detected, opening host browser for {}", domain);
                        // Register temporary port mapping for OAuth callback
                        match ctx.gateway.add_mapping(&ctx.vm_id, callback_port, callback_port).await {
                            Ok(_) => {
                                debug!("inspect: OAuth callback port mapping localhost:{} → VM:{} registered", callback_port, callback_port);
                                let gateway = ctx.gateway.clone();
                                tokio::spawn(async move {
                                    tokio::time::sleep(tokio::time::Duration::from_secs(300)).await;
                                    gateway.remove_mapping(callback_port).await;
                                    debug!("inspect: expired OAuth callback port mapping {}", callback_port);
                                });
                            }
                            Err(e) => warn!("inspect: failed to register OAuth callback port mapping: {}", e),
                        }
                        // Open URL in host browser (best-effort, don't block the request)
                        if let Err(e) = crate::proxy::reverse_proxy::open_in_browser(&url) {
                            warn!("inspect: failed to open browser for OAuth authorize: {}", e);
                        }
                    }
                } else if has_response_type_code && !has_state {
                    debug!("inspect: OAuth preflight detected (no state param), skipping browser open for {}", domain);
                    // Still register callback port mapping for the upcoming real request
                    if let Some(callback_port) = crate::proxy::reverse_proxy::extract_redirect_port(&url) {
                        match ctx.gateway.add_mapping(&ctx.vm_id, callback_port, callback_port).await {
                            Ok(_) => {
                                debug!("inspect: OAuth callback port mapping localhost:{} → VM:{} pre-registered", callback_port, callback_port);
                                let gateway = ctx.gateway.clone();
                                tokio::spawn(async move {
                                    tokio::time::sleep(tokio::time::Duration::from_secs(300)).await;
                                    gateway.remove_mapping(callback_port).await;
                                    debug!("inspect: expired OAuth callback port mapping {}", callback_port);
                                });
                            }
                            Err(e) => debug!("inspect: preflight OAuth port mapping skipped: {}", e),
                        }
                    }
                }
            }
        }
    }

    // Strip Accept-Encoding when we'll intercept the OAuth token response,
    // so upstream returns uncompressed JSON that we can parse directly.
    if oauth_matched_provider.is_some() {
        request.headers_mut().remove(reqwest::header::ACCEPT_ENCODING);
        debug!("inspect OAuth: stripped Accept-Encoding for token exchange interception");
    }

    // 7. Forward to real upstream
    match client.execute(request).await {
        Ok(response) => {
            let status = response.status();
            let status_code = status.as_u16() as i32;
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

            // Detect SSE streaming before consuming response body
            let response_content_type: Option<String> = response.headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let is_streaming = response_content_type.as_deref()
                .map(|ct| ct.contains("text/event-stream"))
                .unwrap_or(false);

            // Read full body
            let raw_body = response.bytes().await
                .map_err(|e| anyhow!("inspect upstream body: {}", e))?;

            // OAuth token response interception (inspect path)
            // Match by: (1) credential substitution done earlier, (2) token_path_pattern, (3) URL
            let oauth_engine_snapshot = ctx.oauth_engine.read().await.clone();
            let provider_by_cred = oauth_matched_provider.as_deref()
                .and_then(|name| oauth_engine_snapshot.get_provider(name));
            // Path-based fallback: only for placeholder providers.
            // PKCE providers must be confirmed in the request phase (via provider_by_cred)
            // to avoid intercepting non-token-exchange responses (e.g. HTML error pages).
            let provider_by_path = oauth_engine_snapshot.find_by_token_domain_and_path(&domain, &parsed_req.path)
                .or_else(|| oauth_engine_snapshot.find_by_token_path(&parsed_req.path))
                .or_else(|| oauth_engine_snapshot.find_by_token_path(&url))
                .filter(|p| p.info.flow_type != "pkce");
            let matched_provider_ref = provider_by_cred.or(provider_by_path);
            // debug!(
            //     "inspect OAuth response: domain={} path={} method={} status={} by_cred={} by_path={} matched={}",
            //     domain, parsed_req.path, parsed_req.method, status.as_u16(),
            //     provider_by_cred.map(|p| p.info.name.as_str()).unwrap_or("none"),
            //     provider_by_path.map(|p| format!("{}({})", p.info.name, p.info.flow_type)).unwrap_or_else(|| "none".into()),
            //     matched_provider_ref.map(|p| p.info.name.as_str()).unwrap_or("none"),
            // );
            let body = if status.as_u16() == 200
                && parsed_req.method == "POST"
                && matched_provider_ref.is_some()
            {
                let provider = matched_provider_ref.unwrap();
                let provider_id = &provider.info.name;
                let fields = oauth_engine_snapshot.call_token_response_fields(&provider);
                let content_type = response_content_type.as_deref().unwrap_or("(unknown)");

                // Decompress body if compressed (upstream may compress even for token endpoints)
                let content_encoding = head.lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("content-encoding:"))
                    .and_then(|l| l.split(':').nth(1))
                    .map(|v| v.trim().to_lowercase());
                let decompressed = match content_encoding.as_deref() {
                    Some("gzip") => {
                        use std::io::Read;
                        let mut decoder = flate2::read::GzDecoder::new(&raw_body[..]);
                        let mut buf = Vec::new();
                        match decoder.read_to_end(&mut buf) {
                            Ok(_) => {
                                debug!("inspect OAuth: gzip decompressed {} → {} bytes", raw_body.len(), buf.len());
                                Some(bytes::Bytes::from(buf))
                            }
                            Err(e) => { debug!("inspect OAuth: gzip decompress failed: {}", e); None }
                        }
                    }
                    Some("br") => {
                        use std::io::Read;
                        let mut decoder = brotli::Decompressor::new(&raw_body[..], 4096);
                        let mut buf = Vec::new();
                        match decoder.read_to_end(&mut buf) {
                            Ok(_) => {
                                debug!("inspect OAuth: brotli decompressed {} → {} bytes", raw_body.len(), buf.len());
                                Some(bytes::Bytes::from(buf))
                            }
                            Err(e) => { debug!("inspect OAuth: brotli decompress failed: {}", e); None }
                        }
                    }
                    _ => None,
                };
                let intercept_body = decompressed.as_ref().unwrap_or(&raw_body);
                let body_preview = String::from_utf8_lossy(&intercept_body[..intercept_body.len().min(200)]);

                // Skip interception for path-only matches when body is clearly not a token response
                // (e.g. SSE streams, HTML pages). Token responses are JSON or form-urlencoded.
                let is_path_only_match = provider_by_cred.is_none();
                let looks_like_token_response = intercept_body.first() == Some(&b'{')
                    || content_type.contains("json")
                    || content_type.contains("form-urlencoded");

                if is_path_only_match && !looks_like_token_response {
                    debug!(
                        "inspect OAuth: skipping interception, path-only match but body not JSON/form (provider={} content_type={} body_len={})",
                        provider_id, content_type, intercept_body.len(),
                    );
                    raw_body
                } else {
                    // For refresh: reuse existing session UUID instead of creating new one
                    let existing_uuid = oauth_old_refresh_dummy.as_deref()
                        .and_then(|d| super::oauth_token_vault::parse_dummy_prefix(d))
                        .map(|(_, uuid)| uuid.to_string());
                    let cross_domains = provider.info.cross_domains.clone();
                    match ctx.oauth_vault.intercept_token_response(
                        provider_id, &ctx.vm_id, intercept_body, &fields,
                        existing_uuid.as_deref(),
                        cross_domains,
                    ).await {
                        Ok(modified) => {
                            let mod_preview = String::from_utf8_lossy(&modified[..modified.len().min(300)]);
                            emit_typed(
                                &ctx.emitter,
                                "oauth-session-updated",
                                &serde_json::json!({
                                    "vm_id": ctx.vm_id,
                                    "provider_id": provider_id,
                                }),
                            );
                            // Response is now uncompressed JSON; strip content-encoding from head
                            if decompressed.is_some() {
                                let filtered_head: String = head.lines()
                                    .filter(|l| !l.to_ascii_lowercase().starts_with("content-encoding:"))
                                    .collect::<Vec<_>>().join("\r\n") + "\r\n";
                                head = filtered_head;
                            }
                            bytes::Bytes::from(modified)
                        }
                        Err(e) => {
                            warn!(
                                "inspect: OAuth token interception failed, passing through: {} (provider={} flow={} content_type={} body_len={})",
                                e, provider_id, provider.info.flow_type, content_type, intercept_body.len(),
                            );
                            raw_body  // pass original (possibly compressed) body through
                        }
                    }
                }
            } else {
                raw_body
            };

            // Token extraction and logging (inspect path)
            if let Some(ref lm) = llm_match {
                if let Some(ref provider_info) = lm.provider_info {
                    // Provider configured: accurate token extraction from API response.
                    // Detect SSE from body content when Content-Type header is missing/wrong
                    // (chatgpt.com returns SSE without Content-Type: text/event-stream).
                    let body_is_sse = !is_streaming && (
                        body.starts_with(b"event: ") || body.starts_with(b"data: ")
                    );
                    let usage = if is_streaming || body_is_sse {
                        extract_from_sse_chunks(&[body.to_vec()], provider_info)
                            .unwrap_or_else(|| estimate_from_bytes_fallback(body.len()))
                    } else {
                        extract_from_body(&body, provider_info)
                            .unwrap_or_else(|| estimate_from_bytes_fallback(body.len()))
                    };
                    if let Err(e) = ctx.token_logger.log(
                        &ctx.vm_id, &lm.provider_id, &usage,
                        Some(&parsed_req.path), Some(status_code), false,
                    ) {
                        warn!("inspect token logging failed: {}", e);
                    }
                    if let Err(e) = ctx.token_limit_checker.check_soft_warnings(&ctx.vm_id, &lm.provider_id) {
                        warn!("inspect soft-warning check failed: {}", e);
                    }
                    if let Err(e) = ctx.token_limit_checker.check_soft_warnings(&ctx.vm_id, "*") {
                        warn!("inspect wildcard soft-warning check failed: {}", e);
                    }
                } else {
                    // Heuristic match (no provider config): byte estimation
                    let pid = ctx.llm_matcher.match_domain_only(&domain).unwrap_or_else(|| lm.provider_id.clone());
                    let usage = estimate_from_bytes(request_body_len, body.len());
                    if let Err(e) = ctx.token_logger.log(
                        &ctx.vm_id, &pid, &usage,
                        Some(&parsed_req.path), Some(status_code), false,
                    ) {
                        warn!("inspect token logging failed: {}", e);
                    }
                    if let Err(e) = ctx.token_limit_checker.check_soft_warnings(&ctx.vm_id, &pid) {
                        warn!("inspect soft-warning check failed: {}", e);
                    }
                    if let Err(e) = ctx.token_limit_checker.check_soft_warnings(&ctx.vm_id, "*") {
                        warn!("inspect wildcard soft-warning check failed: {}", e);
                    }
                }
            } else if let Some(provider_id) = ctx.llm_matcher.match_domain_only(&domain) {
                // No provider configured, but known LLM domain: byte estimation
                let usage = estimate_from_bytes(request_body_len, body.len());
                if let Err(e) = ctx.token_logger.log(
                    &ctx.vm_id, &provider_id, &usage,
                    Some(&parsed_req.path), Some(status_code), false,
                ) {
                    warn!("inspect token logging failed: {}", e);
                }
                if let Err(e) = ctx.token_limit_checker.check_soft_warnings(&ctx.vm_id, &provider_id) {
                    warn!("inspect soft-warning check failed: {}", e);
                }
                if let Err(e) = ctx.token_limit_checker.check_soft_warnings(&ctx.vm_id, "*") {
                    warn!("inspect wildcard soft-warning check failed: {}", e);
                }
            }

            // Set Content-Length from actual body size
            head.push_str(&format!("Content-Length: {}\r\n", body.len()));
            head.push_str("Connection: close\r\n\r\n");

            tls_stream.write_all(head.as_bytes()).await
                .map_err(|e| anyhow!("TLS write head: {}", e))?;
            tls_stream.write_all(&body).await
                .map_err(|e| anyhow!("inspect TLS write body: {}", e))?;
            tls_stream.flush().await
                .map_err(|e| anyhow!("inspect TLS flush: {}", e))?;
            // Track proxy bytes with domain info for StatusBar display
            ctx.monitoring.record_proxy_activity(
                &domain,
                (head.len() + body.len()) as u64,
                n as u64,
            );
        }
        Err(e) => {
            error!("inspect upstream request failed: {}", e);
            let _ = tls_stream.write_all(b"HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\n\r\n").await;
        }
    }

    let _ = tls_stream.shutdown().await;
    Ok(())
}
