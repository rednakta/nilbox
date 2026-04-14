//! FUSE proxy protocol — guest-side encode/decode.
//!
//! Mirror of nilbox-core's file_proxy/protocol.rs.
//! Guest encodes requests, decodes responses/notifications.

use bytes::{Buf, BufMut, BytesMut};

pub const MAGIC: u32 = 0x46555345;
pub const HEADER_SIZE: usize = 10;
pub const MAX_PAYLOAD_SIZE: u32 = 0x01000000; // 16MB
pub const FUSE_PORT_BASE: u32 = 9500;
pub const FUSE_PORT_MAX: u32 = 9519;

pub fn is_fuse_port(port: u32) -> bool {
    port >= FUSE_PORT_BASE && port <= FUSE_PORT_MAX
}

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

impl TryFrom<u16> for StatusCode {
    type Error = anyhow::Error;
    fn try_from(v: u16) -> Result<Self, Self::Error> {
        match v {
            0x0000 => Ok(StatusCode::Ok),
            0x0001 => Ok(StatusCode::ErrNoent),
            0x0002 => Ok(StatusCode::ErrExist),
            0x0003 => Ok(StatusCode::ErrAccess),
            0x0004 => Ok(StatusCode::ErrNotdir),
            0x0005 => Ok(StatusCode::ErrIsdir),
            0x0006 => Ok(StatusCode::ErrNotempty),
            0x0007 => Ok(StatusCode::ErrNospc),
            0x0008 => Ok(StatusCode::ErrIo),
            0x0009 => Ok(StatusCode::ErrInval),
            0x000A => Ok(StatusCode::ErrNametoolong),
            0x0010 => Ok(StatusCode::ErrBusy),
            0x0011 => Ok(StatusCode::ErrPathchange),
            0x0012 => Ok(StatusCode::ErrSandboxed),
            0x00FF => Ok(StatusCode::ErrUnknown),
            _ => Err(anyhow::anyhow!("Invalid StatusCode: 0x{:04X}", v)),
        }
    }
}

/// Map StatusCode to libc errno
pub fn status_to_errno(status: StatusCode) -> i32 {
    match status {
        StatusCode::Ok => 0,
        StatusCode::ErrNoent => libc::ENOENT,
        StatusCode::ErrExist => libc::EEXIST,
        StatusCode::ErrAccess => libc::EACCES,
        StatusCode::ErrNotdir => libc::ENOTDIR,
        StatusCode::ErrIsdir => libc::EISDIR,
        StatusCode::ErrNotempty => libc::ENOTEMPTY,
        StatusCode::ErrNospc => libc::ENOSPC,
        StatusCode::ErrIo => libc::EIO,
        StatusCode::ErrInval => libc::EINVAL,
        StatusCode::ErrNametoolong => libc::ENAMETOOLONG,
        StatusCode::ErrBusy => libc::EBUSY,
        StatusCode::ErrPathchange => libc::EBUSY,
        StatusCode::ErrSandboxed => libc::EACCES,
        StatusCode::ErrUnknown => libc::EIO,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FileType {
    Regular = 0x01,
    Directory = 0x02,
    Symlink = 0x03,
    Other = 0x04,
}

impl TryFrom<u8> for FileType {
    type Error = anyhow::Error;
    fn try_from(v: u8) -> Result<Self, Self::Error> {
        match v {
            0x01 => Ok(FileType::Regular),
            0x02 => Ok(FileType::Directory),
            0x03 => Ok(FileType::Symlink),
            0x04 => Ok(FileType::Other),
            _ => Err(anyhow::anyhow!("Invalid FileType: 0x{:02X}", v)),
        }
    }
}

pub type RequestId = u64;

#[derive(Debug, Clone)]
pub struct DirEntry {
    pub file_type: FileType,
    pub name: String,
}

/// Guest-originating request
#[derive(Debug)]
pub enum FuseRequest {
    Open { request_id: RequestId, flags: u32, mode: u32, path: String },
    Read { request_id: RequestId, fd: u64, offset: u64, size: u32 },
    Write { request_id: RequestId, fd: u64, offset: u64, data: Vec<u8> },
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

    pub fn op_type(&self) -> OpType {
        match self {
            FuseRequest::Open { .. } => OpType::Open,
            FuseRequest::Read { .. } => OpType::Read,
            FuseRequest::Write { .. } => OpType::Write,
            FuseRequest::Close { .. } => OpType::Close,
            FuseRequest::Readdir { .. } => OpType::Readdir,
            FuseRequest::Stat { .. } => OpType::Stat,
            FuseRequest::Mkdir { .. } => OpType::Mkdir,
            FuseRequest::Remove { .. } => OpType::Remove,
            FuseRequest::PathQuery { .. } => OpType::PathQuery,
            FuseRequest::Ping { .. } => OpType::Ping,
        }
    }
}

/// Host response
#[derive(Debug)]
pub enum FuseResponse {
    Open { request_id: RequestId, status: StatusCode, fd: u64 },
    Read { request_id: RequestId, status: StatusCode, data: Vec<u8> },
    Write { request_id: RequestId, status: StatusCode, written: u32 },
    Close { request_id: RequestId, status: StatusCode },
    Readdir { request_id: RequestId, status: StatusCode, entries: Vec<DirEntry> },
    Stat { request_id: RequestId, status: StatusCode, file_type: FileType, mode: u32, size: u64, mtime: u64, atime: u64, ctime: u64 },
    Mkdir { request_id: RequestId, status: StatusCode },
    Remove { request_id: RequestId, status: StatusCode },
    PathQuery { request_id: RequestId, status: StatusCode, state: u8, path: String },
    Pong { request_id: RequestId },
}

impl FuseResponse {
    pub fn request_id(&self) -> RequestId {
        match self {
            FuseResponse::Open { request_id, .. } => *request_id,
            FuseResponse::Read { request_id, .. } => *request_id,
            FuseResponse::Write { request_id, .. } => *request_id,
            FuseResponse::Close { request_id, .. } => *request_id,
            FuseResponse::Readdir { request_id, .. } => *request_id,
            FuseResponse::Stat { request_id, .. } => *request_id,
            FuseResponse::Mkdir { request_id, .. } => *request_id,
            FuseResponse::Remove { request_id, .. } => *request_id,
            FuseResponse::PathQuery { request_id, .. } => *request_id,
            FuseResponse::Pong { request_id } => *request_id,
        }
    }

    pub fn status(&self) -> StatusCode {
        match self {
            FuseResponse::Open { status, .. } => *status,
            FuseResponse::Read { status, .. } => *status,
            FuseResponse::Write { status, .. } => *status,
            FuseResponse::Close { status, .. } => *status,
            FuseResponse::Readdir { status, .. } => *status,
            FuseResponse::Stat { status, .. } => *status,
            FuseResponse::Mkdir { status, .. } => *status,
            FuseResponse::Remove { status, .. } => *status,
            FuseResponse::PathQuery { status, .. } => *status,
            FuseResponse::Pong { .. } => StatusCode::Ok,
        }
    }
}

/// Host-initiated notification
#[derive(Debug)]
pub enum FuseNotification {
    PathChanged { old_path: String, new_path: String },
    PathPending { pending_handles: u32, timeout_sec: u32 },
    PathReady,
    InvalidateCache,
    Shutdown,
}

/// Decoded message from host (response or notification)
#[derive(Debug)]
pub enum FuseMessage {
    Response(FuseResponse),
    Notification(FuseNotification),
}

/// Encode a FuseRequest into bytes for VirtualStream.write().
pub fn encode_request(req: &FuseRequest) -> BytesMut {
    let mut payload = BytesMut::new();
    let op_type: u16;

    match req {
        FuseRequest::Open { request_id, flags, mode, path } => {
            op_type = OpType::Open as u16;
            payload.put_u64_le(*request_id);
            payload.put_u32_le(*flags);
            payload.put_u32_le(*mode);
            let path_bytes = path.as_bytes();
            payload.put_u16_le(path_bytes.len() as u16);
            payload.put_slice(path_bytes);
        }
        FuseRequest::Read { request_id, fd, offset, size } => {
            op_type = OpType::Read as u16;
            payload.put_u64_le(*request_id);
            payload.put_u64_le(*fd);
            payload.put_u64_le(*offset);
            payload.put_u32_le(*size);
        }
        FuseRequest::Write { request_id, fd, offset, data } => {
            op_type = OpType::Write as u16;
            payload.put_u64_le(*request_id);
            payload.put_u64_le(*fd);
            payload.put_u64_le(*offset);
            payload.put_u32_le(data.len() as u32);
            payload.put_slice(data);
        }
        FuseRequest::Close { request_id, fd } => {
            op_type = OpType::Close as u16;
            payload.put_u64_le(*request_id);
            payload.put_u64_le(*fd);
        }
        FuseRequest::Readdir { request_id, fd, offset, max_entries } => {
            op_type = OpType::Readdir as u16;
            payload.put_u64_le(*request_id);
            payload.put_u64_le(*fd);
            payload.put_u64_le(*offset);
            payload.put_u32_le(*max_entries);
        }
        FuseRequest::Stat { request_id, path } => {
            op_type = OpType::Stat as u16;
            payload.put_u64_le(*request_id);
            let path_bytes = path.as_bytes();
            payload.put_u16_le(path_bytes.len() as u16);
            payload.put_slice(path_bytes);
        }
        FuseRequest::Mkdir { request_id, mode, path } => {
            op_type = OpType::Mkdir as u16;
            payload.put_u64_le(*request_id);
            payload.put_u32_le(*mode);
            let path_bytes = path.as_bytes();
            payload.put_u16_le(path_bytes.len() as u16);
            payload.put_slice(path_bytes);
        }
        FuseRequest::Remove { request_id, is_dir, path } => {
            op_type = OpType::Remove as u16;
            payload.put_u64_le(*request_id);
            payload.put_u8(if *is_dir { 1 } else { 0 });
            let path_bytes = path.as_bytes();
            payload.put_u16_le(path_bytes.len() as u16);
            payload.put_slice(path_bytes);
        }
        FuseRequest::PathQuery { request_id } => {
            op_type = OpType::PathQuery as u16;
            payload.put_u64_le(*request_id);
        }
        FuseRequest::Ping { request_id } => {
            op_type = OpType::Ping as u16;
            payload.put_u64_le(*request_id);
        }
    }

    let mut dst = BytesMut::with_capacity(HEADER_SIZE + payload.len());
    dst.put_u32(MAGIC);
    dst.put_u16_le(op_type);
    dst.put_u32_le(payload.len() as u32);
    dst.put(payload);
    dst
}

/// Decode a FuseMessage (response or notification) from a buffer.
pub fn decode_response(src: &mut BytesMut) -> anyhow::Result<Option<FuseMessage>> {
    if src.len() < HEADER_SIZE {
        return Ok(None);
    }

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

    src.advance(HEADER_SIZE);
    let mut payload = src.split_to(length);

    let op = OpType::try_from(op_type)?;
    let message = match op {
        OpType::Open => {
            let request_id = payload.get_u64_le();
            let status = StatusCode::try_from(payload.get_u16_le())?;
            let fd = payload.get_u64_le();
            FuseMessage::Response(FuseResponse::Open { request_id, status, fd })
        }
        OpType::Read => {
            let request_id = payload.get_u64_le();
            let status = StatusCode::try_from(payload.get_u16_le())?;
            let data_len = payload.get_u32_le() as usize;
            let data = payload.split_to(data_len).to_vec();
            FuseMessage::Response(FuseResponse::Read { request_id, status, data })
        }
        OpType::Write => {
            let request_id = payload.get_u64_le();
            let status = StatusCode::try_from(payload.get_u16_le())?;
            let written = payload.get_u32_le();
            FuseMessage::Response(FuseResponse::Write { request_id, status, written })
        }
        OpType::Close => {
            let request_id = payload.get_u64_le();
            let status = StatusCode::try_from(payload.get_u16_le())?;
            FuseMessage::Response(FuseResponse::Close { request_id, status })
        }
        OpType::Readdir => {
            let request_id = payload.get_u64_le();
            let status = StatusCode::try_from(payload.get_u16_le())?;
            let count = payload.get_u32_le() as usize;
            let mut entries = Vec::with_capacity(count);
            for _ in 0..count {
                let ft = FileType::try_from(payload.get_u8())?;
                let name_len = payload.get_u16_le() as usize;
                let name = String::from_utf8(payload.split_to(name_len).to_vec())?;
                entries.push(DirEntry { file_type: ft, name });
            }
            FuseMessage::Response(FuseResponse::Readdir { request_id, status, entries })
        }
        OpType::Stat => {
            let request_id = payload.get_u64_le();
            let status = StatusCode::try_from(payload.get_u16_le())?;
            let file_type_u8 = payload.get_u8();
            let file_type = FileType::try_from(file_type_u8).unwrap_or(FileType::Other);
            let mode = payload.get_u32_le();
            let size = payload.get_u64_le();
            let mtime = payload.get_u64_le();
            let atime = payload.get_u64_le();
            let ctime = payload.get_u64_le();
            FuseMessage::Response(FuseResponse::Stat { request_id, status, file_type, mode, size, mtime, atime, ctime })
        }
        OpType::Mkdir => {
            let request_id = payload.get_u64_le();
            let status = StatusCode::try_from(payload.get_u16_le())?;
            FuseMessage::Response(FuseResponse::Mkdir { request_id, status })
        }
        OpType::Remove => {
            let request_id = payload.get_u64_le();
            let status = StatusCode::try_from(payload.get_u16_le())?;
            FuseMessage::Response(FuseResponse::Remove { request_id, status })
        }
        OpType::PathQuery => {
            let request_id = payload.get_u64_le();
            let status = StatusCode::try_from(payload.get_u16_le())?;
            let state = payload.get_u8();
            let path_len = payload.get_u16_le() as usize;
            let path = String::from_utf8(payload.split_to(path_len).to_vec())?;
            FuseMessage::Response(FuseResponse::PathQuery { request_id, status, state, path })
        }
        OpType::Pong => {
            let request_id = payload.get_u64_le();
            FuseMessage::Response(FuseResponse::Pong { request_id })
        }
        // Host-initiated notifications (no request_id)
        OpType::PathChanged => {
            let old_len = payload.get_u16_le() as usize;
            let old_path = String::from_utf8(payload.split_to(old_len).to_vec())?;
            let new_len = payload.get_u16_le() as usize;
            let new_path = String::from_utf8(payload.split_to(new_len).to_vec())?;
            FuseMessage::Notification(FuseNotification::PathChanged { old_path, new_path })
        }
        OpType::PathPending => {
            let pending_handles = payload.get_u32_le();
            let timeout_sec = payload.get_u32_le();
            FuseMessage::Notification(FuseNotification::PathPending { pending_handles, timeout_sec })
        }
        OpType::PathReady => {
            FuseMessage::Notification(FuseNotification::PathReady)
        }
        OpType::InvalidateCache => {
            FuseMessage::Notification(FuseNotification::InvalidateCache)
        }
        OpType::Shutdown => {
            FuseMessage::Notification(FuseNotification::Shutdown)
        }
        _ => return Err(anyhow::anyhow!("Unexpected response type: {:?}", op)),
    };

    Ok(Some(message))
}
