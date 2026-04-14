//! nilbox-mcp-bridge — stdio ↔ TCP relay for Claude Desktop MCP integration
//!
//! Claude Desktop spawns this binary as a subprocess. It connects to a TCP port
//! on the host where the nilbox Tauri app is forwarding traffic via VSOCK to
//! an MCP server running inside the VM.
//!
//! Flow: Claude Desktop ↔ (stdio) ↔ nilbox-mcp-bridge ↔ (TCP) ↔ Tauri App ↔ (VSOCK) ↔ VM MCP Server

use std::net::IpAddr;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::{debug, error};

#[derive(Parser)]
#[command(name = "nilbox-mcp-bridge")]
#[command(about = "nilbox MCP Bridge — stdio ↔ TCP relay for Claude Desktop")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// TCP port to connect to on localhost
    #[arg(short, long, default_value_t = 0)]
    port: u16,

    /// Host to connect to
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the stdio ↔ TCP bridge (default behavior)
    Bridge {
        /// TCP port to connect to on localhost
        #[arg(short, long)]
        port: u16,

        /// Host to connect to
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
    },

    /// Generate Claude Desktop MCP configuration JSON
    GenerateConfig {
        /// MCP server name
        #[arg(short, long)]
        name: String,

        /// TCP port the bridge will connect to
        #[arg(short, long)]
        port: u16,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("nilbox_mcp_bridge=info".parse().unwrap()),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Bridge { port, host }) => {
            run_bridge(&host, port).await
        }
        Some(Commands::GenerateConfig { name, port }) => {
            generate_config(&name, port);
            Ok(())
        }
        None => {
            // Default: run bridge with top-level --port / --host args
            if cli.port == 0 {
                anyhow::bail!(
                    "Port is required. Usage:\n  \
                     nilbox-mcp-bridge --port <PORT>\n  \
                     nilbox-mcp-bridge bridge --port <PORT>\n  \
                     nilbox-mcp-bridge generate-config --name <NAME> --port <PORT>"
                );
            }
            run_bridge(&cli.host, cli.port).await
        }
    }
}

/// Check if the given host resolves to a loopback address only.
///
/// Security: This bridge must NEVER connect to external networks.
/// Only localhost (127.0.0.0/8, ::1) connections are permitted.
fn validate_localhost(host: &str) -> Result<()> {
    // Fast path: common localhost literals
    if host == "localhost" || host == "127.0.0.1" || host == "::1" {
        return Ok(());
    }

    // Parse as IP address and verify loopback
    if let Ok(ip) = host.parse::<IpAddr>() {
        if ip.is_loopback() {
            return Ok(());
        }
        anyhow::bail!(
            "Security: connection to non-localhost address '{host}' is blocked. \
             Only loopback addresses (127.0.0.1, ::1) are allowed."
        );
    }

    // For hostnames, resolve and check ALL addresses are loopback
    use std::net::ToSocketAddrs;
    let addrs: Vec<_> = (host, 0_u16)
        .to_socket_addrs()
        .with_context(|| format!("Failed to resolve host '{host}'"))?
        .collect();

    if addrs.is_empty() {
        anyhow::bail!("Host '{host}' resolved to no addresses");
    }

    for addr in &addrs {
        if !addr.ip().is_loopback() {
            anyhow::bail!(
                "Security: host '{host}' resolves to non-localhost address '{}'. \
                 Only loopback addresses are allowed.",
                addr.ip()
            );
        }
    }

    Ok(())
}

/// Run the stdio ↔ TCP bridge.
///
/// Reads JSON-RPC messages from stdin and forwards them to the TCP connection,
/// and vice versa. This is the MCP transport protocol used by Claude Desktop.
///
/// Terminates ONLY when TCP connection is closed by server (MCP server disconnect).
/// stdin close does NOT terminate the bridge - TCP response may still arrive.
async fn run_bridge(host: &str, port: u16) -> Result<()> {
    // Security: block all non-localhost connections
    validate_localhost(host)?;

    debug!("Connecting to {}:{}...", host, port);

    let stream = TcpStream::connect((host, port))
        .await
        .with_context(|| format!("Failed to connect to {}:{}", host, port))?;

    debug!("Connected to {}:{}", host, port);

    let (tcp_reader, tcp_writer) = stream.into_split();

    // Spawn: stdin → TCP (runs until stdin closes or TCP write fails)
    let stdin_to_tcp = tokio::spawn(async move {
        let mut stdin = io::stdin();
        let mut writer = tcp_writer;
        let mut buf = [0u8; 8192];

        loop {
            let n = match stdin.read(&mut buf).await {
                Ok(0) => {
                    debug!("stdin closed");
                    break;
                }
                Ok(n) => n,
                Err(e) => {
                    error!("stdin read error: {}", e);
                    break;
                }
            };

            if let Err(e) = writer.write_all(&buf[..n]).await {
                error!("TCP write error: {}", e);
                break;
            }
            if let Err(e) = writer.flush().await {
                error!("TCP flush error: {}", e);
                break;
            }
        }

        // Keep writer alive - don't shutdown, let TCP reader continue
        // Writer will be dropped when this task scope ends
        debug!("stdin→TCP task ended, writer kept alive");

        // Hold writer indefinitely until process exits
        std::future::pending::<()>().await;
    });

    // Spawn: TCP → stdout (runs until TCP closes)
    let tcp_to_stdout = tokio::spawn(async move {
        let mut reader = tcp_reader;
        let mut stdout = io::stdout();
        let mut buf = [0u8; 8192];

        loop {
            let n = match reader.read(&mut buf).await {
                Ok(0) => {
                    debug!("TCP connection closed by server");
                    break;
                }
                Ok(n) => n,
                Err(e) => {
                    error!("TCP read error: {}", e);
                    break;
                }
            };

            if let Err(e) = stdout.write_all(&buf[..n]).await {
                error!("stdout write error: {}", e);
                break;
            }
            if let Err(e) = stdout.flush().await {
                error!("stdout flush error: {}", e);
                break;
            }
        }
    });

    // Bridge terminates ONLY when TCP connection closes
    // stdin task runs independently
    let _ = tcp_to_stdout.await;

    // Abort stdin task since TCP is closed
    stdin_to_tcp.abort();

    debug!("Bridge shutting down");
    Ok(())
}

/// Generate Claude Desktop MCP configuration JSON and print to stdout.
///
/// Output format matches Claude Desktop's `claude_desktop_config.json`.
/// The `command` field uses the absolute path of the current binary so
/// Claude Desktop can locate it inside the app bundle.
/// ```json
/// {
///   "mcpServers": {
///     "<name>": {
///       "command": "/path/to/nilbox.app/Contents/MacOS/nilbox-mcp-bridge",
///       "args": ["--port", "<port>"]
///     }
///   }
/// }
/// ```
fn generate_config(name: &str, port: u16) {
    // Use the current binary's absolute path so Claude Desktop can find it
    // inside the app bundle (e.g. nilbox.app/Contents/MacOS/nilbox-mcp-bridge)
    let command = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "nilbox-mcp-bridge".to_string());

    let config = serde_json::json!({
        "mcpServers": {
            name: {
                "command": command,
                "args": ["--port", port.to_string()]
            }
        }
    });

    println!("{}", serde_json::to_string_pretty(&config).unwrap());
}
