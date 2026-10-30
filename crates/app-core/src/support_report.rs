//! Bounded, redacted Desktop-local support report.
//!
//! The report restates what the existing observation publication and diagnostic
//! ring already expose, in a fixed field set, so a support reader can see the
//! Desktop's own view without any new authority. It never reads a Runtime
//! database, log file, credential store or arbitrary path, never derives Task or
//! Domain truth, and never invents a value: anything the Desktop cannot observe
//! is reported as `Unknown` with the reason, not as a zero.
//!
//! DD-22 ratifies bounded retention, non-exposure of credentials/locators/private
//! paths and typed diagnostic categories. The export format, destination,
//! retention period and consent flow are **not** decided, so this module renders
//! a string for an in-process reader and provides no file or network export.

use crate::diagnostics::{
    DiagnosticBatch, DiagnosticEvent, DiagnosticKind, EventGap, MAX_DIAGNOSTIC_EVENTS,
};
use crate::observation::PublicationSource;
use crate::observation::{ObservationPublication, PublicationConnection, PublicationError};
use crate::redaction::{MAX_DIAGNOSTIC_LINE_BYTES, redact_diagnostic_text_within};

/// Fixed number of report lines the renderer emits.
///
/// The bound is derived from the existing ratified line budget rather than a new
/// retention policy: one line per field, each already capped by
/// [`MAX_DIAGNOSTIC_LINE_BYTES`].
pub const MAX_REPORT_LINES: usize = 24;

/// Hard ceiling for one rendered report, in bytes.
pub const MAX_SUPPORT_REPORT_BYTES: usize = MAX_REPORT_LINES * MAX_DIAGNOSTIC_LINE_BYTES;

/// A field the Desktop either observes directly or cannot observe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReportValue<T> {
    /// Observed through an existing authoritative path.
    Available(T),
    /// Not available; the reason is a fixed token, never a guessed value.
    Unknown(&'static str),
}

/// Per-category event counts. Categories are the existing safe kinds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiagnosticCounts {
    /// Events without a separate category.
    pub activity: usize,
    /// Connection transitions.
    pub connection: usize,
    /// Errors reduced to a safe category.
    pub error: usize,
    /// Requested resynchronizations.
    pub resync: usize,
}

/// Bounded summary of the retained diagnostic ring for one reader cursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticsSummary {
    /// Events retained for this reader after the cursor.
    pub retained_events: usize,
    /// Latest sequence ever appended, or zero before the first event.
    pub latest_sequence: u64,
    /// Oldest retained sequence, when anything is retained.
    pub oldest_sequence: Option<u64>,
    /// Exact lost interval, when events fell out of the ring.
    pub event_gap: Option<EventGap>,
    /// True once the sequence space is exhausted.
    pub exhausted: bool,
    /// Counts per safe category across the retained events.
    pub counts: DiagnosticCounts,
}

impl DiagnosticsSummary {
    /// Summarize one bounded batch. The event vector is already capped by the
    /// ring, so this never grows with input.
    pub fn from_batch(batch: &DiagnosticBatch) -> Self {
        let mut counts = DiagnosticCounts::default();
        for event in &batch.events {
            match event.kind {
                DiagnosticKind::Activity => counts.activity += 1,
                DiagnosticKind::Connection(_) => counts.connection += 1,
                DiagnosticKind::Error(_) => counts.error += 1,
                DiagnosticKind::Resync => counts.resync += 1,
            }
        }
        Self {
            retained_events: batch.events.len().min(MAX_DIAGNOSTIC_EVENTS),
            latest_sequence: batch.latest_sequence,
            oldest_sequence: batch
                .events
                .first()
                .map(|event: &DiagnosticEvent| event.sequence),
            event_gap: batch.event_gap,
            exhausted: batch.exhausted,
            counts,
        }
    }
}

/// Fixed limits the report states about itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReportLimits {
    /// Maximum diagnostic events the ring retains.
    pub max_diagnostic_events: usize,
    /// Maximum bytes from one report line.
    pub max_line_bytes: usize,
    /// Maximum bytes of one rendered report.
    pub max_report_bytes: usize,
    /// Maximum lines of one rendered report.
    pub max_report_lines: usize,
}

impl Default for ReportLimits {
    fn default() -> Self {
        Self {
            max_diagnostic_events: MAX_DIAGNOSTIC_EVENTS,
            max_line_bytes: MAX_DIAGNOSTIC_LINE_BYTES,
            max_report_bytes: MAX_SUPPORT_REPORT_BYTES,
            max_report_lines: MAX_REPORT_LINES,
        }
    }
}

/// Desktop-local support report.
///
/// Every field names its own source: values come from one
/// [`ObservationPublication`] the window also reads and from the bounded
/// diagnostic ring. `runtime_version` and `protocol_version` are `Unknown`
/// because no negotiated pair is exposed through observation; they are never
/// filled from the Desktop's own Cargo version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupportReport {
    /// Desktop package version.
    pub desktop_version: &'static str,
    /// Negotiated Runtime version, only when observation exposes one.
    pub runtime_version: ReportValue<String>,
    /// Negotiated protocol version, only when observation exposes one.
    pub protocol_version: ReportValue<String>,
    /// Safe connection category.
    pub connection: ReportValue<PublicationConnection>,
    /// Safe publication source category.
    pub source: ReportValue<PublicationSource>,
    /// Whether the visible projection may be stale.
    pub stale: Option<bool>,
    /// Owner-local projection-commit counter, never Domain truth.
    pub projection_revision: ReportValue<String>,
    /// Owner generation of the published record.
    pub session_generation: Option<u64>,
    /// Monotonic publication version.
    pub publication_version: Option<u64>,
    /// Safe error category of the last failure.
    pub last_error: Option<PublicationError>,
    /// Bounded diagnostic summary, or `Unknown` when the ring was never read.
    pub diagnostics: ReportValue<DiagnosticsSummary>,
    /// Referenced command views; a count only, never the identities.
    pub command_views: Option<usize>,
    /// Why observation is unavailable, when it is.
    pub unavailable: Option<&'static str>,
    /// Fixed limits.
    pub limits: ReportLimits,
}

impl SupportReport {
    /// Build the report from the publication and diagnostic batch a reader
    /// already holds. Nothing else is read.
    ///
    /// `desktop_version` is supplied by the binary that owns the Desktop package
    /// version; app-core must not report its own crate version as the Desktop's.
    pub fn from_publication(
        publication: &ObservationPublication,
        diagnostics: &DiagnosticBatch,
        desktop_version: &'static str,
    ) -> Self {
        Self {
            desktop_version,
            runtime_version: ReportValue::Unknown(
                "Runtime version is not exposed through observation",
            ),
            protocol_version: ReportValue::Unknown("no protocol version pair was negotiated"),
            connection: ReportValue::Available(publication.connection),
            source: ReportValue::Available(publication.source),
            stale: Some(publication.stale),
            projection_revision: match &publication.revision {
                Some(revision) => ReportValue::Available(revision.as_str().to_owned()),
                None => ReportValue::Unknown("no projection commit was published"),
            },
            session_generation: Some(publication.session_generation),
            publication_version: Some(publication.publication_version),
            last_error: publication.last_error.clone(),
            diagnostics: ReportValue::Available(DiagnosticsSummary::from_batch(diagnostics)),
            command_views: Some(publication.command_statuses.len()),
            unavailable: None,
            limits: ReportLimits::default(),
        }
    }

    /// Report for a Desktop with no observation handle at all.
    pub fn unavailable(reason: &'static str, desktop_version: &'static str) -> Self {
        Self {
            desktop_version,
            runtime_version: ReportValue::Unknown("no observation session exists"),
            protocol_version: ReportValue::Unknown("no observation session exists"),
            connection: ReportValue::Available(PublicationConnection::Disconnected),
            source: ReportValue::Available(PublicationSource::Unavailable),
            stale: None,
            projection_revision: ReportValue::Unknown("no observation session exists"),
            session_generation: None,
            publication_version: None,
            last_error: None,
            diagnostics: ReportValue::Unknown("no diagnostic ring was read"),
            command_views: None,
            unavailable: Some(reason),
            limits: ReportLimits::default(),
        }
    }

    /// Render the report as bounded, redacted `key: value` lines.
    ///
    /// Every line passes through the existing diagnostic redactor, so a value
    /// that somehow carried a token, signed locator or private path is removed
    /// before it can leave the process. The result never exceeds
    /// [`MAX_SUPPORT_REPORT_BYTES`]; a truncation is stated, never silent.
    pub fn render(&self) -> String {
        let mut lines: Vec<String> = Vec::with_capacity(MAX_REPORT_LINES);
        lines.push(format!("Desktop version: {}", self.desktop_version));
        lines.push(format!(
            "Runtime version: {}",
            value_line(&self.runtime_version)
        ));
        lines.push(format!(
            "Protocol version: {}",
            value_line(&self.protocol_version)
        ));
        lines.push(format!("Connection: {}", connection_line(&self.connection)));
        lines.push(format!("Source: {}", source_line(&self.source)));
        lines.push(format!(
            "Stale projection: {}",
            optional(self.stale.map(|stale| if stale { "yes" } else { "no" }))
        ));
        lines.push(format!(
            "Projection revision: {}",
            value_line(&self.projection_revision)
        ));
        lines.push(format!(
            "Session generation: {}",
            optional(self.session_generation)
        ));
        lines.push(format!(
            "Publication version: {}",
            optional(self.publication_version)
        ));
        lines.push(format!(
            "Last error: {}",
            optional(self.last_error.as_ref().map(error_line))
        ));
        lines.push(format!("Command views: {}", optional(self.command_views)));
        lines.push(format!(
            "Unavailable: {}",
            self.unavailable.unwrap_or("none")
        ));
        let summary = match &self.diagnostics {
            ReportValue::Available(summary) => summary,
            ReportValue::Unknown(reason) => {
                for label in [
                    "Retained events",
                    "Latest sequence",
                    "Oldest retained",
                    "Event gap",
                    "Sequence exhausted",
                    "Event counts",
                ] {
                    lines.push(format!("{label}: unknown ({reason})"));
                }
                lines.push(format!(
                    "Limits: events {} / line {} bytes / report {} bytes / {} lines",
                    self.limits.max_diagnostic_events,
                    self.limits.max_line_bytes,
                    self.limits.max_report_bytes,
                    self.limits.max_report_lines
                ));
                return self.finish(lines);
            }
        };
        lines.push(format!("Retained events: {}", summary.retained_events));
        lines.push(format!("Latest sequence: {}", summary.latest_sequence));
        lines.push(format!(
            "Oldest retained: {}",
            optional(summary.oldest_sequence)
        ));
        lines.push(format!(
            "Event gap: {}",
            match summary.event_gap {
                Some(gap) => format!("{}-{}", gap.from, gap.to),
                None => "none".to_owned(),
            }
        ));
        lines.push(format!("Sequence exhausted: {}", summary.exhausted));
        lines.push(format!(
            "Event counts: activity {} / connection {} / error {} / resync {}",
            summary.counts.activity,
            summary.counts.connection,
            summary.counts.error,
            summary.counts.resync
        ));
        lines.push(format!(
            "Limits: events {} / line {} bytes / report {} bytes / {} lines",
            self.limits.max_diagnostic_events,
            self.limits.max_line_bytes,
            self.limits.max_report_bytes,
            self.limits.max_report_lines
        ));
        self.finish(lines)
    }

    fn finish(&self, lines: Vec<String>) -> String {
        let mut out = String::with_capacity(MAX_SUPPORT_REPORT_BYTES);
        for (index, line) in lines.iter().take(MAX_REPORT_LINES).enumerate() {
            if index > 0 {
                out.push('\n');
            }
            out.push_str(&redact_diagnostic_text_within(
                line,
                MAX_DIAGNOSTIC_LINE_BYTES,
            ));
        }
        // Per-line caps keep the fixed field set inside the report bound by
        // construction; the clamp is the last line of defence if the field set
        // ever grows past what the bound allows.
        if out.len() > MAX_SUPPORT_REPORT_BYTES {
            let mut limit = MAX_SUPPORT_REPORT_BYTES;
            while !out.is_char_boundary(limit) {
                limit -= 1;
            }
            out.truncate(limit);
        }
        out
    }
}

fn optional<T: ToString>(value: Option<T>) -> String {
    value.map_or_else(|| "unknown".to_owned(), |value| value.to_string())
}

fn value_line<T: ToString>(value: &ReportValue<T>) -> String {
    match value {
        ReportValue::Available(value) => {
            redact_diagnostic_text_within(&value.to_string(), MAX_DIAGNOSTIC_LINE_BYTES)
        }
        ReportValue::Unknown(reason) => format!("unknown ({reason})"),
    }
}

fn connection_line(connection: &ReportValue<PublicationConnection>) -> String {
    match connection {
        ReportValue::Available(connection) => match connection {
            PublicationConnection::Disconnected => "not connected",
            PublicationConnection::Connecting => "connecting",
            PublicationConnection::Observing => "observing",
            PublicationConnection::Recovering => "recovering",
            PublicationConnection::Unauthorized => "unauthorized",
            PublicationConnection::Incompatible => "incompatible",
        }
        .to_owned(),
        ReportValue::Unknown(reason) => format!("unknown ({reason})"),
    }
}

fn source_line(source: &ReportValue<PublicationSource>) -> String {
    match source {
        ReportValue::Available(source) => match source {
            PublicationSource::Runtime => "runtime",
            PublicationSource::Fixture => "fixture",
            PublicationSource::Unavailable => "unavailable",
        }
        .to_owned(),
        ReportValue::Unknown(reason) => format!("unknown ({reason})"),
    }
}

fn error_line(error: &PublicationError) -> &'static str {
    match error {
        PublicationError::Transport => "transport",
        PublicationError::Protocol => "protocol",
        PublicationError::VersionIncompatible => "version incompatible",
        PublicationError::Unauthorized => "unauthorized",
        PublicationError::VerificationPending => "verification pending",
        PublicationError::QueueOverflow => "queue overflow",
        PublicationError::MessageTooLarge => "message too large",
        PublicationError::NotConnected => "not connected",
        PublicationError::InvalidCommand(_) => "command rejected",
        PublicationError::Server(_) => "server",
        PublicationError::Fixture => "fixture",
    }
}
