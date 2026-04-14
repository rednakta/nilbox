//! VSOCK frame protocol

use bytes::{Buf, BufMut, Bytes, BytesMut};
use anyhow::{Result, anyhow};

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameType {
    Data = 0x01,
    Connect = 0x02,
    Fin = 0x03,
    Rst = 0x04,
    ReverseRequest = 0x05,
    ReverseResponse = 0x06,
    TtyData = 0x07,
    TtyResize = 0x08,
    DnsRequest = 0x09,
    DnsResponse = 0x0A,
    HostConnect = 0x0B,
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

#[derive(Debug, Clone)]
pub struct Frame {
    pub stream_id: u32,
    pub frame_type: FrameType,
    pub payload: Bytes,
}

impl Frame {
    pub const HEADER_SIZE: usize = 9;

    pub fn new(stream_id: u32, frame_type: FrameType, payload: impl Into<Bytes>) -> Self {
        Self { stream_id, frame_type, payload: payload.into() }
    }

    pub fn data(stream_id: u32, payload: impl Into<Bytes>) -> Self {
        Self::new(stream_id, FrameType::Data, payload)
    }

    pub fn connect(stream_id: u32, target_port: u32) -> Self {
        let mut payload = BytesMut::with_capacity(4);
        payload.put_u32(target_port);
        Self::new(stream_id, FrameType::Connect, payload.freeze())
    }

    pub fn fin(stream_id: u32) -> Self {
        Self::new(stream_id, FrameType::Fin, Bytes::new())
    }

    pub fn rst(stream_id: u32) -> Self {
        Self::new(stream_id, FrameType::Rst, Bytes::new())
    }

    pub fn reverse_request(stream_id: u32, payload: impl Into<Bytes>) -> Self {
        Self::new(stream_id, FrameType::ReverseRequest, payload)
    }

    pub fn reverse_response(stream_id: u32, payload: impl Into<Bytes>) -> Self {
        Self::new(stream_id, FrameType::ReverseResponse, payload)
    }

    pub fn dns_request(stream_id: u32, payload: impl Into<Bytes>) -> Self {
        Self::new(stream_id, FrameType::DnsRequest, payload)
    }

    pub fn dns_response(stream_id: u32, payload: impl Into<Bytes>) -> Self {
        Self::new(stream_id, FrameType::DnsResponse, payload)
    }

    /// mode: 0x00=Auto, 0x01=Headless, 0x02=Headed
    pub fn host_connect(stream_id: u32, target_port: u32, mode: u8) -> Self {
        let mut payload = BytesMut::with_capacity(5);
        payload.put_u32(target_port);
        payload.put_u8(mode);
        Self::new(stream_id, FrameType::HostConnect, payload.freeze())
    }

    /// Credit-based flow control: advertises additional bytes the sender
    /// can receive. Uses stream_id=0 for connection-level credit.
    pub fn window_update(credit_bytes: u32) -> Self {
        let mut payload = BytesMut::with_capacity(4);
        payload.put_u32(credit_bytes);
        Self::new(0, FrameType::WindowUpdate, payload.freeze())
    }

    pub fn tty_resize(stream_id: u32, rows: u16, cols: u16) -> Self {
        let mut payload = BytesMut::with_capacity(4);
        payload.put_u16(rows);
        payload.put_u16(cols);
        Self::new(stream_id, FrameType::TtyResize, payload.freeze())
    }

    pub fn encode(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(Self::HEADER_SIZE + self.payload.len());
        buf.put_u32(self.stream_id);
        buf.put_u8(self.frame_type as u8);
        buf.put_u32(self.payload.len() as u32);
        buf.put_slice(&self.payload);
        buf.freeze()
    }

    pub fn decode(mut buf: Bytes) -> Result<Self> {
        if buf.len() < Self::HEADER_SIZE {
            return Err(anyhow!("Buffer too small for frame header"));
        }
        let stream_id = buf.get_u32();
        let frame_type = FrameType::try_from(buf.get_u8())?;
        let length = buf.get_u32() as usize;
        if buf.len() < length {
            return Err(anyhow!("Buffer too small for payload"));
        }
        let payload = buf.slice(..length);
        Ok(Self { stream_id, frame_type, payload })
    }
}
