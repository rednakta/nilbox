//! EventEmitter trait — Tauri-independent event abstraction
//!
//! Core library emits events via `Arc<dyn EventEmitter>`.
//! - Tauri app implements `TauriEventEmitter` → `app_handle.emit_all()`
//! - Tests implement `MockEventEmitter` → event recording/verification

use serde::Serialize;
use std::sync::Arc;

/// Event abstraction for decoupling core logic from Tauri.
pub trait EventEmitter: Send + Sync {
    /// Emit a string payload event.
    fn emit(&self, event: &str, payload: &str);

    /// Emit a raw bytes payload event.
    fn emit_bytes(&self, event: &str, payload: &[u8]);
}

/// Convenience: serialize `payload` to JSON and emit.
pub fn emit_typed<T: Serialize>(emitter: &Arc<dyn EventEmitter>, event: &str, payload: &T) {
    if let Ok(json) = serde_json::to_string(payload) {
        emitter.emit(event, &json);
    }
}

/// No-op emitter for headless / testing scenarios.
pub struct NoopEventEmitter;

impl EventEmitter for NoopEventEmitter {
    fn emit(&self, _event: &str, _payload: &str) {}
    fn emit_bytes(&self, _event: &str, _payload: &[u8]) {}
}
