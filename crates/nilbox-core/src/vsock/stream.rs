//! VSOCK stream multiplexer

use anyhow::{anyhow, Result};
use bytes::{Bytes, BytesMut};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{debug, error, trace, warn};

use super::{
    protocol::{Frame, FrameType},
    VsockStream,
};

const CHANNEL_BUFFER_SIZE: usize = 100;

/// Outgoing (host→VM) channel size.
/// macOS (Apple VZ): moderate buffer with 64KB batched writes.
/// Relay socket buffer tuning (1MB/4MB) provides natural backpressure.
/// Linux/Windows (QEMU): full-size buffer, 128KB batched writes.
#[cfg(target_os = "macos")]
const OUTGOING_BUFFER_SIZE: usize = 64;
#[cfg(not(target_os = "macos"))]
const OUTGOING_BUFFER_SIZE: usize = CHANNEL_BUFFER_SIZE;

#[cfg(target_os = "macos")]
const MACOS_MAX_DATA_FRAME_BYTES: usize = 32 * 1024;

pub type IncomingStreamHandler = mpsc::Sender<VirtualStream>;

pub struct VirtualStream {
    pub stream_id: u32,
    pub initial_frame_type: Option<FrameType>,
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
        trace!(
            "[VSOCK-DBG] writer stream={} write {} bytes, channel_cap={}/{}",
            self.stream_id,
            data.len(),
            self.outgoing_tx.capacity(),
            OUTGOING_BUFFER_SIZE
        );
        send_data_frames(&self.outgoing_tx, self.stream_id, data).await
    }

    pub async fn close(&self) -> Result<()> {
        if self.closed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        trace!("[VSOCK-DBG] writer stream={} sending FIN", self.stream_id);
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
        initial_frame_type: Option<FrameType>,
        incoming_rx: mpsc::Receiver<Frame>,
        outgoing_tx: mpsc::Sender<Frame>,
    ) -> Self {
        Self {
            stream_id,
            initial_frame_type,
            incoming_rx,
            outgoing_tx,
            closed: Arc::new(AtomicBool::new(false)),
        }
    }

    fn suppress_drop_close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    /// Send a raw frame on this stream.
    pub async fn send_frame(&self, frame: Frame) -> Result<()> {
        self.outgoing_tx
            .send(frame)
            .await
            .map_err(|_| anyhow!("Failed to send frame"))
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

async fn send_data_frames(
    outgoing_tx: &mpsc::Sender<Frame>,
    stream_id: u32,
    data: &[u8],
) -> Result<()> {
    #[cfg(target_os = "macos")]
    let chunks = data.chunks(MACOS_MAX_DATA_FRAME_BYTES);
    #[cfg(not(target_os = "macos"))]
    let chunks = std::iter::once(data);

    for chunk in chunks {
        let frame = Frame::data(stream_id, Bytes::copy_from_slice(chunk));
        outgoing_tx.send(frame).await.map_err(|_| {
            error!(
                "[VSOCK-DBG] stream={} FAILED to send frame ({} bytes) — outgoing channel closed",
                stream_id,
                chunk.len()
            );
            anyhow!("Failed to send frame")
        })?;
    }

    Ok(())
}

#[async_trait::async_trait]
impl VsockStream for VirtualStream {
    async fn read(&mut self) -> Result<Bytes> {
        match self.incoming_rx.recv().await {
            Some(frame) => match frame.frame_type {
                FrameType::Data
                | FrameType::ReverseResponse
                | FrameType::ReverseRequest
                | FrameType::DnsRequest
                | FrameType::DnsResponse
                | FrameType::TtyData => {
                    trace!(
                        "[VSOCK-DBG] stream={} read {} bytes (type={:?})",
                        self.stream_id,
                        frame.payload.len(),
                        frame.frame_type
                    );
                    Ok(frame.payload)
                }
                FrameType::TtyResize => Ok(frame.payload),
                FrameType::Fin => {
                    trace!(
                        "[VSOCK-DBG] stream={} received FIN from peer",
                        self.stream_id
                    );
                    Err(anyhow!("Stream closed by peer"))
                }
                FrameType::Rst => {
                    trace!(
                        "[VSOCK-DBG] stream={} received RST from peer",
                        self.stream_id
                    );
                    Err(anyhow!("Stream reset by peer"))
                }
                _ => Ok(frame.payload),
            },
            None => {
                trace!(
                    "[VSOCK-DBG] stream={} incoming channel closed (IO loop ended)",
                    self.stream_id
                );
                Err(anyhow!("Stream channel closed"))
            }
        }
    }

    async fn write(&mut self, data: &[u8]) -> Result<()> {
        trace!(
            "[VSOCK-DBG] stream={} write {} bytes, channel_cap={}/{}",
            self.stream_id,
            data.len(),
            self.outgoing_tx.capacity(),
            OUTGOING_BUFFER_SIZE
        );
        send_data_frames(&self.outgoing_tx, self.stream_id, data).await
    }

    async fn close(&mut self) -> Result<()> {
        if self.closed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        trace!("[VSOCK-DBG] stream={} sending FIN", self.stream_id);
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

pub struct StreamMultiplexer {
    streams: Arc<RwLock<HashMap<u32, mpsc::Sender<Frame>>>>,
    outgoing_tx: mpsc::Sender<Frame>,
    next_stream_id: Arc<Mutex<u32>>,
}

impl StreamMultiplexer {
    pub fn new(
        physical_stream: Box<dyn VsockStream>,
        incoming_handler: Option<IncomingStreamHandler>,
    ) -> Self {
        let (outgoing_tx, outgoing_rx) = mpsc::channel(OUTGOING_BUFFER_SIZE);
        let streams = Arc::new(RwLock::new(HashMap::new()));
        // Host uses odd stream IDs (1, 3, 5...) to avoid collision with
        // VM-agent-initiated even IDs (2, 4, 6...).
        let next_stream_id = Arc::new(Mutex::new(1u32));

        let multiplexer = Self {
            streams: streams.clone(),
            outgoing_tx: outgoing_tx.clone(),
            next_stream_id,
        };

        tokio::spawn(Self::run_io_loop(
            physical_stream,
            streams,
            outgoing_rx,
            incoming_handler,
            outgoing_tx,
        ));

        multiplexer
    }

    pub async fn create_stream(&self, target_port: u32) -> Result<VirtualStream> {
        let mut id_guard = self.next_stream_id.lock().await;
        let stream_id = *id_guard;
        *id_guard += 2; // Step by 2 to keep odd IDs for host-initiated streams
        drop(id_guard);

        let (incoming_tx, incoming_rx) = mpsc::channel(CHANNEL_BUFFER_SIZE);
        {
            let mut streams_guard = self.streams.write().await;
            streams_guard.insert(stream_id, incoming_tx);
        }

        let connect_frame = Frame::connect(stream_id, target_port);
        self.outgoing_tx
            .send(connect_frame)
            .await
            .map_err(|_| anyhow!("Failed to send connect frame"))?;

        Ok(VirtualStream::new(
            stream_id,
            None,
            incoming_rx,
            self.outgoing_tx.clone(),
        ))
    }

    async fn run_io_loop(
        mut physical: Box<dyn VsockStream>,
        streams: Arc<RwLock<HashMap<u32, mpsc::Sender<Frame>>>>,
        mut outgoing_rx: mpsc::Receiver<Frame>,
        incoming_handler: Option<IncomingStreamHandler>,
        outgoing_tx: mpsc::Sender<Frame>,
    ) {
        #[cfg(target_os = "macos")]
        let mut buf = BytesMut::with_capacity(65536);
        #[cfg(not(target_os = "macos"))]
        let mut buf = BytesMut::with_capacity(4096);
        let mut total_read: u64 = 0;
        let mut total_written: u64 = 0;

        debug!("[HOST-MUX] IO loop started");

        loop {
            tokio::select! {
                read_result = physical.read() => {
                    match read_result {
                        Ok(data) => {
                            total_read += data.len() as u64;
                            trace!("[HOST-MUX] physical read {} bytes (total: {}), buf_len={}",
                                data.len(), total_read, buf.len() + data.len());
                            buf.extend_from_slice(&data);
                            loop {
                                if buf.len() < Frame::HEADER_SIZE { break; }
                                if FrameType::try_from(buf[4]).is_ok() {
                                    let length = u32::from_be_bytes(buf[5..9].try_into().unwrap()) as usize;
                                    let total_len = Frame::HEADER_SIZE + length;
                                    if buf.len() >= total_len {
                                        let frame_bytes = buf.split_to(total_len).freeze();
                                        match Frame::decode(frame_bytes) {
                                            Ok(frame) => {
                                                // WindowUpdate from vm-agent: credit tracking removed,
                                                // skip silently so it doesn't hit the "unknown stream" warn.
                                                if frame.frame_type == FrameType::WindowUpdate {
                                                    continue;
                                                }

                                                let tx = {
                                                    let streams_guard = streams.read().await;
                                                    streams_guard.get(&frame.stream_id).cloned()
                                                };
                                                if let Some(tx) = tx {
                                                    let is_terminal = frame.frame_type == FrameType::Fin
                                                        || frame.frame_type == FrameType::Rst;
                                                    let sid = frame.stream_id;
                                                    // Timeout-bounded send to prevent a slow consumer
                                                    // from blocking the entire IO loop (head-of-line
                                                    // blocking). A 5s timeout is generous enough for
                                                    // normal backpressure but catches hung handlers.
                                                    match tokio::time::timeout(
                                                        std::time::Duration::from_secs(5),
                                                        tx.send(frame),
                                                    ).await {
                                                        Ok(Ok(())) => {
                                                            if is_terminal {
                                                                let mut streams_guard = streams.write().await;
                                                                streams_guard.remove(&sid);
                                                            }
                                                        }
                                                        Ok(Err(_)) => {
                                                            let mut streams_guard = streams.write().await;
                                                            streams_guard.remove(&sid);
                                                        }
                                                        Err(_) => {
                                                            warn!("[HOST-MUX] stream {} send timed out (5s), resetting", sid);
                                                            let mut streams_guard = streams.write().await;
                                                            streams_guard.remove(&sid);
                                                            let _ = outgoing_tx.try_send(Frame::rst(sid));
                                                        }
                                                    }
                                                } else if frame.frame_type == FrameType::ReverseRequest || frame.frame_type == FrameType::DnsRequest || frame.frame_type == FrameType::HostConnect {
                                                    if let Some(handler) = &incoming_handler {
                                                        let (incoming_tx, incoming_rx) = mpsc::channel(CHANNEL_BUFFER_SIZE);
                                                        {
                                                            let mut streams_guard = streams.write().await;
                                                            streams_guard.insert(frame.stream_id, incoming_tx.clone());
                                                        }
                                                        let stream = VirtualStream::new(
                                                            frame.stream_id,
                                                            Some(frame.frame_type),
                                                            incoming_rx,
                                                            outgoing_tx.clone(),
                                                        );
                                                        let sid = frame.stream_id;
                                                        match handler.try_send(stream) {
                                                            Ok(()) => {
                                                                if incoming_tx.try_send(frame).is_err() {
                                                                    warn!("[HOST-MUX] stream {} initial frame channel full", sid);
                                                                }
                                                            }
                                                            Err(tokio::sync::mpsc::error::TrySendError::Full(stream))
                                                            | Err(tokio::sync::mpsc::error::TrySendError::Closed(stream)) => {
                                                                stream.suppress_drop_close();
                                                                error!("[HOST-MUX] handler channel full, rejecting stream {}", sid);
                                                                {
                                                                    let mut streams_guard = streams.write().await;
                                                                    streams_guard.remove(&sid);
                                                                }
                                                                let _ = outgoing_tx.try_send(Frame::rst(sid));
                                                            }
                                                        }
                                                    }
                                                } else {
                                                    warn!("[HOST-MUX] frame for unknown stream {}", frame.stream_id);
                                                }
                                            }
                                            Err(e) => {
                                                error!("[HOST-MUX] frame decode error: {}", e);
                                                return;
                                            }
                                        }
                                    } else {
                                        break;
                                    }
                                } else {
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            error!("[HOST-MUX] physical read ended: {} (total_read={} total_written={} active_streams={})",
                                e, total_read, total_written, streams.read().await.len());
                            break;
                        }
                    }
                }
                Some(frame) = outgoing_rx.recv() => {
                    // --- macOS (Apple VZ): moderate batching (up to 64KB) ---
                    // With enlarged relay socket buffers (1MB/4MB), moderate
                    // batching is safe and dramatically reduces syscall overhead.
                    // Previous single-frame writes were needed when relay buffers
                    // were tiny (~8KB default), causing per-frame write stalls.
                    #[cfg(target_os = "macos")]
                    {
                        const MACOS_MAX_BATCH_BYTES: usize = 64 * 1024;
                        let mut combined = BytesMut::new();
                        combined.extend_from_slice(&frame.encode());
                        while combined.len() < MACOS_MAX_BATCH_BYTES {
                            match outgoing_rx.try_recv() {
                                Ok(f) => combined.extend_from_slice(&f.encode()),
                                Err(_) => break,
                            }
                        }
                        trace!("[HOST-MUX] physical write {} bytes (batch)", combined.len());
                        if let Err(e) = physical.write(&combined).await {
                            error!("[HOST-MUX] physical write error: {} ({} bytes, total_written={})",
                                e, combined.len(), total_written);
                            break;
                        }
                        total_written += combined.len() as u64;
                    }

                    // --- Linux / Windows (QEMU): batched writes, no pacing ---
                    #[cfg(not(target_os = "macos"))]
                    {
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
                            error!("[HOST-MUX] physical write error: {}", e);
                            break;
                        }
                        total_written += combined.len() as u64;
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
            let stream_ids: Vec<u32> = streams_guard.keys().copied().collect();
            error!(
                "[HOST-MUX] IO loop EXITED — total_read={} total_written={} active_streams={} stream_ids={:?}",
                total_read, total_written, remaining, stream_ids
            );
            if remaining > 0 {
                streams_guard.clear();
                debug!(
                    "[HOST-MUX] IO loop exited, cleaned up {} active streams",
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
        let stream = VirtualStream::new(7, None, incoming_rx, outgoing_tx);
        let writer = stream.writer();

        writer.close().await.unwrap();
        drop(stream);

        let frame = outgoing_rx.recv().await.unwrap();
        assert_eq!(frame.stream_id, 7);
        assert_eq!(frame.frame_type, FrameType::Fin);
        assert!(outgoing_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn suppressed_drop_does_not_send_fin() {
        let (outgoing_tx, mut outgoing_rx) = mpsc::channel(8);
        let (_incoming_tx, incoming_rx) = mpsc::channel(1);
        let stream =
            VirtualStream::new(9, Some(FrameType::ReverseRequest), incoming_rx, outgoing_tx);

        stream.suppress_drop_close();
        drop(stream);

        assert!(outgoing_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn writer_splits_data_frames_for_platform_policy() {
        let (outgoing_tx, mut outgoing_rx) = mpsc::channel(8);
        let (_incoming_tx, incoming_rx) = mpsc::channel(1);
        let stream = VirtualStream::new(11, None, incoming_rx, outgoing_tx);
        let writer = stream.writer();

        // Use 40000 bytes to exceed MACOS_MAX_DATA_FRAME_BYTES (32768)
        // so macOS actually splits into two frames
        writer.write(&vec![7u8; 40000]).await.unwrap();

        #[cfg(target_os = "macos")]
        let expected = vec![32768usize, 7232usize]; // 32768 + 7232 = 40000
        #[cfg(not(target_os = "macos"))]
        let expected = vec![40000usize];

        for expected_len in expected {
            let frame = outgoing_rx.recv().await.unwrap();
            assert_eq!(frame.stream_id, 11);
            assert_eq!(frame.frame_type, FrameType::Data);
            assert_eq!(frame.payload.len(), expected_len);
        }

        assert!(outgoing_rx.try_recv().is_err());
    }
}
