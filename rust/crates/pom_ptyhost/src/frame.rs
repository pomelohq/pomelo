//! The holder wire format, shared with the previous core: every frame is one type byte, a big-endian u16
//! payload length, then the payload. After the snapshot frames the holder switches to raw output.

use std::io::{self, Read, Write};

pub const INPUT: u8 = 0x00;
pub const RESIZE: u8 = 0x01;
pub const PRIMARY: u8 = 0x02;
pub const RESUME: u8 = 0x03;

/// Payload: the absolute output offset right after the snapshot, to resume from on the next connect.
pub const META: u8 = 0x10;
pub const SNAPSHOT: u8 = 0x11;
/// Zero-length: the snapshot is done and raw live output follows.
pub const SNAPSHOT_END: u8 = 0x12;

pub const MAX_PAYLOAD: usize = u16::MAX as usize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub kind: u8,
    pub payload: Vec<u8>,
}

pub fn write_frame(writer: &mut impl Write, kind: u8, payload: &[u8]) -> io::Result<()> {
    let length = u16::try_from(payload.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame payload too large"))?;
    let mut header = [kind, 0, 0];
    header[1..3].copy_from_slice(&length.to_be_bytes());
    writer.write_all(&header)?;
    writer.write_all(payload)
}

pub fn read_frame(reader: &mut impl Read) -> io::Result<Frame> {
    let mut header = [0u8; 3];
    reader.read_exact(&mut header)?;
    let length = usize::from(u16::from_be_bytes([header[1], header[2]]));
    let mut payload = vec![0u8; length];
    reader.read_exact(&mut payload)?;
    Ok(Frame {
        kind: header[0],
        payload,
    })
}

/// Input larger than one frame goes out as several, in order.
pub fn write_input(writer: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    for chunk in bytes.chunks(MAX_PAYLOAD) {
        write_frame(writer, INPUT, chunk)?;
    }
    Ok(())
}

pub fn write_resize(writer: &mut impl Write, cols: u16, rows: u16) -> io::Result<()> {
    let mut payload = [0u8; 4];
    payload[0..2].copy_from_slice(&cols.to_be_bytes());
    payload[2..4].copy_from_slice(&rows.to_be_bytes());
    write_frame(writer, RESIZE, &payload)
}

pub fn write_primary(writer: &mut impl Write) -> io::Result<()> {
    write_frame(writer, PRIMARY, &[])
}

pub fn write_resume(writer: &mut impl Write, since: u64) -> io::Result<()> {
    write_frame(writer, RESUME, &since.to_be_bytes())
}

pub fn parse_resize(payload: &[u8]) -> Option<(u16, u16)> {
    let cols = u16::from_be_bytes([*payload.first()?, *payload.get(1)?]);
    let rows = u16::from_be_bytes([*payload.get(2)?, *payload.get(3)?]);
    (payload.len() == 4).then_some((cols, rows))
}

pub fn parse_u64(payload: &[u8]) -> Option<u64> {
    let bytes: [u8; 8] = payload.try_into().ok()?;
    Some(u64::from_be_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() -> io::Result<()> {
        let mut wire = Vec::new();
        write_resize(&mut wire, 120, 40)?;
        write_resume(&mut wire, 7)?;
        write_primary(&mut wire)?;
        let mut reader = wire.as_slice();
        let resize = read_frame(&mut reader)?;
        assert_eq!(resize.kind, RESIZE);
        assert_eq!(parse_resize(&resize.payload), Some((120, 40)));
        let resume = read_frame(&mut reader)?;
        assert_eq!(parse_u64(&resume.payload), Some(7));
        assert_eq!(
            read_frame(&mut reader)?,
            Frame {
                kind: PRIMARY,
                payload: Vec::new()
            }
        );
        Ok(())
    }

    #[test]
    fn large_input_splits_into_frames() -> io::Result<()> {
        let mut wire = Vec::new();
        let input = vec![b'x'; MAX_PAYLOAD + 10];
        write_input(&mut wire, &input)?;
        let mut reader = wire.as_slice();
        assert_eq!(read_frame(&mut reader)?.payload.len(), MAX_PAYLOAD);
        assert_eq!(read_frame(&mut reader)?.payload.len(), 10);
        Ok(())
    }

    #[test]
    fn header_matches_the_previous_core() -> io::Result<()> {
        let mut wire = Vec::new();
        write_frame(&mut wire, INPUT, b"hi")?;
        assert_eq!(wire, [0x00, 0x00, 0x02, b'h', b'i']);
        Ok(())
    }
}
