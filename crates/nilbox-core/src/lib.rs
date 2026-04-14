//! nilbox-core — Tauri-independent core library
//!
//! All business logic lives here. No dependency on `tauri` crate.

// Phase 2: Ported from bluejean
pub mod vm_platform;
pub mod vsock;
pub mod gateway;
pub mod proxy;
pub mod keystore;
pub mod ssh;
pub mod config;
pub mod config_store;

// Phase 3: Core abstractions
pub mod events;
pub mod state;
pub mod service;
pub mod control_client;

// File proxy (FUSE over VSOCK)
pub mod file_proxy;

// Token monitor
pub mod token_monitor;

// Input validation helpers
pub mod validate;

// Phase 4: New modules
pub mod vm_install;
pub mod store;
pub mod mcp_bridge;
pub mod monitoring;
pub mod audit;
pub mod recovery;
pub mod ssh_gateway;
