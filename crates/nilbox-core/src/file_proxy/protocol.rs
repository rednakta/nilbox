//! FUSE proxy protocol — binary message format for VSOCK transport.
//!
//! Ported from tauri-app-pipe/src-tauri/src/fuse_proxy/protocol.rs.
//! Uses standalone encode/decode functions instead of tokio_util codec,
//! because VirtualStream already provides frame-level I/O.

use bytes::{Buf, BufMut, BytesMut};

/// Magic number for FUSE protocol messages ("FUSE" in Big-endian)
pub const MAGIC: u32 = 0x46555345;
pub const HEADER_SIZE: usize = 10;
pub const MAX_PAYLOAD_SIZE: u32 = 0x01000000; // 16MB

/// VSOCK port range for FUSE file proxies (one port per mapping, max 20)
pub const FUSE_PORT_BASE: u32 = 9500;
pub const FUSE_PORT_MAX: u32 = 9519;
pub const MAX_FILE_MAPPINGS: usize = 20;

pub fn fuse_port_for_mapping(mapping_id: i64) -> u32 {
    FUSE_PORT_BASE + (mapping_id as u32 % MAX_FILE_MAPPINGS as u32)
}

pub fn is_fuse_port(port: u32) -> bool {
    port >= FUSE_PORT_BASE && port <= FUSE_PORT_MAX
}

/// Operation types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum OpType {
    Open = 0x0001,
    Read = 0x0002,
    Write = 0x0003,
    Close = 0x0004,
    Readdir = 0x0005,
    Stat = 0x0006,
    Mkdir = 0x0007,
    Remove = 0x0008,
    Rename = 0x0009,
    Truncate = 0x000A,
    PathChanged = 0x0010,
    PathQuery = 0x0011,
    InvalidateCache = 0x0012,
    PathPending = 0x0013,
    PathReady = 0x0014,
    Ping = 0x00F0,
    Pong = 0x00F1,
    Shutdown = 0x00FF,
}

impl TryFrom<u16> for OpType {
    type Error = anyhow::Error;
    fn try_from(v: u16) -> Result<Self, Self::Error> {
        match v {
            0x0001 => Ok(OpType::Open),
            0x0002 => Ok(OpType::Read),
            0x0003 => Ok(OpType::Write),
            0x0004 => Ok(OpType::Close),
            0x0005 => Ok(OpType::Readdir),
            0x0006 => Ok(OpType::Stat),
            0x0007 => Ok(OpType::Mkdir),
            0x0008 => Ok(OpType::Remove),
            0x0009 => Ok(OpType::Rename),
            0x000A => Ok(OpType::Truncate),
            0x0010 => Ok(OpType::PathChanged),
            0x0011 => Ok(OpType::PathQuery),
            0x0012 => Ok(OpType::InvalidateCache),
            0x0013 => Ok(OpType::PathPending),
            0x0014 => Ok(OpType::PathReady),
            0x00F0 => Ok(OpType::Ping),
            0x00F1 => Ok(OpType::Pong),
            0x00FF => Ok(OpType::Shutdown),
            _ => Err(anyhow::anyhow!("Invalid OpType: 0x{:04X}", v)),
        }
    }
}

/// Response status codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum StatusCode {
    Ok = 0x0000,
    ErrNoent = 0x0001,
    ErrExist = 0x0002,
    ErrAccess = 0x0003,
    ErrNotdir = 0x0004,
    ErrIsdir = 0x0005,
    ErrNotempty = 0x0006,
    ErrNospc = 0x0007,
    ErrIo = 0x0008,
    ErrInval = 0x0009,
    ErrNametoolong = 0x000A,
    ErrBusy = 0x0010,
    ErrPathchange = 0x0011,
    ErrSandboxed = 0x0012,
    ErrUnknown = 0x00FF,
}

impl From<std::io::ErrorKind> for StatusCode {
    fn from(kind: std::io::ErrorKind) -> Self {
        match kind {
            std::io::ErrorKind::NotFound => StatusCode::ErrNoent,
            std::io::ErrorKind::AlreadyExists => StatusCode::ErrExist,
            std::io::ErrorKind::PermissionDenied => StatusCode::ErrAccess,
            std::io::ErrorKind::InvalidInput => StatusCode::ErrInval,
            _ => StatusCode::ErrIo,
        }
    }
}

/// File types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FileType {
    Regular = 0x01,
    Directory = 0x02,
    Symlink = 0x03,
    Other = 0x04,
}

/// Request ID type
pub type RequestId = u64;

/// Incoming request from guest
#[derive(Debug)]
pub enum FuseRequest {
    Open { request_id: RequestId, flags: u32, mode: u32, path: String },
    Read { request_id: RequestId, fd: u64, offset: u64, size: u32 },
    Write { request_id: RequestId, fd: u64, offset: u64, data: BytesMut },
    Close { request_id: RequestId, fd: u64 },
    Readdir { request_id: RequestId, fd: u64, offset: u64, max_entries: u32 },
    Stat { request_id: RequestId, path: String },
    Mkdir { request_id: RequestId, mode: u32, path: String },
    Remove { request_id: RequestId, is_dir: bool, path: String },
    PathQuery { request_id: RequestId },
    Ping { request_id: RequestId },
}

impl FuseRequest {
    pub fn request_id(&self) -> RequestId {
        match self {
            FuseRequest::Open { request_id, .. } => *request_id,
            FuseRequest::Read { request_id, .. } => *request_id,
            FuseRequest::Write { request_id, .. } => *request_id,
            FuseRequest::Close { request_id, .. } => *request_id,
            FuseRequest::Readdir { request_id, .. } => *request_id,
            FuseRequest::Stat { request_id, .. } => *request_id,
            FuseRequest::Mkdir { request_id, .. } => *request_id,
            FuseRequest::Remove { request_id, .. } => *request_id,
            FuseRequest::PathQuery { request_id } => *request_id,
            FuseRequest::Ping { request_id } => *request_id,
        }
    }
}

/// Outgoing response/notification to guest
#[derive(Debug)]
pub enum FuseResponse {
    Open { request_id: RequestId, status: StatusCode, fd: u64 },
    Read { request_id: RequestId, status: StatusCode, data: BytesMut },
    Write { request_id: RequestId, status: StatusCode, written: u32 },
    Close { request_id: RequestId, status: StatusCode },
    Readdir { request_id: RequestId, status: StatusCode, entries: Vec<DirEntry> },
    Stat { request_id: RequestId, status: StatusCode, attr: FileAttr },
    Mkdir { request_id: RequestId, status: StatusCode },
    Remove { request_id: RequestId, status: StatusCode },
    PathQuery { request_id: RequestId, status: StatusCode, state: u8, path: String },
    Pong { request_id: RequestId },
    // Host-initiated notifications
    PathChanged { old_path: String, new_path: String },
    PathPending { pending_handles: u32, timeout_sec: u32 },
    PathReady,
    InvalidateCache,
}

#[derive(Debug, Clone)]
pub struct DirEntry {
    pub file_type: FileType,
    pub name: String,
}

#[derive(Debug, Clone, Default)]
pub struct FileAttr {
    pub file_type: u8,
    pub mode: u32,
    pub size: u64,
    pub mtime: u64,
    pub atime: u64,
    pub ctime: u64,
}

/// Decode a FuseRequest from a buffer. Returns None if not enough data.
pub fn decode_request(src: &mut BytesMut) -> anyhow::Result<Option<FuseRequest>> {
    if src.len() < HEADER_SIZE {
        return Ok(None);
    }

    // Peek header
    let magic = u32::from_be_bytes([src[0], src[1], src[2], src[3]]);
    if magic != MAGIC {
        return Err(anyhow::anyhow!("Invalid magic: 0x{:08X}", magic));
    }

    let op_type = u16::from_le_bytes([src[4], src[5]]);
    let length = u32::from_le_bytes([src[6], src[7], src[8], src[9]]) as usize;

    if length > MAX_PAYLOAD_SIZE as usize {
        return Err(anyhow::anyhow!("Payload too large: {}", length));
    }

    if src.len() < HEADER_SIZE + length {
        src.reserve(HEADER_SIZE + length - src.len());
        return Ok(None);
    }

    // Consume header
    src.advance(HEADER_SIZE);
    let mut payload = src.split_to(length);

    let op = OpType::try_from(op_type)?;
    let request = match op {
        OpType::Open => {
            let request_id = payload.get_u64_le();
            let flags = payload.get_u32_le();
            let mode = payload.get_u32_le();
            let path_len = payload.get_u16_le() as usize;
            let path = String::from_utf8(payload.split_to(path_len).to_vec())?;
            FuseRequest::Open { request_id, flags, mode, path }
        }
        OpType::Read => {
            let request_id = payload.get_u64_le();
            let fd = payload.get_u64_le();
            let offset = payload.get_u64_le();
            let size = payload.get_u32_le();
            FuseRequest::Read { request_id, fd, offset, size }
        }
        OpType::Write => {
            let request_id = payload.get_u64_le();
            let fd = payload.get_u64_le();
            let offset = payload.get_u64_le();
            let size = payload.get_u32_le() as usize;
            let data = payload.split_to(size);
            FuseRequest::Write { request_id, fd, offset, data }
        }
        OpType::Close => {
            let request_id = payload.get_u64_le();
            let fd = payload.get_u64_le();
            FuseRequest::Close { request_id, fd }
        }
        OpType::Readdir => {
            let request_id = payload.get_u64_le();
            let fd = payload.get_u64_le();
            let offset = payload.get_u64_le();
            let max_entries = payload.get_u32_le();
            FuseRequest::Readdir { request_id, fd, offset, max_entries }
        }
        OpType::Stat => {
            let request_id = payload.get_u64_le();
            let path_len = payload.get_u16_le() as usize;
            let path = String::from_utf8(payload.split_to(path_len).to_vec())?;
            FuseRequest::Stat { request_id, path }
        }
        OpType::Mkdir => {
            let request_id = payload.get_u64_le();
            let mode = payload.get_u32_le();
            let path_len = payload.get_u16_le() as usize;
            let path = String::from_utf8(payload.split_to(path_len).to_vec())?;
            FuseRequest::Mkdir { request_id, mode, path }
        }
        OpType::Remove => {
            let request_id = payload.get_u64_le();
            let is_dir = payload.get_u8() != 0;
            let path_len = payload.get_u16_le() as usize;
            let path = String::from_utf8(payload.split_to(path_len).to_vec())?;
            FuseRequest::Remove { request_id, is_dir, path }
        }
        OpType::PathQuery => {
            let request_id = payload.get_u64_le();
            FuseRequest::PathQuery { request_id }
        }
        OpType::Ping => {
            let request_id = payload.get_u64_le();
            FuseRequest::Ping { request_id }
        }
        _ => return Err(anyhow::anyhow!("Unexpected request type: {:?}", op)),
    };

    Ok(Some(request))
}

/// Encode a FuseResponse into a BytesMut ready for VirtualStream.write().
pub fn encode_response(resp: &FuseResponse) -> BytesMut {
    let mut payload = BytesMut::new();
    let op_type: u16;

    match resp {
        FuseResponse::Open { request_id, status, fd } => {
            op_type = OpType::Open as u16;
            payload.put_u64_le(*request_id);
            payload.put_u16_le(*status as u16);
            payload.put_u64_le(*fd);
        }
        FuseResponse::Read { request_id, status, data } => {
            op_type = OpType::Read as u16;
            payload.put_u64_le(*request_id);
            payload.put_u16_le(*status as u16);
            payload.put_u32_le(data.len() as u32);
            payload.put(&data[..]);
        }
        FuseResponse::Write { request_id, status, written } => {
            op_type = OpType::Write as u16;
            payload.put_u64_le(*request_id);
            payload.put_u16_le(*status as u16);
            payload.put_u32_le(*written);
        }
        FuseResponse::Close { request_id, status } => {
            op_type = OpType::Close as u16;
            payload.put_u64_le(*request_id);
            payload.put_u16_le(*status as u16);
        }
        FuseResponse::Readdir { request_id, status, entries } => {
            op_type = OpType::Readdir as u16;
            payload.put_u64_le(*request_id);
            payload.put_u16_le(*status as u16);
            payload.put_u32_le(entries.len() as u32);
            for entry in entries {
                payload.put_u8(entry.file_type as u8);
                let name_bytes = entry.name.as_bytes();
                payload.put_u16_le(name_bytes.len() as u16);
                payload.put_slice(name_bytes);
            }
        }
        FuseResponse::Stat { request_id, status, attr } => {
            op_type = OpType::Stat as u16;
            payload.put_u64_le(*request_id);
            payload.put_u16_le(*status as u16);
            payload.put_u8(attr.file_type);
            payload.put_u32_le(attr.mode);
            payload.put_u64_le(attr.size);
            payload.put_u64_le(attr.mtime);
            payload.put_u64_le(attr.atime);
            payload.put_u64_le(attr.ctime);
        }
        FuseResponse::Mkdir { request_id, status } => {
            op_type = OpType::Mkdir as u16;
            payload.put_u64_le(*request_id);
            payload.put_u16_le(*status as u16);
        }
        FuseResponse::Remove { request_id, status } => {
            op_type = OpType::Remove as u16;
            payload.put_u64_le(*request_id);
            payload.put_u16_le(*status as u16);
        }
        FuseResponse::PathQuery { request_id, status, state, path } => {
            op_type = OpType::PathQuery as u16;
            payload.put_u64_le(*request_id);
            payload.put_u16_le(*status as u16);
            payload.put_u8(*state);
            let path_bytes = path.as_bytes();
            payload.put_u16_le(path_bytes.len() as u16);
            payload.put_slice(path_bytes);
        }
        FuseResponse::Pong { request_id } => {
            op_type = OpType::Pong as u16;
            payload.put_u64_le(*request_id);
        }
        FuseResponse::PathChanged { old_path, new_path } => {
            op_type = OpType::PathChanged as u16;
            let old_bytes = old_path.as_bytes();
            let new_bytes = new_path.as_bytes();
            payload.put_u16_le(old_bytes.len() as u16);
            payload.put_slice(old_bytes);
            payload.put_u16_le(new_bytes.len() as u16);
            payload.put_slice(new_bytes);
        }
        FuseResponse::PathPending { pending_handles, timeout_sec } => {
            op_type = OpType::PathPending as u16;
            payload.put_u32_le(*pending_handles);
            payload.put_u32_le(*timeout_sec);
        }
        FuseResponse::PathReady => {
            op_type = OpType::PathReady as u16;
        }
        FuseResponse::InvalidateCache => {
            op_type = OpType::InvalidateCache as u16;
        }
    }

    // Build frame: header + payload
    let mut dst = BytesMut::with_capacity(HEADER_SIZE + payload.len());
    dst.put_u32(MAGIC); // Big-endian
    dst.put_u16_le(op_type);
    dst.put_u32_le(payload.len() as u32);
    dst.put(payload);
    dst
}
