//! Bounded, versioned records shared by the native endpoints and capture helper.
use anyhow::{bail, ensure, Context, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::io::{Read, Write};

pub const PORT: u16 = 47831;
pub const DISCOVERY_PORT: u16 = 47832;
pub const MAX_RECORD: usize = 8 * 1024 * 1024;
pub const VERSION: u16 = 1;
pub const JSON: u8 = 1;
pub const VIDEO: u8 = 2;
pub const ERROR: u8 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    pub protocol: u16,
    pub name: String,
    pub pin: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Welcome {
    pub accepted: bool,
    pub message: String,
    pub codec: String,
}

#[derive(Debug, Clone)]
pub struct VideoFrame {
    pub sequence: u64,
    pub width: u32,
    pub height: u32,
    pub keyframe: bool,
    pub data: Vec<u8>,
}

pub fn write_record(w: &mut impl Write, kind: u8, bytes: &[u8]) -> Result<()> {
    ensure!(bytes.len() < MAX_RECORD, "record too large");
    w.write_all(&((bytes.len() + 1) as u32).to_be_bytes())?;
    w.write_all(&[kind])?;
    w.write_all(bytes)?;
    w.flush()?;
    Ok(())
}

pub fn read_record(r: &mut impl Read) -> Result<(u8, Vec<u8>)> {
    let mut size = [0; 4];
    r.read_exact(&mut size)
        .context("connection closed while reading record")?;
    let n = u32::from_be_bytes(size) as usize;
    ensure!((1..=MAX_RECORD).contains(&n), "invalid record size");
    let mut bytes = vec![0; n];
    r.read_exact(&mut bytes)?;
    Ok((bytes[0], bytes[1..].to_vec()))
}

pub fn write_json(w: &mut impl Write, value: &impl Serialize) -> Result<()> {
    write_record(w, JSON, &serde_json::to_vec(value)?)
}

pub fn read_json<T: DeserializeOwned>(r: &mut impl Read) -> Result<T> {
    let (kind, data) = read_record(r)?;
    ensure!(
        kind == JSON && data.len() <= 4096,
        "expected control record"
    );
    Ok(serde_json::from_slice(&data)?)
}

pub fn write_video(w: &mut impl Write, frame: &VideoFrame) -> Result<()> {
    validate_dimensions(frame.width, frame.height)?;
    ensure!(frame.data.len() <= MAX_RECORD - 18, "video frame too large");
    let mut data = Vec::with_capacity(17 + frame.data.len());
    data.extend_from_slice(&frame.sequence.to_be_bytes());
    data.extend_from_slice(&frame.width.to_be_bytes());
    data.extend_from_slice(&frame.height.to_be_bytes());
    data.push(u8::from(frame.keyframe));
    data.extend_from_slice(&frame.data);
    write_record(w, VIDEO, &data)
}

pub fn decode_video(data: &[u8]) -> Result<VideoFrame> {
    ensure!(data.len() >= 18, "short video frame");
    let width = u32::from_be_bytes(data[8..12].try_into()?);
    let height = u32::from_be_bytes(data[12..16].try_into()?);
    validate_dimensions(width, height)?;
    ensure!(data[16] <= 1, "invalid keyframe flag");
    Ok(VideoFrame {
        sequence: u64::from_be_bytes(data[..8].try_into()?),
        width,
        height,
        keyframe: data[16] == 1,
        data: data[17..].to_vec(),
    })
}

pub fn validate_dimensions(width: u32, height: u32) -> Result<()> {
    ensure!(
        (16..=4096).contains(&width) && (16..=2160).contains(&height),
        "unsupported video dimensions"
    );
    ensure!(
        width.is_multiple_of(2) && height.is_multiple_of(2),
        "video dimensions must be even"
    );
    Ok(())
}

pub fn validate_hello(hello: &Hello) -> Result<()> {
    ensure!(hello.protocol == VERSION, "protocol version mismatch");
    ensure!(
        !hello.name.trim().is_empty() && hello.name.chars().count() <= 40,
        "invalid device name"
    );
    ensure!(
        hello.pin.len() == 6 && hello.pin.bytes().all(|c| c.is_ascii_digit()),
        "invalid pairing code"
    );
    Ok(())
}

pub fn contains_idr(bytes: &[u8]) -> bool {
    // Both three- and four-byte Annex B start codes end in 00 00 01.
    bytes
        .windows(4)
        .any(|p| p[..3] == [0, 0, 1] && p[3] & 0x1f == 5)
}

pub fn next_frame(r: &mut impl Read) -> Result<VideoFrame> {
    let (kind, data) = read_record(r)?;
    match kind {
        VIDEO => decode_video(&data),
        ERROR => bail!("host stopped the stream"),
        _ => bail!("unexpected stream record"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    #[test]
    fn bounded_frames_round_trip() {
        let f = VideoFrame {
            sequence: 9,
            width: 640,
            height: 360,
            keyframe: true,
            data: vec![0, 0, 0, 1, 0x65, 1],
        };
        let mut bytes = Vec::new();
        write_video(&mut bytes, &f).unwrap();
        let decoded = next_frame(&mut Cursor::new(bytes)).unwrap();
        assert_eq!(decoded.sequence, 9);
        assert_eq!(decoded.data, f.data);
        assert!(contains_idr(&decoded.data));
    }
    #[test]
    fn reject_oversize_and_corrupt_dimensions_before_decode() {
        assert!(read_record(&mut Cursor::new(u32::MAX.to_be_bytes())).is_err());
        assert!(read_record(&mut Cursor::new(0u32.to_be_bytes())).is_err());
        assert!(decode_video(&[0; 17]).is_err());
        assert!(validate_dimensions(8000, 8000).is_err());
        assert!(validate_dimensions(641, 360).is_err());
    }
    #[test]
    fn version_and_pairing_validation() {
        let mut h = Hello {
            protocol: VERSION,
            name: "Receiver".into(),
            pin: "012345".into(),
        };
        assert!(validate_hello(&h).is_ok());
        h.protocol += 1;
        assert!(validate_hello(&h).is_err());
    }
}
