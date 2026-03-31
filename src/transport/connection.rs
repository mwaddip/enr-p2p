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
//! - Invariant: no bytes are lost between handshake and first frame. BufReader ensures
//!   that excess bytes read during handshake remain available for frame reading.

use crate::transport::frame::{self, Frame};
use crate::transport::handshake::{self, HandshakeConfig, PeerSpec};
use std::io;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::time::{timeout, Duration};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
const HANDSHAKE_BUF_SIZE: usize = 8192;

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
        let (read_half, mut write_half) = stream.into_split();
        let mut reader = BufReader::new(read_half);

        // Send our handshake
        let hs_bytes = handshake::build(config);
        write_half.write_all(&hs_bytes).await?;
        write_half.flush().await?;

        // Read peer's handshake through BufReader
        let peer_spec = timeout(HANDSHAKE_TIMEOUT, read_handshake(&mut reader))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Handshake timeout"))??;

        // Validate
        handshake::validate_peer(&peer_spec, &config.network)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

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
        let (read_half, mut write_half) = stream.into_split();
        let mut reader = BufReader::new(read_half);

        // Read peer's handshake first through BufReader
        let peer_spec = timeout(HANDSHAKE_TIMEOUT, read_handshake(&mut reader))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Handshake timeout"))??;

        // Validate before sending ours
        handshake::validate_peer(&peer_spec, &config.network)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        // Send our handshake
        let hs_bytes = handshake::build(config);
        write_half.write_all(&hs_bytes).await?;
        write_half.flush().await?;

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

/// Read a handshake from a buffered reader.
/// Uses a bulk read then parses — excess bytes stay in the BufReader's buffer.
async fn read_handshake(reader: &mut BufReader<OwnedReadHalf>) -> io::Result<PeerSpec> {
    // Fill the internal buffer without consuming — this ensures we have
    // enough data for the handshake while keeping excess bytes buffered.
    let buf = reader.fill_buf().await?;
    if buf.is_empty() {
        return Err(io::Error::new(io::ErrorKind::ConnectionReset, "Empty handshake"));
    }

    // Parse the handshake from whatever's in the buffer
    let buf_snapshot = buf.to_vec();
    let spec = handshake::parse(&buf_snapshot)?;

    // We need to consume exactly the handshake bytes, not the entire buffer.
    // Re-build the handshake to know its exact size (or measure during parse).
    // For now: re-serialize what we parsed and measure the size.
    // Simpler approach: the parse consumed from a Cursor — measure cursor position.
    let consumed = measure_handshake_size(&buf_snapshot)?;
    reader.consume(consumed);

    Ok(spec)
}

/// Measure the byte size of a handshake by parsing it with a cursor.
/// Delegated to the handshake module to avoid async trait ambiguity.
fn measure_handshake_size(data: &[u8]) -> io::Result<usize> {
    handshake::measure_size(data)
}
