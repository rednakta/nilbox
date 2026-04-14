//! Gateway module — inbound port forwarding (per-VM)

pub mod listener;
pub mod forwarder;
pub mod cdp_rewriter;

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use anyhow::{Result, anyhow};
use tracing::{debug, warn, error};

use crate::vsock::stream::StreamMultiplexer;
use self::listener::create_listener;
use self::forwarder::forward_connection;

/// host_port → (vm_id, vm_port)
pub type PortMapping = HashMap<u16, (String, u16)>;

pub struct Gateway {
    port_mapping: Arc<RwLock<PortMapping>>,
    listeners: Arc<RwLock<HashMap<u16, JoinHandle<()>>>>,
    /// Per-VM multiplexers: vm_id → multiplexer
    multiplexers: Arc<RwLock<HashMap<String, Arc<StreamMultiplexer>>>>,
}

impl Gateway {
    pub fn new() -> Self {
        Self {
            port_mapping: Arc::new(RwLock::new(HashMap::new())),
            listeners: Arc::new(RwLock::new(HashMap::new())),
            multiplexers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a multiplexer for a specific VM.
    pub async fn set_multiplexer(&self, vm_id: &str, multiplexer: Arc<StreamMultiplexer>) {
        let mut lock = self.multiplexers.write().await;
        lock.insert(vm_id.to_string(), multiplexer);
        debug!("Gateway multiplexer set for VM {}", vm_id);
    }

    /// Remove a VM's multiplexer.
    pub async fn remove_multiplexer(&self, vm_id: &str) {
        let mut lock = self.multiplexers.write().await;
        lock.remove(vm_id);
        debug!("Gateway multiplexer removed for VM {}", vm_id);
    }

    /// Add a port mapping for a specific VM. Host port must be globally unique.
    pub async fn add_mapping(&self, vm_id: &str, host_port: u16, vm_port: u16) -> Result<()> {
        {
            let mapping = self.port_mapping.read().await;
            if let Some((existing_vm, existing_vm_port)) = mapping.get(&host_port) {
                if existing_vm != vm_id {
                    return Err(anyhow!(
                        "Host port {} is already mapped to VM {}",
                        host_port,
                        existing_vm
                    ));
                }
                // Same VM, same port — reuse existing listener if it is still alive
                if *existing_vm_port == vm_port {
                    drop(mapping);
                    let listeners = self.listeners.read().await;
                    if listeners.contains_key(&host_port) {
                        return Ok(());
                    }
                    // Listener is dead; fall through to restart it
                }
            }
        }
        self.stop_listener(host_port).await;
        // Yield so the runtime can process the abort and drop the old TcpListener
        tokio::task::yield_now().await;
        // Start the listener BEFORE updating the mapping to avoid a stale entry on failure
        self.start_listener(vm_id, host_port, vm_port).await?;
        {
            let mut mapping = self.port_mapping.write().await;
            mapping.insert(host_port, (vm_id.to_string(), vm_port));
        }
        Ok(())
    }

    /// Remove a port mapping by host port.
    pub async fn remove_mapping(&self, host_port: u16) {
        self.stop_listener(host_port).await;
        let mut mapping = self.port_mapping.write().await;
        mapping.remove(&host_port);
        debug!("Removed port mapping: {}", host_port);
    }

    /// Remove all port mappings belonging to a specific VM.
    pub async fn remove_mappings_for_vm(&self, vm_id: &str) {
        let ports: Vec<u16> = {
            let mapping = self.port_mapping.read().await;
            mapping
                .iter()
                .filter(|(_, (vid, _))| vid == vm_id)
                .map(|(port, _)| *port)
                .collect()
        };
        for port in ports {
            self.stop_listener(port).await;
        }
        let mut mapping = self.port_mapping.write().await;
        mapping.retain(|_, (vid, _)| vid != vm_id);
        debug!("Removed all port mappings for VM {}", vm_id);
    }

    /// Get all mappings.
    pub async fn get_all_mappings(&self) -> PortMapping {
        self.port_mapping.read().await.clone()
    }

    /// Get mappings for a specific VM.
    pub async fn get_mappings_for_vm(&self, vm_id: &str) -> Vec<(u16, u16)> {
        let mapping = self.port_mapping.read().await;
        mapping
            .iter()
            .filter(|(_, (vid, _))| vid == vm_id)
            .map(|(host_port, (_, vm_port))| (*host_port, *vm_port))
            .collect()
    }

    /// Add an ephemeral port mapping for a specific VM.
    /// Tries to bind to the same port as vm_port first (so HTTP redirects from the VM service
    /// keep working). Falls back to a random OS-assigned port if vm_port is already in use.
    pub async fn add_mapping_ephemeral(&self, vm_id: &str, vm_port: u16) -> Result<u16> {
        let listener = match create_listener(vm_port).await {
            Ok(l) => l,
            Err(_) => create_listener(0).await?,
        };
        let host_port = listener.local_addr()?.port();
        {
            let mut mapping = self.port_mapping.write().await;
            mapping.insert(host_port, (vm_id.to_string(), vm_port));
        }
        self.start_listener_with(vm_id, host_port, vm_port, listener).await?;
        debug!("Ephemeral mapping: localhost:{} -> VM {}:{}", host_port, vm_id, vm_port);
        Ok(host_port)
    }

    async fn start_listener(&self, vm_id: &str, host_port: u16, vm_port: u16) -> Result<()> {
        let listener = create_listener(host_port).await?;
        self.start_listener_with(vm_id, host_port, vm_port, listener).await
    }

    async fn start_listener_with(
        &self,
        vm_id: &str,
        host_port: u16,
        vm_port: u16,
        listener: tokio::net::TcpListener,
    ) -> Result<()> {
        let multiplexers = self.multiplexers.clone();
        let vm_id_owned = vm_id.to_string();

        let handle = tokio::spawn(async move {
            debug!("Started listener on port {} -> VM {}:{}", host_port, vm_id_owned, vm_port);
            loop {
                match listener.accept().await {
                    Ok((socket, addr)) => {
                        debug!("Accepted connection from {}", addr);
                        let muxes = multiplexers.read().await;
                        if let Some(mux) = muxes.get(&vm_id_owned) {
                            match mux.create_stream(vm_port as u32).await {
                                Ok(vsock_stream) => {
                                    tokio::spawn(async move {
                                        if let Err(e) = forward_connection(socket, Box::new(vsock_stream)).await {
                                            error!("Forwarder error: {}", e);
                                        }
                                    });
                                }
                                Err(e) => error!("Failed to create VSOCK stream: {}", e),
                            }
                        } else {
                            warn!("No multiplexer for VM {}, dropping connection", vm_id_owned);
                        }
                    }
                    Err(e) => {
                        error!("Accept error on port {}: {}", host_port, e);
                        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    }
                }
            }
        });

        self.listeners.write().await.insert(host_port, handle);
        Ok(())
    }

    async fn stop_listener(&self, host_port: u16) {
        let mut listeners = self.listeners.write().await;
        if let Some(handle) = listeners.remove(&host_port) {
            handle.abort();
            debug!("Stopped listener on port {}", host_port);
        }
    }
}

impl Default for Gateway {
    fn default() -> Self {
        Self::new()
    }
}
