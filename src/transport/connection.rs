//! Per-connection async read/write loops.
//!
//! # Contract
//! - `Connection::outbound`: performs handshake as initiator (send then receive).
//!   Precondition: `stream` is a connected TCP stream.
//!   Postcondition: both sides have exchanged handshakes; peer's PeerSpec is available.
//! - `Connection::inbound`: performs handshake as responder (receive then send).
//!   Precondition: `stream` is a connected TCP stream.
//!   Postcondition: both sides have exchanged handshakes; peer's PeerSpec is available.
//! - `Connection::read_frame`: reads the next frame from the stream.
//!   Postcondition: returns a validated `Frame` or error.
//! - `Connection::write_frame`: writes a frame to the stream.
//!   Postcondition: frame is fully written and flushed.
//! - Invariant: the connection's magic is fixed at construction time and never changes.
//! - Invariant: BufReader is created after handshake completes with an empty internal
//!   buffer. During handshake, no frame data can arrive (both sides block), so no
//!   bytes are lost between handshake and first frame.

use crate::transport::frame::{self, Frame};
use crate::transport::handshake::{self, HandshakeConfig, PeerSpec};
use std::io;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::time::{timeout, Duration};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
const HANDSHAKE_MAX_SIZE: usize = 8192;

/// An active P2P connection with completed handshake.
/// Uses BufReader to ensure no bytes are lost between handshake and framed messages.
pub struct Connection {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    magic: [u8; 4],
    peer_spec: PeerSpec,
}

impl Connection {
    /// Establish a connection by performing the handshake as initiator (outbound).
    ///
    /// Sends our handshake, reads the peer's handshake, validates it.
    pub async fn outbound(
        stream: TcpStream,
        config: &HandshakeConfig,
    ) -> io::Result<Self> {
        let magic = config.network.magic();
        let (mut read_half, mut write_half) = stream.into_split();

        // Send our handshake
        let hs_bytes = handshake::build(config);
        write_half.write_all(&hs_bytes).await?;
        write_half.flush().await?;

        // Read peer's handshake from raw stream (accumulates TCP segments)
        let peer_spec = timeout(HANDSHAKE_TIMEOUT, read_handshake_raw(&mut read_half))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Handshake timeout"))??;

        // Validate
        handshake::validate_peer(&peer_spec, &config.network)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        // Wrap in BufReader after handshake — internal buffer starts empty
        let reader = BufReader::new(read_half);
        Ok(Self { reader, writer: write_half, magic, peer_spec })
    }

    /// Accept a connection by performing the handshake as responder (inbound).
    ///
    /// Reads the peer's handshake first, validates it, then sends ours.
    pub async fn inbound(
        stream: TcpStream,
        config: &HandshakeConfig,
    ) -> io::Result<Self> {
        let magic = config.network.magic();
        let (mut read_half, mut write_half) = stream.into_split();

        // Read peer's handshake from raw stream (accumulates TCP segments)
        let peer_spec = timeout(HANDSHAKE_TIMEOUT, read_handshake_raw(&mut read_half))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Handshake timeout"))??;

        // Validate before sending ours
        handshake::validate_peer(&peer_spec, &config.network)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        // Send our handshake
        let hs_bytes = handshake::build(config);
        write_half.write_all(&hs_bytes).await?;
        write_half.flush().await?;

        // Wrap in BufReader after handshake — internal buffer starts empty
        let reader = BufReader::new(read_half);
        Ok(Self { reader, writer: write_half, magic, peer_spec })
    }

    /// Read the next message frame.
    pub async fn read_frame(&mut self) -> io::Result<Frame> {
        frame::read_frame(&mut self.reader, &self.magic).await
    }

    /// Write a message frame.
    pub async fn write_frame(&mut self, f: &Frame) -> io::Result<()> {
        frame::write_frame(&mut self.writer, &self.magic, f).await
    }

    /// Get the peer's specification from the handshake.
    pub fn peer_spec(&self) -> &PeerSpec {
        &self.peer_spec
    }

    /// Split the connection into separate read and write halves.
    pub fn split(self) -> (BufReader<OwnedReadHalf>, OwnedWriteHalf, [u8; 4], PeerSpec) {
        (self.reader, self.writer, self.magic, self.peer_spec)
    }
}

/// Read a handshake by accumulating TCP segments until parsing succeeds.
///
/// Reads from the raw `OwnedReadHalf` before it's wrapped in `BufReader`.
/// During handshake, no frame data can arrive — both sides block until
/// handshake completes — so there are no excess bytes to worry about.
async fn read_handshake_raw(read_half: &mut OwnedReadHalf) -> io::Result<PeerSpec> {
    let mut buf = Vec::with_capacity(256);
    let mut tmp = [0u8; 1024];

    loop {
        let n = read_half.read(&mut tmp).await?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "Connection closed during handshake",
            ));
        }
        buf.extend_from_slice(&tmp[..n]);

        if buf.len() > HANDSHAKE_MAX_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Handshake exceeds maximum size",
            ));
        }

        match handshake::parse(&buf) {
            Ok(spec) => return Ok(spec),
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => continue,
            Err(e) => return Err(e),
        }
    }
}
