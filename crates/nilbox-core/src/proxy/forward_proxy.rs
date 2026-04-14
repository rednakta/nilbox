//! Forward proxy (CONNECT tunnel)

use crate::config_store::ConfigStore;
use crate::events::EventEmitter;
use crate::keystore::KeyStore;
use crate::monitoring::MonitoringCollector;
use crate::vsock::stream::VirtualStream;
use crate::vsock::VsockStream;
use crate::proxy::request_parser::ParsedRequest;
use crate::proxy::domain_gate::DomainGate;
use crate::proxy::token_mismatch_gate::TokenMismatchGate;
use crate::proxy::auth_router::AuthRouter;
use crate::proxy::inspect::{InspectCertAuthority, InspectContext, handle_inspect_connect};
use crate::proxy::llm_detector::LlmProviderMatcher;
use crate::proxy::oauth_script_engine::OAuthScriptEngine;
use crate::proxy::oauth_token_vault::OAuthTokenVault;
use crate::proxy::token_limit::TokenLimitChecker;
use crate::token_monitor::TokenUsageLogger;
use crate::gateway::forwarder::forward_connection;
use anyhow::Result;
use reqwest::Client;
use std::sync::Arc;

use tokio::net::TcpStream;
use tracing::{error, trace};

pub async fn handle_connect(
    mut stream: VirtualStream,
    req: ParsedRequest,
    gate: Option<Arc<DomainGate>>,
    vm_id: &str,
    pre_allowed: bool,
    inspect_ca: Arc<InspectCertAuthority>,
    auth_router: Arc<AuthRouter>,
    config_store: Arc<ConfigStore>,
    keystore: Arc<dyn KeyStore>,
    emitter: Arc<dyn EventEmitter>,
    token_mismatch_gate: Arc<TokenMismatchGate>,
    client: Client,
    oauth_engine: Arc<tokio::sync::RwLock<Arc<OAuthScriptEngine>>>,
    oauth_vault: Arc<OAuthTokenVault>,
    llm_matcher: Arc<LlmProviderMatcher>,
    token_logger: Arc<TokenUsageLogger>,
    token_limit_checker: Arc<TokenLimitChecker>,
    monitoring: Arc<MonitoringCollector>,
    gateway: Arc<crate::gateway::Gateway>,
) -> Result<()> {
    let target = &req.path;
    // debug!("Forward Proxy CONNECT to {}", target);

    // Parse domain and port from "domain:port"
    let (domain, port) = {
        let mut parts = target.splitn(2, ':');
        let domain = parts.next().unwrap_or(target).to_string();
        let port: u16 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(443);
        (domain, port)
    };

    // Gate check (caller passes None for pre-allowed domains)
    if let Some(ref gate) = gate {
        if !gate.check(&domain, port, vm_id, "proxy").await {
            let _ = stream.write(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n").await;
            stream.close().await?;
            return Ok(());
        }
    }

    // Inspect decision: intercept when pre-allowed, pending allow_once tokens exist,
    // or domain is explicitly in the gate allowlist (includes cross-domain OAuth targets).
    let has_pending = if let Some(ref g) = gate {
        g.has_pending_tokens(&domain).await
    } else {
        false
    };
    let gate_allowed = if let Some(ref gate) = gate {
        gate.is_allowed(&domain).await
    } else {
        false
    };

    let do_inspect = !config_store.is_inspect_bypass_domain(&domain)
        && (pre_allowed || has_pending || gate_allowed);

    // debug!("forward_proxy: domain={} has_pending={} pre_allowed={} gate_allowed={} do_inspect={}",
    //     domain, has_pending, pre_allowed, gate_allowed, do_inspect);

    if do_inspect {
        trace!("[FWD-DBG] CONNECT stream={} → INSPECT for {}:{}", stream.stream_id, domain, port);
        let ctx = InspectContext {
            ca: inspect_ca, auth_router, config_store, keystore, emitter,
            token_mismatch_gate, client, gate: gate.clone(),
            vm_id: vm_id.to_string(),
            oauth_engine, oauth_vault,
            llm_matcher,
            token_logger,
            token_limit_checker,
            monitoring,
            gateway,
        };
        return handle_inspect_connect(stream, domain, port, ctx).await;
    }

    // Raw tunnel for non-whitelisted domains
    trace!("[FWD-DBG] CONNECT stream={} → raw tunnel for {}", stream.stream_id, target);
    match TcpStream::connect(target.as_str()).await {
        Ok(target_stream) => {
            if let Err(e) = stream.write(b"HTTP/1.1 200 Connection Established\r\n\r\n").await {
                error!("Failed to send 200 OK: {}", e);
                return Err(e);
            }
            forward_connection(target_stream, Box::new(stream)).await
        }
        Err(e) => {
            error!("Failed to connect to {}: {}", target, e);
            let _ = stream.write(b"HTTP/1.1 502 Bad Gateway\r\n\r\n").await;
            stream.close().await?;
            Ok(())
        }
    }
}
