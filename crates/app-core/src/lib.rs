//! GPUI-free application coordination. All client work runs on a background executor.

pub mod crash_report;
pub mod diagnostics;
pub mod fence;
pub mod history;
pub mod media_presentation;
pub mod navigation;
pub mod observation;
pub mod panic_report;
pub mod preferences;
pub mod redaction;
pub mod support_report;

/// Opaque server-issued continuation cursor of one history page.
///
/// Re-exported because it is part of the history boundary app-core exposes to
/// ui-model; ui-model does not depend on the client crate.
pub use cos_client::HistoryCursor;
pub use cos_client::SnapshotHandle;
/// Value-free cooperative cancellation token for one media fetch.
///
/// Re-exported so the GPUI boundary can cancel a background media resolution without
/// depending on `cos-client` directly.
pub use cos_client::media::Cancellation as MediaFetchCancellation;
pub use cos_client::{
    Catalog, CatalogDocument, CatalogExecutionLimits, CatalogGraphRef, CatalogInputLimits,
    CatalogParameter, CatalogParameterBounds, CatalogParameterChoice, CatalogParameterValue,
    CatalogRecipe, CatalogRecipeRef, CatalogServer,
};
pub use crash_report::{
    CRASH_REPORT_DIR, CrashCategory, CrashReportInput, CrashReportMeta, CrashReportStore,
    CrashStoreError, MAX_CRASH_REPORT_BYTES, MAX_CRASH_REPORTS, REPORT_EXTENSION, REPORT_PREFIX,
};
pub use diagnostics::{
    DiagnosticBatch, DiagnosticCounts, DiagnosticEvent, DiagnosticKind, EventGap,
};
pub use fence::{
    AuthorizedCommandRead, ObservationLease, PublicationChange, PublicationSubscription,
    TaggedRead, VisibilityFence,
};
pub use history::{
    HISTORY_MAX_ENTRIES, HISTORY_PAGE_LIMIT, HistoryEntry, HistoryError, HistoryFetchReply,
    HistoryPublication, HistoryRequest, HistoryStatus,
};
pub use media_presentation::{
    AdmittedMedia, MediaAvailability, MediaIdentity, MediaPresentation, MediaRepository,
    MediaSelection, MediaSessionContext, MediaSource, MediaUnavailable, MediaUnavailableCause,
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
pub use support_report::{
    DiagnosticsSummary, ExportData, MAX_REPORT_LINES, MAX_SUPPORT_REPORT_BYTES, ReportLimits,
    ReportValue, SupportReport, checked_export,
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
                "Disconnected — Runtime endpoint configured but not reachable or authorized"
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
            "Disconnected — Runtime endpoint configured but not reachable or authorized"
        );
        // The Runtime transport now exists; the label must stop claiming it does
        // not, while still stating that the endpoint is not serving this session.
        assert!(!core.unavailable_reason().contains("not implemented"));
    }
}
