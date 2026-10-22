//! Wire framing: `u32 len BE | u8 kind | u32 channel BE | payload`, where `len` counts everything after itself.

use std::io::{self, Read, Write};

use serde::Serialize;

use crate::error::Error;

/// Largest frame, measured like the length prefix (kind + channel + payload).
pub const MAX_FRAME: usize = 1024 * 1024;
/// Largest byte run in one DATA or INPUT frame, excluding its offset.
pub const MAX_DATA: usize = 32 * 1024;
const HEADER_AFTER_LENGTH: usize = 1 + 4;
const OFFSET_LENGTH: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Hello,
    Request,
    Response,
    Cancel,
    Push,
    End,
    Data,
    Input,
    Ack,
    Reset,
    Ping,
    Pong,
}

impl Kind {
    pub const ALL: [Kind; 12] = [
        Kind::Hello,
        Kind::Request,
        Kind::Response,
        Kind::Cancel,
        Kind::Push,
        Kind::End,
        Kind::Data,
        Kind::Input,
        Kind::Ack,
        Kind::Reset,
        Kind::Ping,
        Kind::Pong,
    ];

    pub fn code(self) -> u8 {
        match self {
            Kind::Hello => 0x01,
            Kind::Request => 0x02,
            Kind::Response => 0x03,
            Kind::Cancel => 0x04,
            Kind::Push => 0x05,
            Kind::End => 0x06,
            Kind::Data => 0x10,
            Kind::Input => 0x11,
            Kind::Ack => 0x12,
            Kind::Reset => 0x13,
            Kind::Ping => 0x20,
            Kind::Pong => 0x21,
        }
    }

    pub fn from_code(code: u8) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.code() == code)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub kind: Kind,
    pub channel: u32,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn new(kind: Kind, channel: u32, payload: Vec<u8>) -> Self {
        Frame {
            kind,
            channel,
            payload,
        }
    }

    pub fn json(kind: Kind, channel: u32, value: &impl Serialize) -> Result<Self, Error> {
        let payload =
            serde_json::to_vec(value).map_err(|error| Error::Protocol(error.to_string()))?;
        let frame = Frame::new(kind, channel, payload);
        frame.check_size()?;
        Ok(frame)
    }

    /// DATA and INPUT carry the absolute offset of their first byte, so a resumed reader can tell what it missed.
    pub fn bytes(kind: Kind, channel: u32, offset: u64, bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_DATA {
            return Err(Error::Limit(format!(
                "{} bytes in one frame, at most {MAX_DATA}",
                bytes.len()
            )));
        }
        let mut payload = Vec::with_capacity(OFFSET_LENGTH + bytes.len());
        payload.extend_from_slice(&offset.to_be_bytes());
        payload.extend_from_slice(bytes);
        Ok(Frame::new(kind, channel, payload))
    }

    pub fn offset_only(kind: Kind, channel: u32, offset: u64) -> Self {
        Frame::new(kind, channel, offset.to_be_bytes().to_vec())
    }

    pub fn check_size(&self) -> Result<(), Error> {
        let length = HEADER_AFTER_LENGTH + self.payload.len();
        if length > MAX_FRAME {
            return Err(Error::Limit(format!(
                "{length} byte frame, at most {MAX_FRAME}"
            )));
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        self.check_size()?;
        let length = u32::try_from(HEADER_AFTER_LENGTH + self.payload.len())
            .map_err(|_| Error::Limit("frame length overflows u32".to_string()))?;
        let mut buffer = Vec::with_capacity(4 + HEADER_AFTER_LENGTH + self.payload.len());
        buffer.extend_from_slice(&length.to_be_bytes());
        buffer.push(self.kind.code());
        buffer.extend_from_slice(&self.channel.to_be_bytes());
        buffer.extend_from_slice(&self.payload);
        Ok(buffer)
    }

    /// Splits a DATA or INPUT payload into its offset and bytes.
    pub fn offset_and_bytes(&self) -> Result<(u64, &[u8]), Error> {
        let (offset, bytes) = split_offset(&self.payload)?;
        if bytes.len() > MAX_DATA {
            return Err(Error::Protocol(format!(
                "{} bytes in one {:?} frame, at most {MAX_DATA}",
                bytes.len(),
                self.kind
            )));
        }
        Ok((offset, bytes))
    }

    /// Reads an ACK or RESET payload.
    pub fn offset(&self) -> Result<u64, Error> {
        let (offset, rest) = split_offset(&self.payload)?;
        if !rest.is_empty() {
            return Err(Error::Protocol(format!(
                "{:?} payload longer than an offset",
                self.kind
            )));
        }
        Ok(offset)
    }
}

fn split_offset(payload: &[u8]) -> Result<(u64, &[u8]), Error> {
    let (head, rest) = payload
        .split_first_chunk::<OFFSET_LENGTH>()
        .ok_or_else(|| Error::Protocol("payload shorter than its offset".to_string()))?;
    Ok((u64::from_be_bytes(*head), rest))
}

pub fn write_frame(writer: &mut impl Write, frame: &Frame) -> Result<(), Error> {
    let buffer = frame.encode()?;
    writer.write_all(&buffer)?;
    writer.flush()?;
    Ok(())
}

#[derive(Debug)]
pub enum ReadError {
    Io(io::Error),
    Protocol(String),
}

/// `Ok(None)` is a clean end of stream between frames; end of stream inside a frame is an error.
pub fn read_frame(reader: &mut impl Read) -> Result<Option<Frame>, ReadError> {
    let mut length_bytes = [0u8; 4];
    if !read_exact_or_eof(reader, &mut length_bytes)? {
        return Ok(None);
    }
    let length = usize::try_from(u32::from_be_bytes(length_bytes))
        .map_err(|_| ReadError::Protocol("frame length overflows usize".to_string()))?;
    if length > MAX_FRAME {
        return Err(ReadError::Protocol(format!(
            "{length} byte frame, at most {MAX_FRAME}"
        )));
    }
    if length < HEADER_AFTER_LENGTH {
        return Err(ReadError::Protocol(format!(
            "{length} byte frame is shorter than its header"
        )));
    }
    let mut header = [0u8; HEADER_AFTER_LENGTH];
    reader.read_exact(&mut header).map_err(ReadError::Io)?;
    let [code, channel_bytes @ ..] = header;
    let kind = Kind::from_code(code)
        .ok_or_else(|| ReadError::Protocol(format!("unknown frame kind 0x{code:02x}")))?;
    let mut payload = vec![0u8; length - HEADER_AFTER_LENGTH];
    reader.read_exact(&mut payload).map_err(ReadError::Io)?;
    Ok(Some(Frame {
        kind,
        channel: u32::from_be_bytes(channel_bytes),
        payload,
    }))
}

fn read_exact_or_eof(reader: &mut impl Read, buffer: &mut [u8]) -> Result<bool, ReadError> {
    let mut filled = 0;
    while filled < buffer.len() {
        let Some(rest) = buffer.get_mut(filled..) else {
            break;
        };
        match reader.read(rest) {
            Ok(0) if filled == 0 => return Ok(false),
            Ok(0) => {
                return Err(ReadError::Io(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "end of stream inside a frame header",
                )))
            }
            Ok(count) => filled += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(ReadError::Io(error)),
        }
    }
    Ok(true)
}
