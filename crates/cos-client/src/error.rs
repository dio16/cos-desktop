//! Client-side errors.

use desktop_protocol::ProtocolError;

/// Client failure.
///
/// A transport failure is never a task failure: it is surfaced as a connection
/// state plus this error, while the last-known authoritative projection is kept
/// and labelled stale.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ClientError {
    /// Transport-level failure (not a protocol or task outcome).
    #[error("transport failure: {0}")]
    Transport(String),
    /// A frame exceeded the negotiated maximum and was rejected before parsing.
    #[error("frame of {declared} bytes exceeds the negotiated maximum of {limit} bytes")]
    MessageTooLarge {
        /// Declared frame length.
        declared: u64,
        /// Negotiated limit.
        limit: u64,
    },
    /// The bounded receive queue overflowed.
    #[error("receive queue overflow; the stream was cancelled and a full resync is required")]
    QueueOverflow,
    /// Malformed or contradictory protocol content.
    #[error("protocol violation: {0}")]
    Protocol(String),
    /// No mutually supported protocol/schema pair.
    #[error("version incompatible: {0}")]
    VersionIncompatible(String),
    /// The session is not in a state that allows this operation.
    #[error("not connected: {0}")]
    NotConnected(String),
    /// The command envelope is invalid or unknown.
    #[error("invalid command: {0}")]
    InvalidCommand(String),
    /// Structured server error.
    #[error("server error: {0}")]
    Server(Box<ProtocolError>),
    /// Fixture definition or loading failure.
    #[error("fixture error: {0}")]
    Fixture(String),
}

impl ClientError {
    /// Wrap a structured server error (boxed to keep `Result` small).
    pub fn server(error: ProtocolError) -> Self {
        Self::Server(Box::new(error))
    }

    /// Whether the error invalidates the live session and requires resync.
    pub fn requires_resync(&self) -> bool {
        matches!(
            self,
            Self::MessageTooLarge { .. }
                | Self::QueueOverflow
                | Self::Protocol(_)
                | Self::Transport(_)
        )
    }
}
