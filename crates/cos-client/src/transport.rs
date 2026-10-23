//! Byte-oriented transport boundary.
//!
//! The client owns protocol encoding, frame-size policy and recovery; a
//! transport only moves encoded frames. This keeps `CosClient` independent of
//! any concrete transport (fixture, loopback HTTP or a future adapter).

/// Transport-level failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    /// The peer is gone or the transport was closed.
    #[error("transport disconnected")]
    Disconnected,
    /// The bounded receive queue overflowed; the stream must be cancelled.
    #[error("receive queue overflow")]
    QueueOverflow,
    /// I/O failure with a safe message.
    #[error("transport i/o failure: {0}")]
    Io(String),
}

/// A duplex byte transport for encoded protocol frames.
pub trait Transport: Send {
    /// Send one encoded frame.
    fn send(&mut self, frame: Vec<u8>) -> Result<(), TransportError>;

    /// Receive one encoded frame if one is available.
    ///
    /// Returns `Ok(None)` when the receive queue is currently empty; this is not
    /// an error and never blocks.
    fn poll(&mut self) -> Result<Option<Vec<u8>>, TransportError>;

    /// Drop queued inbound frames, cancelling the current stream.
    fn reset_stream(&mut self);
}
