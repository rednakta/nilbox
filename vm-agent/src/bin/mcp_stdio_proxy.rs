//! mcp-stdio-proxy — TCP-to-stdio bridge for MCP servers inside nilbox VMs.
//!
//! Reads `/etc/nilbox/mcp-servers.json` and opens one TCP listener per
//! configured MCP server. Each inbound TCP connection spawns the server
//! command as a subprocess and bridges TCP ↔ stdin/stdout bidirectionally.
//!
//! Signals:
//!   SIGHUP  — reload configuration (add/remove/update servers)
//!   SIGTERM — graceful shutdown (terminate all subprocesses)
//!
//! Auto-reload:
//!   Watches config file via inotify. Edits trigger reload automatically.
//!
//! This is the Rust port of the original Python tool that used to live at
//! `vm-agent/tools/mcp-stdio-proxy`.

#![cfg(target_os = "linux")]

use std::collections::HashMap;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;

const CONFIG_PATH: &str = "/etc/nilbox/mcp-servers.json";
const SUBPROCESS_KILL_TIMEOUT: Duration = Duration::from_secs(5);

// inotify constants (Linux)
const IN_CLOSE_WRITE: u32 = 0x0000_0008;
const IN_MOVED_TO: u32 = 0x0000_0080;
const IN_CREATE: u32 = 0x0000_0100;
const INOTIFY_WATCH_MASK: u32 = IN_CLOSE_WRITE | IN_MOVED_TO | IN_CREATE;

// ── Configuration ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    servers: Vec<ServerEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct ServerEntry {
    #[serde(default)]
    name: Option<String>,
    port: Option<u16>,
    #[serde(default)]
    command: Vec<String>,
    #[serde(default)]
    env: Option<HashMap<String, String>>,
}

impl ServerEntry {
    fn display_name(&self, port: u16) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| format!("port-{}", port))
    }
}

fn load_config() -> Vec<ServerEntry> {
    let path = Path::new(CONFIG_PATH);
    if !path.exists() {
        warn!(
            "Config file not found: {} — running with no servers",
            CONFIG_PATH
        );
        return Vec::new();
    }
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<ConfigFile>(&text) {
            Ok(cfg) => {
                debug!("Loaded {} server(s) from {}", cfg.servers.len(), CONFIG_PATH);
                cfg.servers
            }
            Err(e) => {
                error!("Failed to parse config {}: {}", CONFIG_PATH, e);
                Vec::new()
            }
        },
        Err(e) => {
            error!("Failed to read config {}: {}", CONFIG_PATH, e);
            Vec::new()
        }
    }
}

// ── Per-connection handler ──────────────────────────────────────────────

async fn handle_connection(
    stream: tokio::net::TcpStream,
    peer: String,
    command: Vec<String>,
    env: Option<HashMap<String, String>>,
    name: String,
    lock: Arc<Mutex<()>>,
) {
    // Single active session per port: if lock is held, reject immediately.
    let _guard = match lock.try_lock() {
        Ok(g) => g,
        Err(_) => {
            warn!(
                "[{}] Rejecting connection from {} — session already active",
                name, peer
            );
            drop(stream);
            return;
        }
    };

    debug!("[{}] Connection from {} — spawning: {:?}", name, peer, command);

    if command.is_empty() {
        error!("[{}] Empty command, closing connection", name);
        return;
    }

    let mut cmd = Command::new(&command[0]);
    cmd.args(&command[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(extra) = &env {
        for (k, v) in extra {
            cmd.env(k, v);
        }
    }
    cmd.kill_on_drop(true);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            error!("[{}] Failed to spawn {:?}: {}", name, command, e);
            return;
        }
    };

    let pid = child.id().unwrap_or(0);
    debug!("[{}] Subprocess pid={} started", name, pid);

    let mut child_stdin = child.stdin.take().expect("stdin piped");
    let child_stdout = child.stdout.take().expect("stdout piped");
    let child_stderr = child.stderr.take().expect("stderr piped");

    let (mut tcp_read, mut tcp_write) = stream.into_split();

    let name_in = name.clone();
    let tcp_to_stdin = tokio::spawn(async move {
        let mut buf = vec![0u8; 8192];
        loop {
            match tcp_read.read(&mut buf).await {
                Ok(0) => {
                    debug!("[{}] TCP read EOF", name_in);
                    break;
                }
                Ok(n) => {
                    if let Err(e) = child_stdin.write_all(&buf[..n]).await {
                        debug!("[{}] tcp_to_stdin ended: {}", name_in, e);
                        break;
                    }
                    if let Err(e) = child_stdin.flush().await {
                        debug!("[{}] tcp_to_stdin flush ended: {}", name_in, e);
                        break;
                    }
                }
                Err(e) => {
                    debug!("[{}] tcp_to_stdin ended: {}", name_in, e);
                    break;
                }
            }
        }
        // Drop stdin to close subprocess input.
        drop(child_stdin);
    });

    let name_out = name.clone();
    let stdout_to_tcp = tokio::spawn(async move {
        let mut child_stdout = child_stdout;
        let mut buf = vec![0u8; 8192];
        loop {
            match child_stdout.read(&mut buf).await {
                Ok(0) => {
                    debug!("[{}] stdout EOF", name_out);
                    break;
                }
                Ok(n) => {
                    if let Err(e) = tcp_write.write_all(&buf[..n]).await {
                        debug!("[{}] stdout_to_tcp ended: {}", name_out, e);
                        break;
                    }
                    if let Err(e) = tcp_write.flush().await {
                        debug!("[{}] stdout_to_tcp flush ended: {}", name_out, e);
                        break;
                    }
                }
                Err(e) => {
                    debug!("[{}] stdout_to_tcp ended: {}", name_out, e);
                    break;
                }
            }
        }
        let _ = tcp_write.shutdown().await;
    });

    let name_err = name.clone();
    let log_stderr = tokio::spawn(async move {
        let mut reader = BufReader::new(child_stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            debug!("[{}:stderr] {}", name_err, line);
        }
    });

    // Wait until either direction finishes.
    tokio::select! {
        _ = tcp_to_stdin => {},
        _ = stdout_to_tcp => {},
    }

    // Terminate subprocess: SIGTERM → wait → SIGKILL.
    terminate_subprocess(&mut child, &name, pid).await;

    // Cancel the stderr logger.
    log_stderr.abort();

    debug!("[{}] Connection from {} closed", name, peer);
}

async fn terminate_subprocess(child: &mut tokio::process::Child, name: &str, pid: u32) {
    if child.try_wait().ok().flatten().is_some() {
        return;
    }
    // SIGTERM
    if pid > 0 {
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
    }
    match tokio::time::timeout(SUBPROCESS_KILL_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) => {
            debug!("[{}] pid={} exited with code {:?}", name, pid, status.code());
            return;
        }
        Ok(Err(e)) => {
            debug!("[{}] wait error: {}", name, e);
        }
        Err(_) => {
            warn!(
                "[{}] pid={} did not exit after SIGTERM, sending SIGKILL",
                name, pid
            );
        }
    }
    let _ = child.kill().await;
    match child.wait().await {
        Ok(status) => debug!("[{}] pid={} exited with code {:?}", name, pid, status.code()),
        Err(e) => debug!("[{}] wait error: {}", name, e),
    }
}

// ── inotify file watcher ────────────────────────────────────────────────

/// Spawn a blocking watcher thread. Sends `()` on `tx` whenever the config
/// file is touched. Returns the watcher file descriptor so we can close it
/// on shutdown (which unblocks `read()`).
fn start_inotify_watcher(path: PathBuf, tx: mpsc::Sender<()>) -> Option<i32> {
    let parent = path.parent()?.to_path_buf();
    let filename = path.file_name()?.as_bytes().to_vec();

    let fd = unsafe { libc::inotify_init1(0) };
    if fd < 0 {
        warn!(
            "inotify_init1 failed (errno={}), file watch disabled",
            std::io::Error::last_os_error()
        );
        return None;
    }

    let parent_c = match std::ffi::CString::new(parent.as_os_str().as_bytes()) {
        Ok(s) => s,
        Err(_) => {
            unsafe { libc::close(fd) };
            return None;
        }
    };

    let wd = unsafe { libc::inotify_add_watch(fd, parent_c.as_ptr(), INOTIFY_WATCH_MASK) };
    if wd < 0 {
        warn!(
            "inotify_add_watch failed (errno={}), file watch disabled",
            std::io::Error::last_os_error()
        );
        unsafe { libc::close(fd) };
        return None;
    }

    debug!("inotify: watching {} for changes", path.display());

    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            let n = unsafe {
                libc::read(
                    fd,
                    buf.as_mut_ptr() as *mut libc::c_void,
                    buf.len(),
                )
            };
            if n <= 0 {
                // fd closed or error — exit thread.
                break;
            }
            let data = &buf[..n as usize];
            if has_target_event(data, &filename) {
                info!("inotify: config file changed, triggering reload");
                // Best-effort send; if receiver is gone, just exit.
                if tx.blocking_send(()).is_err() {
                    break;
                }
            }
        }
    });

    Some(fd)
}

fn has_target_event(data: &[u8], filename: &[u8]) -> bool {
    // struct inotify_event { int wd; uint32_t mask, cookie, len; char name[]; }
    let mut offset = 0usize;
    while offset + 16 <= data.len() {
        let name_len = u32::from_ne_bytes([
            data[offset + 12],
            data[offset + 13],
            data[offset + 14],
            data[offset + 15],
        ]) as usize;
        let name_start = offset + 16;
        let name_end = name_start + name_len;
        if name_end > data.len() {
            break;
        }
        let raw = &data[name_start..name_end];
        // Strip trailing NULs.
        let trimmed_end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        if &raw[..trimmed_end] == filename {
            return true;
        }
        offset = name_end;
    }
    false
}

// ── Server lifecycle ────────────────────────────────────────────────────

struct ListenerState {
    handle: JoinHandle<()>,
    cfg: ServerEntry,
}

struct Daemon {
    listeners: HashMap<u16, ListenerState>,
}

impl Daemon {
    fn new() -> Self {
        Self {
            listeners: HashMap::new(),
        }
    }

    async fn apply_config(&mut self, servers: Vec<ServerEntry>) {
        let mut desired: HashMap<u16, ServerEntry> = HashMap::new();
        for s in servers {
            let port = match s.port {
                Some(p) if p > 0 => p,
                _ => {
                    warn!("Skipping invalid server entry: {:?}", s);
                    continue;
                }
            };
            if s.command.is_empty() {
                warn!("Skipping invalid server entry: {:?}", s);
                continue;
            }
            if desired.contains_key(&port) {
                warn!(
                    "Duplicate port {} — skipping: {}",
                    port,
                    s.name.clone().unwrap_or_else(|| "?".to_string())
                );
                continue;
            }
            desired.insert(port, s);
        }

        // Stop removed / changed listeners.
        let existing_ports: Vec<u16> = self.listeners.keys().copied().collect();
        for port in existing_ports {
            let keep = match desired.get(&port) {
                Some(new_cfg) => new_cfg == &self.listeners[&port].cfg,
                None => false,
            };
            if !keep {
                debug!("Stopping listener on port {}", port);
                if let Some(state) = self.listeners.remove(&port) {
                    state.handle.abort();
                }
            }
        }

        // Start new listeners.
        for (port, cfg) in desired {
            if self.listeners.contains_key(&port) {
                continue;
            }
            self.start_server(port, cfg).await;
        }
    }

    async fn start_server(&mut self, port: u16, cfg: ServerEntry) {
        let name = cfg.display_name(port);
        let listener = match TcpListener::bind(("127.0.0.1", port)).await {
            Ok(l) => l,
            Err(e) => {
                error!("Failed to bind port {} for [{}]: {}", port, name, e);
                return;
            }
        };
        debug!(
            "Listening on 127.0.0.1:{} for [{}] → {:?}",
            port, name, cfg.command
        );

        let lock = Arc::new(Mutex::new(()));
        let command = cfg.command.clone();
        let env = cfg.env.clone();
        let name_for_task = name.clone();
        let handle = tokio::spawn(async move {
            loop {
                let (stream, addr) = match listener.accept().await {
                    Ok(v) => v,
                    Err(e) => {
                        error!("[{}] accept error: {}", name_for_task, e);
                        break;
                    }
                };
                let peer = addr.to_string();
                let lock_cl = lock.clone();
                let cmd_cl = command.clone();
                let env_cl = env.clone();
                let name_cl = name_for_task.clone();
                tokio::spawn(async move {
                    handle_connection(stream, peer, cmd_cl, env_cl, name_cl, lock_cl).await;
                });
            }
        });

        self.listeners.insert(
            port,
            ListenerState {
                handle,
                cfg,
            },
        );
    }

    async fn stop_all(&mut self) {
        for (port, state) in self.listeners.drain() {
            debug!("Stopping listener on port {}", port);
            state.handle.abort();
        }
    }
}

// ── Entry point ─────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_writer(std::io::stderr)
        .init();

    info!(
        "mcp-stdio-proxy starting (pid={}, config={})",
        std::process::id(),
        CONFIG_PATH
    );

    let (reload_tx, mut reload_rx) = mpsc::channel::<()>(8);

    // inotify file watcher
    let inotify_fd = start_inotify_watcher(PathBuf::from(CONFIG_PATH), reload_tx.clone());

    // Signal handlers
    let mut sighup = match signal(SignalKind::hangup()) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to install SIGHUP handler: {}", e);
            return;
        }
    };
    let mut sigterm = match signal(SignalKind::terminate()) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to install SIGTERM handler: {}", e);
            return;
        }
    };
    let mut sigint = match signal(SignalKind::interrupt()) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to install SIGINT handler: {}", e);
            return;
        }
    };

    let mut daemon = Daemon::new();
    daemon.apply_config(load_config()).await;

    loop {
        tokio::select! {
            _ = sighup.recv() => {
                debug!("Config reload triggered (SIGHUP)");
                daemon.apply_config(load_config()).await;
            }
            _ = reload_rx.recv() => {
                debug!("Config reload triggered (inotify)");
                daemon.apply_config(load_config()).await;
            }
            _ = sigterm.recv() => {
                info!("SIGTERM received, shutting down");
                break;
            }
            _ = sigint.recv() => {
                info!("SIGINT received, shutting down");
                break;
            }
        }
    }

    daemon.stop_all().await;
    if let Some(fd) = inotify_fd {
        unsafe { libc::close(fd) };
    }
    info!("Shutdown complete");
}
