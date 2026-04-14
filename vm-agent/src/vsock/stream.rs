//! VSOCK Stream Management
//!
//! Handles multiplexing of logical streams over a single physical VSOCK connection.

use anyhow::{anyhow, Result};
use bytes::{Bytes, BytesMut};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{debug, error, warn};

use super::{
    protocol::{Frame, FrameType},
    VsockStream,
};

/// Size of the channel buffer for incoming/outgoing frames
const CHANNEL_BUFFER_SIZE: usize = 100;

/// Type for handling unsolicited incoming streams
pub type IncomingStreamHandler = mpsc::Sender<VirtualStream>;

/// A virtual stream multiplexed over a physical connection
pub struct VirtualStream {
    pub stream_id: u32,
    incoming_rx: mpsc::Receiver<Frame>,
    outgoing_tx: mpsc::Sender<Frame>,
    closed: Arc<AtomicBool>,
}

/// Write-only handle to a VirtualStream — movable into a separate task.
pub struct VirtualStreamWriter {
    pub stream_id: u32,
    outgoing_tx: mpsc::Sender<Frame>,
    closed: Arc<AtomicBool>,
}

impl VirtualStreamWriter {
    pub async fn write(&self, data: &[u8]) -> Result<()> {
        let frame = Frame::data(self.stream_id, Bytes::copy_from_slice(data));
        self.outgoing_tx
            .send(frame)
            .await
            .map_err(|_| anyhow!("Failed to send frame"))
    }

    pub async fn close(&self) -> Result<()> {
        if self.closed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let frame = Frame::fin(self.stream_id);
        self.outgoing_tx
            .send(frame)
            .await
            .map_err(|_| anyhow!("Failed to send fin frame"))
    }
}

impl VirtualStream {
    fn new(
        stream_id: u32,
        incoming_rx: mpsc::Receiver<Frame>,
        outgoing_tx: mpsc::Sender<Frame>,
    ) -> Self {
        Self {
            stream_id,
            incoming_rx,
            outgoing_tx,
            closed: Arc::new(AtomicBool::new(false)),
        }
    }

    fn suppress_drop_close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    /// Return a write-only handle that can be moved into a separate async task.
    pub fn writer(&self) -> VirtualStreamWriter {
        VirtualStreamWriter {
            stream_id: self.stream_id,
            outgoing_tx: self.outgoing_tx.clone(),
            closed: self.closed.clone(),
        }
    }
}

#[async_trait::async_trait]
impl VsockStream for VirtualStream {
    async fn read(&mut self) -> Result<Bytes> {
        match self.incoming_rx.recv().await {
            Some(frame) => match frame.frame_type {
                FrameType::Data
                | FrameType::ReverseResponse
                | FrameType::ReverseRequest
                | FrameType::DnsResponse
                | FrameType::DnsRequest
                | FrameType::TtyData
                | FrameType::TtyResize
                | FrameType::Connect
                | FrameType::HostConnect
                | FrameType::WindowUpdate => Ok(frame.payload),
                FrameType::Fin => Err(anyhow!("Stream closed by peer")),
                FrameType::Rst => Err(anyhow!("Stream reset by peer")),
            },
            None => Err(anyhow!("Stream channel closed")),
        }
    }

    async fn write(&mut self, data: &[u8]) -> Result<()> {
        let frame = Frame::data(self.stream_id, Bytes::copy_from_slice(data));
        self.outgoing_tx
            .send(frame)
            .await
            .map_err(|_| anyhow!("Failed to send frame"))
    }

    async fn close(&mut self) -> Result<()> {
        if self.closed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let frame = Frame::fin(self.stream_id);
        self.outgoing_tx
            .send(frame)
            .await
            .map_err(|_| anyhow!("Failed to send close frame"))
    }
}

impl Drop for VirtualStream {
    fn drop(&mut self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            let _ = self.outgoing_tx.try_send(Frame::fin(self.stream_id));
        }
    }
}

/// Manager for multiplexing streams
pub struct StreamMultiplexer {
    streams: Arc<RwLock<HashMap<u32, mpsc::Sender<Frame>>>>,
    outgoing_tx: mpsc::Sender<Frame>,
    next_stream_id: Arc<Mutex<u32>>,
}

impl StreamMultiplexer {
    /// Create a new multiplexer over a physical stream.
    pub fn new(
        physical_stream: Box<dyn VsockStream>,
        incoming_handler: Option<IncomingStreamHandler>,
    ) -> Self {
        let (outgoing_tx, outgoing_rx) = mpsc::channel(CHANNEL_BUFFER_SIZE);
        let streams = Arc::new(RwLock::new(HashMap::new()));
        // VM-agent uses even stream IDs (2, 4, 6...) to avoid collision with
        // host-initiated odd IDs (1, 3, 5...).
        let next_stream_id = Arc::new(Mutex::new(2u32));

        let multiplexer = Self {
            streams: streams.clone(),
            outgoing_tx: outgoing_tx.clone(),
            next_stream_id,
        };

        // Spawn background task to handle physical I/O
        tokio::spawn(Self::run_io_loop(
            physical_stream,
            streams,
            outgoing_rx,
            incoming_handler,
            outgoing_tx,
        ));

        multiplexer
    }

    /// Create a new virtual stream
    pub async fn create_stream(&self, target_port: u32) -> Result<VirtualStream> {
        let mut id_guard = self.next_stream_id.lock().await;
        let stream_id = *id_guard;
        *id_guard += 2; // Step by 2 to keep even IDs for vm-agent-initiated streams
        drop(id_guard);

        let (incoming_tx, incoming_rx) = mpsc::channel(CHANNEL_BUFFER_SIZE);

        {
            let mut streams_guard = self.streams.write().await;
            streams_guard.insert(stream_id, incoming_tx);
        }

        // Send Connect frame
        let connect_frame = Frame::connect(stream_id, target_port);
        self.outgoing_tx
            .send(connect_frame)
            .await
            .map_err(|_| anyhow!("Failed to send connect frame"))?;

        Ok(VirtualStream::new(
            stream_id,
            incoming_rx,
            self.outgoing_tx.clone(),
        ))
    }

    /// Create a new reverse proxy stream
    pub async fn create_reverse_stream(
        &self,
        initial_payload: impl Into<Bytes>,
    ) -> Result<VirtualStream> {
        let mut id_guard = self.next_stream_id.lock().await;
        let stream_id = *id_guard;
        *id_guard += 2; // Step by 2 to keep even IDs for vm-agent-initiated streams
        drop(id_guard);

        let (incoming_tx, incoming_rx) = mpsc::channel(CHANNEL_BUFFER_SIZE);

        {
            let mut streams_guard = self.streams.write().await;
            streams_guard.insert(stream_id, incoming_tx);
        }

        // Send ReverseRequest frame
        let frame = Frame::reverse_request(stream_id, initial_payload);
        self.outgoing_tx
            .send(frame)
            .await
            .map_err(|_| anyhow!("Failed to send reverse request frame"))?;

        Ok(VirtualStream::new(
            stream_id,
            incoming_rx,
            self.outgoing_tx.clone(),
        ))
    }

    /// Create a new DNS request stream
    pub async fn create_dns_stream(&self, payload: impl Into<Bytes>) -> Result<VirtualStream> {
        let mut id_guard = self.next_stream_id.lock().await;
        let stream_id = *id_guard;
        *id_guard += 2;
        drop(id_guard);

        let (incoming_tx, incoming_rx) = mpsc::channel(CHANNEL_BUFFER_SIZE);

        {
            let mut streams_guard = self.streams.write().await;
            streams_guard.insert(stream_id, incoming_tx);
        }

        let frame = Frame::dns_request(stream_id, payload);
        self.outgoing_tx
            .send(frame)
            .await
            .map_err(|_| anyhow!("Failed to send DNS request frame"))?;

        Ok(VirtualStream::new(
            stream_id,
            incoming_rx,
            self.outgoing_tx.clone(),
        ))
    }

    /// Create a new stream for HostConnect (raw TCP tunnel to host localhost)
    /// mode: 0x00=Auto, 0x01=Headless, 0x02=Headed
    pub async fn create_host_connect_stream(
        &self,
        target_port: u32,
        mode: u8,
    ) -> Result<VirtualStream> {
        let mut id_guard = self.next_stream_id.lock().await;
        let stream_id = *id_guard;
        *id_guard += 2;
        drop(id_guard);

        let (incoming_tx, incoming_rx) = mpsc::channel(CHANNEL_BUFFER_SIZE);

        {
            let mut streams_guard = self.streams.write().await;
            streams_guard.insert(stream_id, incoming_tx);
        }

        let frame = Frame::host_connect(stream_id, target_port, mode);
        self.outgoing_tx
            .send(frame)
            .await
            .map_err(|_| anyhow!("Failed to send HostConnect frame"))?;

        Ok(VirtualStream::new(
            stream_id,
            incoming_rx,
            self.outgoing_tx.clone(),
        ))
    }

    /// Handle the IO loop
    async fn run_io_loop(
        mut physical: Box<dyn VsockStream>,
        streams: Arc<RwLock<HashMap<u32, mpsc::Sender<Frame>>>>,
        mut outgoing_rx: mpsc::Receiver<Frame>,
        incoming_handler: Option<IncomingStreamHandler>,
        outgoing_tx: mpsc::Sender<Frame>,
    ) {
        let mut buf = BytesMut::with_capacity(4096);

        loop {
            tokio::select! {
                // Read from physical stream
                read_result = physical.read() => {
                    match read_result {
                        Ok(data) => {
                            buf.extend_from_slice(&data);

                            // Try to decode frames
                            loop {
                                if buf.len() < Frame::HEADER_SIZE {
                                    break;
                                }

                                if FrameType::try_from(buf[4]).is_ok() {
                                    let length = u32::from_be_bytes(buf[5..9].try_into().unwrap()) as usize;
                                    let total_len = Frame::HEADER_SIZE + length;

                                    if buf.len() >= total_len {
                                        let frame_bytes = buf.split_to(total_len).freeze();
                                        match Frame::decode(frame_bytes) {
                                            Ok(frame) => {
                                                // Try read lock first for existing streams (fast path)
                                                let tx = {
                                                    let streams_guard = streams.read().await;
                                                    streams_guard.get(&frame.stream_id).cloned()
                                                };
                                                if let Some(tx) = tx {
                                                    let is_terminal = frame.frame_type == FrameType::Fin
                                                        || frame.frame_type == FrameType::Rst;
                                                    let sid = frame.stream_id;
                                                    // Existing stream: use blocking send to preserve
                                                    // data integrity (backpressure). Only clean up
                                                    // when the receiver is dropped (channel closed).
                                                    if tx.send(frame).await.is_err() {
                                                        let mut streams_guard = streams.write().await;
                                                        streams_guard.remove(&sid);
                                                    } else if is_terminal {
                                                        let mut streams_guard = streams.write().await;
                                                        streams_guard.remove(&sid);
                                                    }
                                                } else if frame.frame_type == FrameType::ReverseRequest || frame.frame_type == FrameType::Connect {
                                                    if let Some(handler) = &incoming_handler {
                                                        let (incoming_tx, incoming_rx) = mpsc::channel(CHANNEL_BUFFER_SIZE);
                                                        {
                                                            let mut streams_guard = streams.write().await;
                                                            streams_guard.insert(frame.stream_id, incoming_tx.clone());
                                                        }

                                                        let stream = VirtualStream::new(
                                                            frame.stream_id,
                                                            incoming_rx,
                                                            outgoing_tx.clone(),
                                                        );

                                                        let sid = frame.stream_id;
                                                        match handler.try_send(stream) {
                                                            Ok(()) => {
                                                                if incoming_tx.try_send(frame).is_err() {
                                                                    warn!("[VM-MUX] stream {} initial frame channel full", sid);
                                                                }
                                                            }
                                                            Err(tokio::sync::mpsc::error::TrySendError::Full(stream))
                                                            | Err(tokio::sync::mpsc::error::TrySendError::Closed(stream)) => {
                                                                stream.suppress_drop_close();
                                                                error!("[VM-MUX] handler channel full, rejecting stream {}", sid);
                                                                {
                                                                    let mut streams_guard = streams.write().await;
                                                                    streams_guard.remove(&sid);
                                                                }
                                                                let _ = outgoing_tx.try_send(Frame::rst(sid));
                                                            }
                                                        }
                                                    }
                                                } else if frame.frame_type == FrameType::WindowUpdate {
                                                    // WindowUpdate from host — ignored on vm-agent side
                                                } else {
                                                    warn!("[VM-MUX] frame for unknown stream {}", frame.stream_id);
                                                }
                                            }
                                            Err(e) => {
                                                error!("[VM-MUX] frame decode error: {}", e);
                                                return;
                                            }
                                        }
                                    } else {
                                        break; // Wait for more data
                                    }
                                } else {
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            warn!("[VM-MUX] physical read ended: {}", e);
                            break;
                        }
                    }
                }

                // Write to physical stream (batch multiple frames, capped at 128KB)
                Some(frame) = outgoing_rx.recv() => {
                    const MAX_BATCH_BYTES: usize = 128 * 1024;
                    let mut combined = BytesMut::new();
                    combined.extend_from_slice(&frame.encode());
                    while combined.len() < MAX_BATCH_BYTES {
                        match outgoing_rx.try_recv() {
                            Ok(f) => combined.extend_from_slice(&f.encode()),
                            Err(_) => break,
                        }
                    }
                    if let Err(e) = physical.write(&combined).await {
                        error!("[VM-MUX] physical write error: {}", e);
                        break;
                    }
                }

            }
        }
        // IO loop exited (VM shutdown or connection lost).
        // Clear all stream entries so handler tasks waiting on
        // incoming_rx.recv() immediately get None and can terminate.
        {
            let mut streams_guard = streams.write().await;
            let remaining = streams_guard.len();
            if remaining > 0 {
                streams_guard.clear();
                debug!(
                    "[VM-MUX] IO loop exited, cleaned up {} active streams",
                    remaining
                );
            }
        }
        outgoing_rx.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn writer_close_and_drop_only_send_one_fin() {
        let (outgoing_tx, mut outgoing_rx) = mpsc::channel(8);
        let (_incoming_tx, incoming_rx) = mpsc::channel(1);
        let stream = VirtualStream::new(8, incoming_rx, outgoing_tx);
        let writer = stream.writer();

        writer.close().await.unwrap();
        drop(stream);

        let frame = outgoing_rx.recv().await.unwrap();
        assert_eq!(frame.stream_id, 8);
        assert_eq!(frame.frame_type, FrameType::Fin);
        assert!(outgoing_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn suppressed_drop_does_not_send_fin() {
        let (outgoing_tx, mut outgoing_rx) = mpsc::channel(8);
        let (_incoming_tx, incoming_rx) = mpsc::channel(1);
        let stream = VirtualStream::new(10, incoming_rx, outgoing_tx);

        stream.suppress_drop_close();
        drop(stream);

        assert!(outgoing_rx.try_recv().is_err());
    }
}
