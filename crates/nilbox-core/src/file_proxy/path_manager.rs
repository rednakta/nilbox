//! Path manager — shared directory switching with sandbox enforcement.
//!
//! Ported from tauri-app-pipe/src-tauri/src/fuse_proxy/path_manager.rs.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

use super::protocol::FuseResponse;

const DEFAULT_SWITCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Path switch state machine
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathState {
    Active = 0,
    Pending = 1,
    Switching = 2,
}

/// Manages the shared path and path switching logic
pub struct PathManager {
    current_path: PathBuf,
    pending_path: Option<PathBuf>,
    previous_paths: Vec<PathBuf>,
    state: PathState,
    open_handles: HashSet<u64>,
    switch_timeout: Duration,
    pending_since: Option<Instant>,
    notification_tx: Option<mpsc::Sender<FuseResponse>>,
    unmount_tx: Option<oneshot::Sender<()>>,
}

impl PathManager {
    pub fn new(initial_path: PathBuf) -> Self {
        let current_path = initial_path.canonicalize().unwrap_or(initial_path);
        Self {
            current_path,
            pending_path: None,
            previous_paths: Vec::new(),
            state: PathState::Active,
            open_handles: HashSet::new(),
            switch_timeout: DEFAULT_SWITCH_TIMEOUT,
            pending_since: None,
            notification_tx: None,
            unmount_tx: None,
        }
    }

    pub fn set_notification_channel(&mut self, tx: mpsc::Sender<FuseResponse>) {
        self.notification_tx = Some(tx);
    }

    pub fn current_path(&self) -> &Path {
        &self.current_path
    }

    pub fn state(&self) -> PathState {
        self.state
    }

    pub fn open_handles_count(&self) -> usize {
        self.open_handles.len()
    }

    pub fn register_handle(&mut self, fd: u64) {
        self.open_handles.insert(fd);
    }

    pub fn unregister_handle(&mut self, fd: u64) {
        self.open_handles.remove(&fd);

        if self.state == PathState::Pending && self.open_handles.is_empty() {
            self.complete_switch();
        }

        if self.unmount_tx.is_some() && self.open_handles.is_empty() {
            if let Some(tx) = self.unmount_tx.take() {
                let _ = tx.send(());
            }
        }
    }

    /// 언마운트 대기 — 모든 파일이 닫히면 oneshot으로 신호를 보낸다.
    pub fn set_unmount_pending(&mut self) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        self.unmount_tx = Some(tx);
        rx
    }

    /// 강제 언마운트용 — open_handles를 즉시 클리어하고 unmount_tx 신호 전송
    pub fn clear_open_handles(&mut self) {
        self.open_handles.clear();
        if let Some(tx) = self.unmount_tx.take() {
            let _ = tx.send(());
        }
    }

    /// Request to change the shared path.
    /// Returns Ok(true) if switch was immediate, Ok(false) if delayed.
    pub async fn request_path_change(&mut self, new_path: PathBuf) -> anyhow::Result<bool> {
        if !new_path.exists() {
            return Err(anyhow::anyhow!("Path does not exist: {:?}", new_path));
        }
        if !new_path.is_dir() {
            return Err(anyhow::anyhow!("Path is not a directory: {:?}", new_path));
        }

        let new_path = new_path.canonicalize()?;

        if new_path == self.current_path {
            return Ok(true);
        }

        if self.open_handles.is_empty() {
            self.do_switch(new_path).await;
            Ok(true)
        } else {
            self.pending_path = Some(new_path);
            self.state = PathState::Pending;
            self.pending_since = Some(Instant::now());

            if let Some(ref tx) = self.notification_tx {
                let _ = tx.send(FuseResponse::PathPending {
                    pending_handles: self.open_handles.len() as u32,
                    timeout_sec: self.switch_timeout.as_secs() as u32,
                }).await;
            }

            Ok(false)
        }
    }

    /// Force switch even if handles are open.
    pub async fn force_switch(&mut self) -> anyhow::Result<()> {
        if let Some(new_path) = self.pending_path.take() {
            self.open_handles.clear();
            self.do_switch(new_path).await;
            Ok(())
        } else {
            Err(anyhow::anyhow!("No pending path change"))
        }
    }

    /// Cancel pending path change.
    pub async fn cancel_path_change(&mut self) {
        self.pending_path = None;
        self.state = PathState::Active;
        self.pending_since = None;

        if let Some(ref tx) = self.notification_tx {
            let _ = tx.send(FuseResponse::PathReady).await;
        }
    }

    /// Check if switch has timed out.
    pub fn check_timeout(&self) -> bool {
        if let Some(since) = self.pending_since {
            since.elapsed() > self.switch_timeout
        } else {
            false
        }
    }

    fn complete_switch(&mut self) {
        if let Some(new_path) = self.pending_path.take() {
            let tx = self.notification_tx.clone();
            let old_path = self.current_path.clone();

            self.previous_paths.push(std::mem::replace(&mut self.current_path, new_path.clone()));
            self.state = PathState::Active;
            self.pending_since = None;

            if let Some(tx) = tx {
                tokio::spawn(async move {
                    let _ = tx.send(FuseResponse::PathChanged {
                        old_path: old_path.to_string_lossy().to_string(),
                        new_path: new_path.to_string_lossy().to_string(),
                    }).await;
                    let _ = tx.send(FuseResponse::PathReady).await;
                    let _ = tx.send(FuseResponse::InvalidateCache).await;
                });
            }
        }
    }

    async fn do_switch(&mut self, new_path: PathBuf) {
        let old_path = std::mem::replace(&mut self.current_path, new_path.clone());
        self.previous_paths.push(old_path.clone());
        self.state = PathState::Active;
        self.pending_since = None;
        self.pending_path = None;

        if let Some(ref tx) = self.notification_tx {
            let _ = tx.send(FuseResponse::PathChanged {
                old_path: old_path.to_string_lossy().to_string(),
                new_path: new_path.to_string_lossy().to_string(),
            }).await;
            let _ = tx.send(FuseResponse::InvalidateCache).await;
        }
    }

    /// Resolve a relative path from guest to absolute host path.
    /// Validates path doesn't escape sandbox.
    pub fn resolve_path(&self, relative_path: &str) -> anyhow::Result<PathBuf> {
        let relative = relative_path.trim_start_matches('/');

        let full_path = if relative.is_empty() {
            self.current_path.clone()
        } else {
            self.current_path.join(relative)
        };

        let canonical = full_path.canonicalize().map_err(|e| {
            anyhow::anyhow!("Path resolution failed: {:?}", e)
        })?;

        if !canonical.starts_with(&self.current_path) {
            return Err(anyhow::anyhow!("Path escapes sandbox: {:?}", relative_path));
        }

        Ok(canonical)
    }

    /// Validate path without canonicalizing (for paths that don't exist yet).
    pub fn validate_new_path(&self, relative_path: &str) -> anyhow::Result<PathBuf> {
        let relative = relative_path.trim_start_matches('/');

        let components: Vec<_> = Path::new(relative).components().collect();
        for comp in &components {
            if let std::path::Component::ParentDir = comp {
                return Err(anyhow::anyhow!("Parent directory traversal not allowed"));
            }
        }

        let full_path = self.current_path.join(relative);
        Ok(full_path)
    }
}

impl Default for PathManager {
    fn default() -> Self {
        Self::new(PathBuf::from("/tmp"))
    }
}
