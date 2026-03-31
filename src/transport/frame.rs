//! Ergo P2P message frame encoding and decoding.
//!
//! # Contract
//! - `encode`: given magic, code, and body, produces a valid frame.
//!   Postcondition: output is `[magic:4][code:1][len:4 BE][checksum:4][body:N]`.
//! - `decode`: given magic and raw bytes, validates and extracts a Frame.
//!   Precondition: `data.len() >= 13` (header size).
//!   Postcondition: magic matches, checksum valid, body length matches header.
//! - Invariant: `decode(magic, encode(magic, frame)) == Ok(frame)` for any valid frame.

use blake2::digest::consts::U32;
use blake2::{Blake2b, Digest};
use std::io;

type Blake2b256 = Blake2b<U32>;

const HEADER_SIZE: usize = 13; // 4 magic + 1 code + 4 length + 4 checksum
const MAX_BODY_SIZE: u32 = 256 * 1024;

/// A validated P2P message frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub code: u8,
    pub body: Vec<u8>,
}

/// Encode a Frame into wire bytes with the given network magic.
pub fn encode(magic: &[u8; 4], frame: &Frame) -> Vec<u8> {
    let mut msg = Vec::with_capacity(HEADER_SIZE + frame.body.len());
    msg.extend_from_slice(magic);
    msg.push(frame.code);
    msg.extend_from_slice(&(frame.body.len() as u32).to_be_bytes());
    let hash = Blake2b256::digest(&frame.body);
    msg.extend_from_slice(&hash[..4]);
    msg.extend_from_slice(&frame.body);
    msg
}

/// Decode a Frame from wire bytes, validating magic and checksum.
pub fn decode(magic: &[u8; 4], data: &[u8]) -> io::Result<Frame> {
    if data.len() < HEADER_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Frame too short: {} bytes (need at least {})", data.len(), HEADER_SIZE),
        ));
    }

    if &data[0..4] != magic {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Bad magic: {:?} (expected {:?})", &data[0..4], magic),
        ));
    }

    let code = data[4];
    let body_len = u32::from_be_bytes([data[5], data[6], data[7], data[8]]);

    if body_len > MAX_BODY_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Body too large: {} bytes (max {})", body_len, MAX_BODY_SIZE),
        ));
    }

    let expected_total = HEADER_SIZE + body_len as usize;
    if data.len() < expected_total {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Frame truncated: have {} bytes, need {}", data.len(), expected_total),
        ));
    }

    let checksum = &data[9..13];
    let body = &data[HEADER_SIZE..expected_total];

    let hash = Blake2b256::digest(body);
    if &hash[..4] != checksum {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "Checksum mismatch"));
    }

    Ok(Frame {
        code,
        body: body.to_vec(),
    })
}

/// Read a complete frame from an async reader.
///
/// # Contract
/// - **Precondition**: reader is positioned at the start of a frame.
/// - **Postcondition**: returns a validated Frame, or error on I/O, bad magic, bad checksum, or oversize.
pub async fn read_frame(
    reader: &mut (impl tokio::io::AsyncReadExt + Unpin),
    magic: &[u8; 4],
) -> io::Result<Frame> {
    let mut header = [0u8; HEADER_SIZE];
    reader.read_exact(&mut header).await?;

    if &header[0..4] != magic {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Bad magic: {:?}", &header[0..4]),
        ));
    }

    let code = header[4];
    let body_len = u32::from_be_bytes([header[5], header[6], header[7], header[8]]);

    if body_len > MAX_BODY_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Body too large: {} bytes", body_len),
        ));
    }

    let checksum = &header[9..13];

    let mut body = vec![0u8; body_len as usize];
    if body_len > 0 {
        reader.read_exact(&mut body).await?;
    }

    let hash = Blake2b256::digest(&body);
    if &hash[..4] != checksum {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "Checksum mismatch"));
    }

    Ok(Frame { code, body })
}

/// Write a frame to an async writer.
///
/// # Contract
/// - **Postcondition**: the full encoded frame is written and flushed.
pub async fn write_frame(
    writer: &mut (impl tokio::io::AsyncWriteExt + Unpin),
    magic: &[u8; 4],
    frame: &Frame,
) -> io::Result<()> {
    let bytes = encode(magic, frame);
    writer.write_all(&bytes).await?;
    writer.flush().await
}
