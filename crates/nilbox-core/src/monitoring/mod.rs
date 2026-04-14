//! Monitoring module — VM metrics collection

use serde::{Serialize, Deserialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::SystemTime;
use tokio::sync::watch;

/// CPU usage snapshot from /proc/stat (jiffies).
pub struct CpuSnapshot {
    pub total: u64,
    pub idle: u64,
}

/// Parse the aggregate "cpu" line from /proc/stat.
/// Returns None if the line cannot be found or parsed.
pub fn parse_proc_stat(data: &str) -> Option<CpuSnapshot> {
    // The aggregate line starts with "cpu " (followed by space, not a digit)
    let line = data.lines().find(|l| {
        let mut chars = l.chars();
        chars.next() == Some('c')
            && chars.next() == Some('p')
            && chars.next() == Some('u')
            && chars.next().map(|c| c.is_whitespace()).unwrap_or(false)
    })?;
    let parts: Vec<u64> = line
        .split_whitespace()
        .skip(1) // skip "cpu" token
        .filter_map(|s| s.parse().ok())
        .collect();
    if parts.len() < 4 {
        return None;
    }
    // Fields: user nice system idle iowait irq softirq steal ...
    // idle + iowait are both "idle" time
    let idle = parts[3] + parts.get(4).copied().unwrap_or(0);
    let total: u64 = parts.iter().sum();
    Some(CpuSnapshot { total, idle })
}

/// Parse /proc/meminfo and return (used_mb, total_mb).
pub fn parse_proc_meminfo(data: &str) -> Option<(u64, u64)> {
    let mut total_kb = 0u64;
    let mut available_kb = 0u64;
    for line in data.lines() {
        if line.starts_with("MemTotal:") {
            total_kb = line.split_whitespace().nth(1)?.parse().ok()?;
        } else if line.starts_with("MemAvailable:") {
            available_kb = line.split_whitespace().nth(1)?.parse().ok()?;
        }
    }
    if total_kb == 0 {
        return None;
    }
    let used_kb = total_kb.saturating_sub(available_kb);
    Some((used_kb / 1024, total_kb / 1024))
}

/// Parse /proc/net/dev and return cumulative (rx_bytes, tx_bytes) across non-loopback interfaces.
pub fn parse_proc_net_dev(data: &str) -> Option<(u64, u64)> {
    let mut rx_total = 0u64;
    let mut tx_total = 0u64;
    let mut found = false;
    for line in data.lines() {
        let trimmed = line.trim();
        let colon = match trimmed.find(':') {
            Some(pos) => pos,
            None => continue,
        };
        let iface = trimmed[..colon].trim();
        if iface == "lo" {
            continue;
        }
        // Fields after colon: rx_bytes rx_packets rx_errs rx_drop rx_fifo
        //   rx_frame rx_compressed rx_multicast | tx_bytes ...
        let fields: Vec<u64> = trimmed[colon + 1..]
            .split_whitespace()
            .filter_map(|s| s.parse().ok())
            .collect();
        if fields.len() >= 9 {
            rx_total += fields[0]; // rx_bytes
            tx_total += fields[8]; // tx_bytes
            found = true;
        }
    }
    if found { Some((rx_total, tx_total)) } else { None }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmMetrics {
    pub cpu_percent: f64,
    pub memory_used_mb: u64,
    pub memory_total_mb: u64,
    pub network_tx_bytes: u64,
    pub network_rx_bytes: u64,
    pub timestamp: SystemTime,
}

impl Default for VmMetrics {
    fn default() -> Self {
        Self {
            cpu_percent: 0.0,
            memory_used_mb: 0,
            memory_total_mb: 0,
            network_tx_bytes: 0,
            network_rx_bytes: 0,
            timestamp: SystemTime::now(),
        }
    }
}

/// Snapshot emitted every 500ms via `vm-metrics-stream` event.
/// Combines real-time proxy activity (network) with cached VM metrics (CPU/memory).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyActivitySnapshot {
    /// Most recently accessed domain (from last proxy request in this interval)
    pub last_domain: Option<String>,
    /// Bytes downloaded (response→VM) in this 500ms interval
    pub rx_bytes_delta: u64,
    /// Bytes uploaded (request from VM) in this 500ms interval
    pub tx_bytes_delta: u64,
    /// Whether any proxy traffic occurred in this interval
    pub active: bool,
    /// Cached CPU % from last metrics collection (updated every 15s)
    pub cpu_percent: f64,
    /// Cached memory used MB
    pub memory_used_mb: u64,
    /// Cached memory total MB
    pub memory_total_mb: u64,
    /// Cumulative proxy rx bytes (total)
    pub network_rx_bytes: u64,
    /// Cumulative proxy tx bytes (total)
    pub network_tx_bytes: u64,
}

pub struct MonitoringCollector {
    tx: watch::Sender<VmMetrics>,
    rx: watch::Receiver<VmMetrics>,
    /// Cumulative bytes received by VM from internet (via proxy) — Network Down
    pub proxy_rx_bytes: Arc<AtomicU64>,
    /// Cumulative bytes sent by VM to internet (via proxy) — Network Up
    pub proxy_tx_bytes: Arc<AtomicU64>,
    /// Per-interval accumulator: bytes downloaded in current 500ms window
    interval_rx_bytes: Arc<AtomicU64>,
    /// Per-interval accumulator: bytes uploaded in current 500ms window
    interval_tx_bytes: Arc<AtomicU64>,
    /// Most recent domain accessed (updated per proxy request)
    last_domain: Arc<Mutex<Option<String>>>,
}

impl MonitoringCollector {
    pub fn new() -> Self {
        let (tx, rx) = watch::channel(VmMetrics::default());
        Self {
            tx,
            rx,
            proxy_rx_bytes: Arc::new(AtomicU64::new(0)),
            proxy_tx_bytes: Arc::new(AtomicU64::new(0)),
            interval_rx_bytes: Arc::new(AtomicU64::new(0)),
            interval_tx_bytes: Arc::new(AtomicU64::new(0)),
            last_domain: Arc::new(Mutex::new(None)),
        }
    }

    /// Get current metrics snapshot.
    pub fn get_metrics(&self) -> VmMetrics {
        self.rx.borrow().clone()
    }

    /// Get a receiver for subscribing to metric updates.
    pub fn subscribe(&self) -> watch::Receiver<VmMetrics> {
        self.rx.clone()
    }

    /// Update metrics (called internally by collection loop).
    pub fn update(&self, metrics: VmMetrics) {
        let _ = self.tx.send(metrics);
    }

    /// Increment proxy byte counters from proxy handlers.
    /// rx = bytes written to VM (response from internet), tx = bytes from VM (request to internet).
    pub fn add_proxy_bytes(&self, rx: u64, tx: u64) {
        self.proxy_rx_bytes.fetch_add(rx, Ordering::Relaxed);
        self.proxy_tx_bytes.fetch_add(tx, Ordering::Relaxed);
    }

    /// Read current proxy byte counters.
    pub fn get_proxy_bytes(&self) -> (u64, u64) {
        (
            self.proxy_rx_bytes.load(Ordering::Relaxed),
            self.proxy_tx_bytes.load(Ordering::Relaxed),
        )
    }

    /// Record proxy activity with domain info.
    /// Updates both cumulative counters and per-interval accumulators.
    pub fn record_proxy_activity(&self, domain: &str, rx: u64, tx: u64) {
        self.add_proxy_bytes(rx, tx);
        self.interval_rx_bytes.fetch_add(rx, Ordering::Relaxed);
        self.interval_tx_bytes.fetch_add(tx, Ordering::Relaxed);
        if let Ok(mut d) = self.last_domain.lock() {
            *d = Some(domain.to_string());
        }
    }

    /// Atomically take the current interval snapshot and reset accumulators.
    /// Returns a snapshot combining real-time network delta with cached CPU/memory.
    pub fn take_interval_snapshot(&self) -> ProxyActivitySnapshot {
        let rx_delta = self.interval_rx_bytes.swap(0, Ordering::Relaxed);
        let tx_delta = self.interval_tx_bytes.swap(0, Ordering::Relaxed);
        let domain = self.last_domain.lock().ok().and_then(|mut d| d.take());
        let metrics = self.get_metrics();
        let (total_rx, total_tx) = self.get_proxy_bytes();
        ProxyActivitySnapshot {
            last_domain: domain,
            rx_bytes_delta: rx_delta,
            tx_bytes_delta: tx_delta,
            active: rx_delta > 0 || tx_delta > 0,
            cpu_percent: metrics.cpu_percent,
            memory_used_mb: metrics.memory_used_mb,
            memory_total_mb: metrics.memory_total_mb,
            network_rx_bytes: total_rx,
            network_tx_bytes: total_tx,
        }
    }
}

impl Default for MonitoringCollector {
    fn default() -> Self {
        Self::new()
    }
}
