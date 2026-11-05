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

#[cfg(test)]
mod tests {
    use super::ClientError;
    use desktop_protocol::{ErrorCode, ProtocolError};

    fn server_error(message: &str) -> ProtocolError {
        ProtocolError {
            code: ErrorCode::Forbidden,
            message: message.to_string(),
            correlation_id: None,
            command_id: None,
            scope_id: None,
            retry_after_ms: None,
            field_errors: Vec::new(),
            current_revision: None,
            resync_reason: None,
        }
    }

    /// Every variant, so a change to the rule shows up as a failing row rather than being absorbed
    /// by whichever variants the test happened to cover.
    #[test]
    fn the_resync_rule_covers_every_variant() {
        let cases: Vec<(&str, ClientError, bool)> = vec![
            (
                "transport",
                ClientError::Transport("connection reset".to_string()),
                true,
            ),
            (
                "message too large",
                ClientError::MessageTooLarge {
                    declared: 1024,
                    limit: 512,
                },
                true,
            ),
            ("queue overflow", ClientError::QueueOverflow, true),
            (
                "protocol",
                ClientError::Protocol("contradictory frame".to_string()),
                true,
            ),
            (
                "version incompatible",
                ClientError::VersionIncompatible("no shared pair".to_string()),
                false,
            ),
            (
                "not connected",
                ClientError::NotConnected("no session".to_string()),
                false,
            ),
            (
                "invalid command",
                ClientError::InvalidCommand("unknown operation".to_string()),
                false,
            ),
            (
                "server error",
                ClientError::server(server_error("forbidden")),
                false,
            ),
            (
                "fixture",
                ClientError::Fixture("scenario invalid".to_string()),
                false,
            ),
        ];
        assert_eq!(cases.len(), 9, "the enum has nine variants");
        for (label, error, expected) in cases {
            assert_eq!(
                error.requires_resync(),
                expected,
                "{label} must{} require a resync",
                if expected { "" } else { " not" }
            );
        }
    }

    /// A structured server error is answered, not resynced: the denial is the answer, and throwing
    /// the session away would discard a valid binding.
    #[test]
    fn a_server_error_that_asks_for_resync_is_still_answered_by_code_not_by_variant() {
        // `requires_resync` is about the *transport* class, so even a server error whose code is
        // RESYNC_REQUIRED does not make the variant a resync error. The protocol carries that reason
        // in the code for the caller to read.
        let mut error = server_error("resync needed");
        error.code = ErrorCode::ResyncRequired;
        let wrapped = ClientError::server(error.clone());
        assert!(!wrapped.requires_resync());
        // The code survives, so a caller that wants to act on it can.
        match &wrapped {
            ClientError::Server(inner) => assert_eq!(inner.code, ErrorCode::ResyncRequired),
            other => panic!("expected a server error, got {other:?}"),
        }
    }

    /// The raw message is **not** safe for publication: the safe category is derived in `app-core`,
    /// which reduces free text. Pinning that boundary keeps a future change from treating this
    /// `Display` as publishable.
    #[test]
    fn the_display_of_a_raw_error_is_not_a_safe_publication_form() {
        let secret = "SEC2-SYNTHETIC-CREDENTIAL-7f3a9c";
        let wrapped = ClientError::server(server_error(secret));
        let rendered = format!("{wrapped}");
        assert!(
            rendered.contains(secret),
            "the raw form carries the server text; redaction is the caller's job"
        );
        // The same holds for the free-text variants, which is why nothing downstream prints them
        // as-is.
        assert!(format!("{}", ClientError::Protocol(secret.to_string())).contains(secret));
        assert!(format!("{}", ClientError::Transport(secret.to_string())).contains(secret));
    }
}
