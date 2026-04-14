//! VSOCK frame protocol (Shared with Host)
//!
//! Frame format:
//! - StreamID: 4 bytes (big-endian)
//! - FrameType: 1 byte
//! - Length: 4 bytes (big-endian)
//! - Payload: variable length

use bytes::{Buf, BufMut, Bytes, BytesMut};
use anyhow::{Result, anyhow};

/// Frame type identifiers
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameType {
    /// TCP payload data
    Data = 0x01,
    /// Stream connection request
    Connect = 0x02,
    /// Normal stream close
    Fin = 0x03,
    /// Error/reset
    Rst = 0x04,
    /// VM → Host reverse proxy request
    ReverseRequest = 0x05,
    /// Host → VM reverse proxy response
    ReverseResponse = 0x06,
    /// TTY data
    TtyData = 0x07,
    /// TTY resize
    TtyResize = 0x08,
    /// VM → Host DNS query
    DnsRequest = 0x09,
    /// Host → VM DNS response
    DnsResponse = 0x0A,
    /// VM → Host raw TCP tunnel to host localhost
    HostConnect = 0x0B,
    /// Credit-based flow control (connection-level)
    WindowUpdate = 0x0C,
}

impl TryFrom<u8> for FrameType {
    type Error = anyhow::Error;

    fn try_from(value: u8) -> Result<Self> {
        match value {
            0x01 => Ok(FrameType::Data),
            0x02 => Ok(FrameType::Connect),
            0x03 => Ok(FrameType::Fin),
            0x04 => Ok(FrameType::Rst),
            0x05 => Ok(FrameType::ReverseRequest),
            0x06 => Ok(FrameType::ReverseResponse),
            0x07 => Ok(FrameType::TtyData),
            0x08 => Ok(FrameType::TtyResize),
            0x09 => Ok(FrameType::DnsRequest),
            0x0A => Ok(FrameType::DnsResponse),
            0x0B => Ok(FrameType::HostConnect),
            0x0C => Ok(FrameType::WindowUpdate),
            _ => Err(anyhow!("Invalid frame type: {}", value)),
        }
    }
}

/// VSOCK frame structure
#[derive(Debug, Clone)]
pub struct Frame {
    /// Stream identifier for multiplexing
    pub stream_id: u32,
    /// Frame type
    pub frame_type: FrameType,
    /// Payload data
    pub payload: Bytes,
}

impl Frame {
    /// Frame header size: StreamID(4) + FrameType(1) + Length(4) = 9 bytes
    pub const HEADER_SIZE: usize = 9;

    /// Create a new frame
    pub fn new(stream_id: u32, frame_type: FrameType, payload: impl Into<Bytes>) -> Self {
        Self {
            stream_id,
            frame_type,
            payload: payload.into(),
        }
    }

    /// Create a data frame
    pub fn data(stream_id: u32, payload: impl Into<Bytes>) -> Self {
        Self::new(stream_id, FrameType::Data, payload)
    }

    /// Create a connect frame
    pub fn connect(stream_id: u32, target_port: u32) -> Self {
        let mut payload = BytesMut::with_capacity(4);
        payload.put_u32(target_port);
        Self::new(stream_id, FrameType::Connect, payload.freeze())
    }

    /// Create a fin frame
    pub fn fin(stream_id: u32) -> Self {
        Self::new(stream_id, FrameType::Fin, Bytes::new())
    }

    /// Create a rst frame
    pub fn rst(stream_id: u32) -> Self {
        Self::new(stream_id, FrameType::Rst, Bytes::new())
    }

    /// Create a reverse request frame
    pub fn reverse_request(stream_id: u32, payload: impl Into<Bytes>) -> Self {
        Self::new(stream_id, FrameType::ReverseRequest, payload)
    }

    /// Create a reverse response frame
    pub fn reverse_response(stream_id: u32, payload: impl Into<Bytes>) -> Self {
        Self::new(stream_id, FrameType::ReverseResponse, payload)
    }

    /// Create a DNS request frame
    pub fn dns_request(stream_id: u32, payload: impl Into<Bytes>) -> Self {
        Self::new(stream_id, FrameType::DnsRequest, payload)
    }

    /// Create a DNS response frame
    pub fn dns_response(stream_id: u32, payload: impl Into<Bytes>) -> Self {
        Self::new(stream_id, FrameType::DnsResponse, payload)
    }

    /// Credit-based flow control: advertises additional bytes the sender
    /// can receive. Uses stream_id=0 for connection-level credit.
    pub fn window_update(credit_bytes: u32) -> Self {
        let mut payload = BytesMut::with_capacity(4);
        payload.put_u32(credit_bytes);
        Self::new(0, FrameType::WindowUpdate, payload.freeze())
    }

    /// Create a host connect frame (VM → Host raw TCP tunnel)
    /// mode: 0x00=Auto, 0x01=Headless, 0x02=Headed
    pub fn host_connect(stream_id: u32, target_port: u32, mode: u8) -> Self {
        let mut payload = BytesMut::with_capacity(5);
        payload.put_u32(target_port);
        payload.put_u8(mode);
        Self::new(stream_id, FrameType::HostConnect, payload.freeze())
    }

    /// Encode frame to bytes
    pub fn encode(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(Self::HEADER_SIZE + self.payload.len());
        buf.put_u32(self.stream_id);
        buf.put_u8(self.frame_type as u8);
        buf.put_u32(self.payload.len() as u32);
        buf.put_slice(&self.payload);
        buf.freeze()
    }

    /// Decode frame from bytes
    pub fn decode(mut buf: Bytes) -> Result<Self> {
        if buf.len() < Self::HEADER_SIZE {
            return Err(anyhow!("Buffer too small for frame header"));
        }

        let stream_id = buf.get_u32();
        let frame_type = FrameType::try_from(buf.get_u8())?;
        let length = buf.get_u32() as usize;

        if buf.len() < length {
            return Err(anyhow!("Buffer too small for payload: expected {}, got {}", length, buf.len()));
        }

        let payload = buf.slice(..length);

        Ok(Self {
            stream_id,
            frame_type,
            payload,
        })
    }
}
