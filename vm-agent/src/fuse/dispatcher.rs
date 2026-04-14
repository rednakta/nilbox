//! Request dispatcher — correlates async requests with responses.

use super::protocol::*;
use anyhow::Result;
use bytes::BytesMut;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::time::timeout;
use tracing::{debug, error, warn};

use crate::vsock::stream::{VirtualStream, VirtualStreamWriter};
use crate::vsock::VsockStream as _;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const READ_WRITE_TIMEOUT: Duration = Duration::from_secs(60);
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);

struct PendingRequest {
    response_tx: oneshot::Sender<Result<FuseResponse, DispatcherError>>,
}

#[derive(Debug)]
pub enum DispatcherError {
    Timeout,
    ChannelClosed,
    TransportError(String),
    ProtocolError(String),
    StatusError(StatusCode),
}

impl std::fmt::Display for DispatcherError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DispatcherError::Timeout => write!(f, "Request timed out"),
            DispatcherError::ChannelClosed => write!(f, "Channel closed"),
            DispatcherError::TransportError(msg) => write!(f, "Transport error: {}", msg),
            DispatcherError::ProtocolError(msg) => write!(f, "Protocol error: {}", msg),
            DispatcherError::StatusError(code) => write!(f, "Status error: {:?}", code),
        }
    }
}

impl std::error::Error for DispatcherError {}

pub type NotificationHandler = Box<dyn Fn(FuseNotification) + Send + Sync>;

pub struct RequestDispatcher {
    pending: Arc<Mutex<HashMap<RequestId, PendingRequest>>>,
    next_id: AtomicU64,
    writer: Arc<Mutex<VirtualStreamWriter>>,
    notification_handler: Arc<Mutex<Option<NotificationHandler>>>,
}

impl RequestDispatcher {
    pub fn new(writer: VirtualStreamWriter) -> Self {
        Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(1),
            writer: Arc::new(Mutex::new(writer)),
            notification_handler: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn set_notification_handler(&self, handler: NotificationHandler) {
        let mut h = self.notification_handler.lock().await;
        *h = Some(handler);
    }

    fn allocate_id(&self) -> RequestId {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    fn timeout_for_op(op: OpType) -> Duration {
        match op {
            OpType::Read | OpType::Write => READ_WRITE_TIMEOUT,
            OpType::Open => OPEN_TIMEOUT,
            _ => DEFAULT_TIMEOUT,
        }
    }

    async fn send_request(&self, request: FuseRequest) -> Result<FuseResponse, DispatcherError> {
        let request_id = request.request_id();
        let op_type = request.op_type();
        let timeout_duration = Self::timeout_for_op(op_type);

        let (tx, rx) = oneshot::channel();

        {
            let mut pending = self.pending.lock().await;
            pending.insert(request_id, PendingRequest { response_tx: tx });
        }

        // Encode and send
        let encoded = encode_request(&request);
        {
            let writer = self.writer.lock().await;
            if let Err(e) = writer.write(&encoded).await {
                let mut pending = self.pending.lock().await;
                pending.remove(&request_id);
                return Err(DispatcherError::TransportError(e.to_string()));
            }
        }

        match timeout(timeout_duration, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => {
                let mut pending = self.pending.lock().await;
                pending.remove(&request_id);
                Err(DispatcherError::ChannelClosed)
            }
            Err(_) => {
                let mut pending = self.pending.lock().await;
                pending.remove(&request_id);
                warn!("Request {} timed out after {:?}", request_id, timeout_duration);
                Err(DispatcherError::Timeout)
            }
        }
    }

    /// Handle an incoming message from transport
    pub async fn handle_message(&self, message: FuseMessage) {
        match message {
            FuseMessage::Response(response) => {
                let request_id = response.request_id();
                let mut pending = self.pending.lock().await;
                if let Some(pending_req) = pending.remove(&request_id) {
                    let _ = pending_req.response_tx.send(Ok(response));
                } else {
                    warn!("Received response for unknown request: {}", request_id);
                }
            }
            FuseMessage::Notification(notification) => {
                let handler = self.notification_handler.lock().await;
                if let Some(ref h) = *handler {
                    h(notification);
                } else {
                    debug!("Notification received but no handler set");
                }
            }
        }
    }

    /// Start background reader loop
    pub fn start_reader(
        self: Arc<Self>,
        mut stream: VirtualStream,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut buf = BytesMut::with_capacity(4096);
            loop {
                match stream.read().await {
                    Ok(data) => {
                        buf.extend_from_slice(&data);
                        loop {
                            match decode_response(&mut buf) {
                                Ok(Some(message)) => {
                                    self.handle_message(message).await;
                                }
                                Ok(None) => break,
                                Err(e) => {
                                    error!("Dispatcher decode error: {}", e);
                                    return;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        debug!("Dispatcher reader stream closed: {}", e);
                        break;
                    }
                }
            }
            debug!("Dispatcher reader loop terminated");
        })
    }

    // ── Convenience methods ──────────────────────────────────────

    pub async fn open(&self, path: String, flags: u32, mode: u32) -> Result<u64, DispatcherError> {
        let request = FuseRequest::Open { request_id: self.allocate_id(), flags, mode, path };
        match self.send_request(request).await? {
            FuseResponse::Open { status, fd, .. } if status == StatusCode::Ok => Ok(fd),
            FuseResponse::Open { status, .. } => Err(DispatcherError::StatusError(status)),
            _ => Err(DispatcherError::ProtocolError("Unexpected response".into())),
        }
    }

    pub async fn read(&self, fd: u64, offset: u64, size: u32) -> Result<Vec<u8>, DispatcherError> {
        let request = FuseRequest::Read { request_id: self.allocate_id(), fd, offset, size };
        match self.send_request(request).await? {
            FuseResponse::Read { status, data, .. } if status == StatusCode::Ok => Ok(data),
            FuseResponse::Read { status, .. } => Err(DispatcherError::StatusError(status)),
            _ => Err(DispatcherError::ProtocolError("Unexpected response".into())),
        }
    }

    pub async fn write(&self, fd: u64, offset: u64, data: Vec<u8>) -> Result<u32, DispatcherError> {
        let request = FuseRequest::Write { request_id: self.allocate_id(), fd, offset, data };
        match self.send_request(request).await? {
            FuseResponse::Write { status, written, .. } if status == StatusCode::Ok => Ok(written),
            FuseResponse::Write { status, .. } => Err(DispatcherError::StatusError(status)),
            _ => Err(DispatcherError::ProtocolError("Unexpected response".into())),
        }
    }

    pub async fn close(&self, fd: u64) -> Result<(), DispatcherError> {
        let request = FuseRequest::Close { request_id: self.allocate_id(), fd };
        match self.send_request(request).await? {
            FuseResponse::Close { status, .. } if status == StatusCode::Ok => Ok(()),
            FuseResponse::Close { status, .. } => Err(DispatcherError::StatusError(status)),
            _ => Err(DispatcherError::ProtocolError("Unexpected response".into())),
        }
    }

    pub async fn readdir(&self, fd: u64, offset: u64, max_entries: u32) -> Result<Vec<DirEntry>, DispatcherError> {
        let request = FuseRequest::Readdir { request_id: self.allocate_id(), fd, offset, max_entries };
        match self.send_request(request).await? {
            FuseResponse::Readdir { status, entries, .. } if status == StatusCode::Ok => Ok(entries),
            FuseResponse::Readdir { status, .. } => Err(DispatcherError::StatusError(status)),
            _ => Err(DispatcherError::ProtocolError("Unexpected response".into())),
        }
    }

    pub async fn stat(&self, path: String) -> Result<FuseResponse, DispatcherError> {
        let request = FuseRequest::Stat { request_id: self.allocate_id(), path };
        match self.send_request(request).await? {
            resp @ FuseResponse::Stat { status, .. } if status == StatusCode::Ok => Ok(resp),
            FuseResponse::Stat { status, .. } => Err(DispatcherError::StatusError(status)),
            _ => Err(DispatcherError::ProtocolError("Unexpected response".into())),
        }
    }

    pub async fn mkdir(&self, path: String, mode: u32) -> Result<(), DispatcherError> {
        let request = FuseRequest::Mkdir { request_id: self.allocate_id(), mode, path };
        match self.send_request(request).await? {
            FuseResponse::Mkdir { status, .. } if status == StatusCode::Ok => Ok(()),
            FuseResponse::Mkdir { status, .. } => Err(DispatcherError::StatusError(status)),
            _ => Err(DispatcherError::ProtocolError("Unexpected response".into())),
        }
    }

    pub async fn remove(&self, path: String, is_dir: bool) -> Result<(), DispatcherError> {
        let request = FuseRequest::Remove { request_id: self.allocate_id(), is_dir, path };
        match self.send_request(request).await? {
            FuseResponse::Remove { status, .. } if status == StatusCode::Ok => Ok(()),
            FuseResponse::Remove { status, .. } => Err(DispatcherError::StatusError(status)),
            _ => Err(DispatcherError::ProtocolError("Unexpected response".into())),
        }
    }

    pub async fn ping(&self) -> Result<(), DispatcherError> {
        let request = FuseRequest::Ping { request_id: self.allocate_id() };
        match self.send_request(request).await? {
            FuseResponse::Pong { .. } => Ok(()),
            _ => Err(DispatcherError::ProtocolError("Unexpected response".into())),
        }
    }
}
