//! Protocol error envelope and error/resync vocabularies.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ids::{CommandId, CorrelationId, Counter, ScopeId};
use crate::macros::wire_enum;

wire_enum! {
    /// Structured error codes. Transport failure is never a task failure and
    /// carries no code here.
    pub enum ErrorCode {
        Unauthorized => "UNAUTHORIZED",
        Forbidden => "FORBIDDEN",
        ValidationFailed => "VALIDATION_FAILED",
        RevisionConflict => "REVISION_CONFLICT",
        UnsupportedCommand => "UNSUPPORTED_COMMAND",
        VersionIncompatible => "VERSION_INCOMPATIBLE",
        ResyncRequired => "RESYNC_REQUIRED",
        QueueOverflow => "QUEUE_OVERFLOW",
        MessageTooLarge => "MESSAGE_TOO_LARGE",
        CommandIdConflict => "COMMAND_ID_CONFLICT",
        ReceiptExpired => "RECEIPT_EXPIRED",
        MediaUnavailable => "MEDIA_UNAVAILABLE",
        InternalError => "INTERNAL_ERROR",
    }
}

wire_enum! {
    /// Why a full resync (fresh atomic snapshot) is required.
    pub enum ResyncReason {
        ExpiredCursor => "EXPIRED_CURSOR",
        UnknownScope => "UNKNOWN_SCOPE",
        AuthorizationChanged => "AUTHORIZATION_CHANGED",
        EpochChanged => "EPOCH_CHANGED",
        HistoryUnavailable => "HISTORY_UNAVAILABLE",
        ProtocolViolation => "PROTOCOL_VIOLATION",
        QueueOverflow => "QUEUE_OVERFLOW",
        MessageTooLarge => "MESSAGE_TOO_LARGE",
        UnsupportedStateEvent => "UNSUPPORTED_STATE_EVENT",
        Internal => "INTERNAL",
    }
}

wire_enum! {
    /// Definitive rejection reasons carried by a rejected receipt.
    pub enum RejectReason {
        ValidationFailed => "VALIDATION_FAILED",
        Unauthorized => "UNAUTHORIZED",
        Forbidden => "FORBIDDEN",
        RevisionConflict => "REVISION_CONFLICT",
        UnsupportedCommand => "UNSUPPORTED_COMMAND",
        CommandIdConflict => "COMMAND_ID_CONFLICT",
        InternalError => "INTERNAL_ERROR",
    }
}

/// Field-level validation detail.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldError {
    /// Field name.
    pub field: String,
    /// Safe message.
    pub message: String,
}

/// Structured protocol error.
///
/// Internal errors must omit paths, secrets, raw prompts and stack internals
/// from normal UI; `message` is a safe message by contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolError {
    /// Structured code.
    pub code: ErrorCode,
    /// Safe human-readable message.
    pub message: String,
    /// Diagnostic correlation identity when the server supplies one.
    #[serde(default)]
    pub correlation_id: Option<CorrelationId>,
    /// Command this error answers, when applicable.
    #[serde(default)]
    pub command_id: Option<CommandId>,
    /// Observation scope, when applicable.
    #[serde(default)]
    pub scope_id: Option<ScopeId>,
    /// Server-requested retry delay.
    #[serde(default)]
    pub retry_after_ms: Option<u64>,
    /// Field-level validation details.
    #[serde(default)]
    pub field_errors: Vec<FieldError>,
    /// Current projection revision for conflict errors.
    #[serde(default)]
    pub current_revision: Option<Counter>,
    /// Resync reason for `RESYNC_REQUIRED` errors.
    #[serde(default)]
    pub resync_reason: Option<ResyncReason>,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ProtocolError {}
