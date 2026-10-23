//! GPUI-free application coordination. All client work runs on a background executor.

pub mod diagnostics;
pub mod fence;
pub mod navigation;
pub mod observation;
pub mod preferences;
pub mod redaction;

pub use cos_client::SnapshotHandle;
pub use diagnostics::{DiagnosticBatch, DiagnosticEvent, DiagnosticKind, EventGap};
pub use fence::{
    AuthorizedCommandRead, ObservationLease, PublicationChange, PublicationSubscription,
    TaggedRead, VisibilityFence,
};
pub use observation::{
    CommandPresentation, CommandRead, CommandReadError, CommandReadView, DeadlineScheduler,
    ExitWatch, InvalidCommandReason, ObservationHandle, ObservationPublication,
    ObservationSessionOwner, OwnerClock, OwnerReply, PublicationConnection, PublicationError,
    PublicationNotice, PublicationSource, ReadUnavailable, ReplacementError, drive,
};
pub use redaction::{
    MAX_DIAGNOSTIC_LINE_BYTES, REDACTED, redact_diagnostic_text, redact_diagnostic_text_within,
};

/// Application ownership is separate from each window's local presentation state.
///
/// App-core owns exactly one observation handle; a missing handle is a real
/// unconfigured/unsupported Runtime, never a fixture fallback. Windows read a
/// complete publication through this handle and never touch cos-client.
pub struct AppCore {
    /// Window-local navigation state.
    pub navigation: navigation::Navigation,
    observation: Option<ObservationHandle>,
    unavailable_reason: &'static str,
}

impl AppCore {
    /// Explicit disconnected UI state; no fixture fallback is implied.
    pub fn disconnected(endpoint_configured: bool) -> Self {
        Self {
            navigation: navigation::Navigation::default(),
            observation: None,
            unavailable_reason: if endpoint_configured {
                "Disconnected — production transport not implemented"
            } else {
                "Disconnected — Runtime not configured"
            },
        }
    }

    /// Attach the process-wide observation handle owned by the binary.
    pub fn with_observation(handle: ObservationHandle) -> Self {
        Self {
            navigation: navigation::Navigation::default(),
            observation: Some(handle),
            unavailable_reason: "",
        }
    }

    /// The shared observation handle, when observation is configured.
    pub fn observation(&self) -> Option<&ObservationHandle> {
        self.observation.as_ref()
    }

    /// Why observation is unavailable.
    pub fn unavailable_reason(&self) -> &'static str {
        self.unavailable_reason
    }
}

#[cfg(test)]
mod tests {
    use super::AppCore;

    #[test]
    fn a_disconnected_core_exposes_no_observation_handle() {
        let core = AppCore::disconnected(false);
        assert!(core.observation().is_none());
        assert_eq!(
            core.unavailable_reason(),
            "Disconnected — Runtime not configured"
        );
    }

    #[test]
    fn a_configured_core_reports_the_transport_gap_honestly() {
        let core = AppCore::disconnected(true);
        assert!(core.observation().is_none());
        assert_eq!(
            core.unavailable_reason(),
            "Disconnected — production transport not implemented"
        );
    }
}
