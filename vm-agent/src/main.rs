mod vsock;
mod inbound;
mod outbound;
mod utils;
mod app_install;

#[cfg(all(target_os = "linux", feature = "with-fuse"))]
mod fuse;

use anyhow::{Result, anyhow};
use tracing::{debug, warn, error};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, filter::LevelFilter};
use vsock::{VsockConnector, VsockStream as VsockStreamTrait, stream::StreamMultiplexer};
use inbound::handler::handle_inbound_stream;
use outbound::proxy::OutboundProxy;
use outbound::dns::DnsForwarder;
use std::sync::Arc;
use std::path::Path;
use tokio::sync::{mpsc, RwLock};

#[cfg(target_os = "linux")]
use vsock::linux::LinuxVsockConnector;
#[cfg(target_os = "linux")]
use vsock::virtio_serial::VirtioSerialConnector;

#[cfg(not(target_os = "linux"))]
fn main() {
    println!("vm-agent only runs on Linux.");
}

#[cfg(target_os = "linux")]
const VSOCK_PORT: u32 = 1024;

/// Named virtio-serial port configured in QEMU
#[cfg(target_os = "linux")]
const VIRTIO_SERIAL_NAME: &str = "run.nilbox.vsock";

/// Detect virtio-serial device path. Checks named port first, then raw device.
#[cfg(target_os = "linux")]
fn detect_virtio_serial() -> Option<std::path::PathBuf> {
    let named = std::path::PathBuf::from(format!("/dev/virtio-ports/{}", VIRTIO_SERIAL_NAME));
    if named.exists() {
        return Some(named);
    }
    let raw = std::path::PathBuf::from("/dev/vport0p1");
    if raw.exists() {
        return Some(raw);
    }
    None
}

/// Create a dummy network interface with a default route.
/// Without a routable interface, the kernel rejects outbound connections with
/// "Network is unreachable" before iptables nat OUTPUT rules are evaluated.
/// The dummy interface makes all destinations routable so iptables REDIRECT works.
#[cfg(target_os = "linux")]
async fn setup_dummy_route() {
    use tokio::process::Command;

    // Load dummy module (ignore error — may be built-in)
    let modprobe = if std::path::Path::new("/sbin/modprobe").exists() {
        "/sbin/modprobe"
    } else {
        "modprobe"
    };
    let _ = Command::new(modprobe).arg("dummy").output().await;

    let ip = if std::path::Path::new("/sbin/ip").exists() {
        "/sbin/ip"
    } else {
        "ip"
    };

    let steps: &[&[&str]] = &[
        &["link", "add", "dummy0", "type", "dummy"],
        &["link", "set", "dummy0", "up"],
        &["addr", "add", "10.0.0.1/24", "dev", "dummy0"],
        &["route", "add", "default", "via", "10.0.0.1", "dev", "dummy0"],
    ];

    for args in steps {
        match Command::new(ip).args(*args).output().await {
            Ok(output) => {
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    // Ignore "already exists" errors on restart
                    if !stderr.contains("File exists") {
                        error!("ip {:?} failed: {}", args, stderr);
                    }
                }
            }
            Err(e) => {
                error!("Failed to run ip {:?}: {}", args, e);
            }
        }
    }

    debug!("Dummy network interface configured for transparent proxy routing");
}

/// Install iptables NAT rules so all outgoing TCP is transparently redirected
/// to the outbound proxy. This ensures apps that ignore http_proxy env vars
/// (e.g. Node.js fetch) still route through the VSOCK tunnel.
#[cfg(target_os = "linux")]
async fn setup_iptables() {
    use tokio::process::Command;

    // Detect which iptables binary is available.
    // Debian bookworm defaults to iptables-nft which needs nf_tables kernel module.
    // iptables-legacy uses the classic xtables interface (ip_tables module).
    // Use full paths since /usr/sbin may not be in systemd service PATH.
    let iptables = if std::path::Path::new("/usr/sbin/iptables-legacy").exists() {
        "/usr/sbin/iptables-legacy"
    } else if std::path::Path::new("/sbin/iptables-legacy").exists() {
        "/sbin/iptables-legacy"
    } else {
        "iptables"
    };
    debug!("Using {} for transparent proxy rules", iptables);

    // Load required kernel modules (ignore errors — may be built-in)
    let modprobe = if std::path::Path::new("/sbin/modprobe").exists() {
        "/sbin/modprobe"
    } else {
        "modprobe"
    };
    for module in &["ip_tables", "iptable_nat", "nf_nat", "nf_conntrack", "xt_REDIRECT"] {
        let _ = Command::new(modprobe).arg(module).output().await;
    }

    // Flush existing OUTPUT NAT rules to avoid duplicates on restart
    let _ = Command::new(iptables)
        .args(["-t", "nat", "-F", "OUTPUT"])
        .output()
        .await;

    let rules: &[&[&str]] = &[
        // Don't redirect traffic to localhost (avoids redirect loops)
        &["-t", "nat", "-A", "OUTPUT", "-p", "tcp", "-d", "127.0.0.0/8", "-j", "RETURN"],
        // Redirect all other outgoing TCP to the outbound proxy
        &["-t", "nat", "-A", "OUTPUT", "-p", "tcp", "-j", "REDIRECT", "--to-port", "18088"],
    ];

    let mut ok = true;
    for args in rules {
        match Command::new(iptables).args(*args).output().await {
            Ok(output) => {
                if !output.status.success() {
                    error!("{} {:?} failed: {}", iptables, args, String::from_utf8_lossy(&output.stderr));
                    ok = false;
                }
            }
            Err(e) => {
                error!("Failed to run {}: {}", iptables, e);
                ok = false;
            }
        }
    }

    if !ok {
        error!("iptables setup failed — transparent proxy will not work. Rebuild VM image to include netfilter modules.");
        return;
    }

    // Verify: list the OUTPUT chain
    match Command::new(iptables).args(["-t", "nat", "-L", "OUTPUT", "-n", "--line-numbers"]).output().await {
        Ok(output) => {
            let listing = String::from_utf8_lossy(&output.stdout);
            debug!("{} nat OUTPUT chain:\n{}", iptables, listing);
        }
        Err(e) => {
            error!("Failed to verify iptables rules: {}", e);
        }
    }
}

/// Parse a 64-character hex string into a 32-byte array.
#[cfg(target_os = "linux")]
fn parse_hex_token(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 {
        return None;
    }
    let mut token = [0u8; 32];
    for i in 0..32 {
        token[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(token)
}

/// Constant-time comparison to prevent timing side-channel attacks.
#[cfg(target_os = "linux")]
fn constant_time_eq(a: &[u8; 32], b: &[u8]) -> bool {
    if b.len() != 32 {
        return false;
    }
    let mut diff: u8 = 0;
    for i in 0..32 {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// Read the agent authentication token from the VM environment.
///
/// Tries two sources in order (fast path first):
/// 1. Kernel cmdline: `nilbox.auth_token=<hex>` — instant, used by Apple VZ (macOS host)
/// 2. QEMU fw_cfg: `/sys/firmware/qemu_fw_cfg/.../auth_token/raw` — used by QEMU (Linux/Windows host)
///
/// Returns error if token is not found — the agent cannot start without authentication.
#[cfg(target_os = "linux")]
fn read_auth_token() -> Result<[u8; 32]> {
    // 1st: kernel cmdline (instant — works for Apple Virtualization.framework)
    if let Ok(cmdline) = std::fs::read_to_string("/proc/cmdline") {
        for param in cmdline.split_whitespace() {
            if let Some(hex) = param.strip_prefix("nilbox.auth_token=") {
                if let Some(token) = parse_hex_token(hex) {
                    debug!("Auth token loaded from kernel cmdline");
                    return Ok(token);
                }
            }
        }
    }

    // 2nd: QEMU fw_cfg (with timeout to prevent hang if module not loaded)
    use std::time::Duration;
    use std::thread;
    use std::sync::{Arc, Mutex};

    let fw_cfg_path = "/sys/firmware/qemu_fw_cfg/by_name/opt/nilbox/auth_token/raw";
    let fw_cfg_result = Arc::new(Mutex::new(None));
    let fw_cfg_result_clone = fw_cfg_result.clone();
    let fw_cfg_path_clone = fw_cfg_path.to_string();

    let _fw_cfg_thread = thread::spawn(move || {
        if let Ok(data) = std::fs::read_to_string(&fw_cfg_path_clone) {
            *fw_cfg_result_clone.lock().unwrap() = Some(data);
        }
    });

    let fw_cfg_timeout = Duration::from_secs(2);
    let fw_cfg_start = std::time::Instant::now();
    while fw_cfg_start.elapsed() < fw_cfg_timeout {
        if fw_cfg_result.lock().unwrap().is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }

    if let Some(data) = fw_cfg_result.lock().unwrap().take() {
        if let Some(token) = parse_hex_token(data.trim()) {
            debug!("Auth token loaded from fw_cfg");
            return Ok(token);
        }
    }

    if fw_cfg_start.elapsed() >= fw_cfg_timeout {
        return Err(anyhow!("Auth token not found — cmdline empty, fw_cfg timed out (2s)"));
    }

    Err(anyhow!("Auth token not found in cmdline or fw_cfg"))
}

#[cfg(target_os = "linux")]
#[tokio::main]
async fn main() -> Result<()> {
    let file_appender = tracing_appender::rolling::never("/var/log", "vm-agent.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::registry()
        .with(LevelFilter::INFO)
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_subscriber::fmt::layer().with_writer(non_blocking))
        .init();

    debug!("Starting nilbox vm-agent v{} ({})",
        env!("CARGO_PKG_VERSION"),
        option_env!("NIL_GIT_SHA").unwrap_or("unknown"));

    // Read auth token — mandatory, agent cannot start without it
    let expected_token = read_auth_token()?;
    debug!("Agent auth token ready");

    let mux_store = Arc::new(RwLock::new(None));

    // Start Outbound Proxy
    let proxy = OutboundProxy::new(mux_store.clone());
    tokio::spawn(async move {
        if let Err(e) = proxy.run().await {
            error!("Outbound Proxy failed: {}", e);
        }
    });

    // Start DNS Forwarder
    let dns = DnsForwarder::new(mux_store.clone());
    tokio::spawn(async move {
        if let Err(e) = dns.run().await {
            error!("DNS Forwarder failed: {}", e);
        }
    });

    // Set up dummy network interface so outbound connections are routable
    // (required for iptables REDIRECT to work — kernel needs a route to enter OUTPUT chain)
    setup_dummy_route().await;

    // Install iptables rules for transparent proxy
    setup_iptables().await;

    // Auto-detect transport
    let connector: Box<dyn VsockConnector> = if Path::new("/dev/vsock").exists() {
        debug!("Using VSOCK transport (/dev/vsock)");
        Box::new(LinuxVsockConnector::new())
    } else if let Some(path) = detect_virtio_serial() {
        debug!("Using virtio-serial transport ({})", path.display());
        Box::new(VirtioSerialConnector::with_path(path))
    } else {
        error!("No transport available");
        return Err(anyhow::anyhow!("No transport available"));
    };

    let mut listener = connector.listen(VSOCK_PORT).await?;
    debug!("Listening on port {}", VSOCK_PORT);

    loop {
        match listener.accept().await {
            Ok(mut stream) => {
                // Exclusive connection: reject if already connected
                if mux_store.read().await.is_some() {
                    warn!("Already connected — rejecting new connection");
                    let _ = stream.close().await;
                    continue;
                }

                debug!("Accepted connection from Host — verifying auth token");

                // Read 32-byte auth token with 10-second timeout.
                // VsockStream.read() may return partial data, so accumulate.
                let mut token_buf = Vec::with_capacity(32);
                let auth_ok = match tokio::time::timeout(
                    tokio::time::Duration::from_secs(10),
                    async {
                        while token_buf.len() < 32 {
                            let chunk = stream.read().await?;
                            if chunk.is_empty() {
                                return Err(anyhow!("Stream closed before token received"));
                            }
                            token_buf.extend_from_slice(&chunk);
                        }
                        Ok::<(), anyhow::Error>(())
                    },
                ).await {
                    Ok(Ok(())) => constant_time_eq(&expected_token, &token_buf[..32]),
                    Ok(Err(e)) => {
                        warn!("Auth token read error: {}", e);
                        false
                    }
                    Err(_) => {
                        warn!("Auth token read timed out (10s)");
                        false
                    }
                };

                if !auth_ok {
                    warn!("Auth token mismatch — rejecting connection");
                    let _ = stream.close().await;
                    continue;
                }

                debug!("Auth token verified — creating multiplexer");

                let (incoming_tx, mut incoming_rx) = mpsc::channel(100);
                let multiplexer = Arc::new(StreamMultiplexer::new(stream, Some(incoming_tx)));

                {
                    let mut lock = mux_store.write().await;
                    *lock = Some(multiplexer.clone());
                }

                let mux_store_cleanup = mux_store.clone();
                tokio::spawn(async move {
                    while let Some(stream) = incoming_rx.recv().await {
                        tokio::spawn(async move {
                            if let Err(e) = handle_inbound_stream(stream).await {
                                error!("Inbound stream error: {}", e);
                            }
                        });
                    }
                    // Connection closed — clear mux_store to allow reconnection
                    debug!("Connection closed — ready to accept new connection");
                    *mux_store_cleanup.write().await = None;
                });
            }
            Err(e) => {
                error!("Accept error: {}", e);
                tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
            }
        }
    }
}
