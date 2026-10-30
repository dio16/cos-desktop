//! Observable guarantees of the bounded, redacted support report.
use std::collections::BTreeMap;

use app_core::diagnostics::{DiagnosticBatch, DiagnosticEvent, DiagnosticKind, EventGap};
use app_core::observation::{
    ObservationPublication, PublicationConnection, PublicationError, PublicationSource,
};
use app_core::support_report::{MAX_REPORT_LINES, MAX_SUPPORT_REPORT_BYTES, SupportReport};
use app_core::{MAX_DIAGNOSTIC_LINE_BYTES, ReportValue};
use desktop_protocol::ids::Counter;

fn observing(revision: Option<&str>) -> ObservationPublication {
    ObservationPublication {
        snapshot: None,
        revision: revision.map(|value| Counter::new(value).expect("a decimal counter")),
        connection: PublicationConnection::Observing,
        source: PublicationSource::Runtime,
        stale: false,
        last_error: None,
        command_statuses: BTreeMap::new(),
        session_generation: 7,
        publication_version: 11,
    }
}

fn batch(events: Vec<DiagnosticEvent>, gap: Option<EventGap>) -> DiagnosticBatch {
    DiagnosticBatch {
        latest_sequence: events.last().map_or(0, |event| event.sequence),
        events,
        event_gap: gap,
        exhausted: false,
    }
}

fn event(sequence: u64, kind: DiagnosticKind) -> DiagnosticEvent {
    DiagnosticEvent { sequence, kind }
}

#[test]
fn versions_are_never_filled_from_the_desktop_package() {
    let report = SupportReport::from_publication(&observing(Some("42")), &batch(Vec::new(), None));
    let rendered = report.render();
    assert_eq!(
        report.runtime_version,
        ReportValue::Unknown("Runtime version is not exposed through observation"),
        "an unobserved Runtime version is never invented"
    );
    assert_eq!(
        report.protocol_version,
        ReportValue::Unknown("no protocol version pair was negotiated")
    );
    let runtime_line = rendered
        .lines()
        .find(|line| line.starts_with("Runtime version: "))
        .expect("the report states the Runtime version field");
    assert!(runtime_line.contains("unknown"), "{runtime_line}");
    assert!(
        !runtime_line.contains(env!("CARGO_PKG_VERSION")),
        "the Desktop version must not be presented as the Runtime version"
    );
}

#[test]
fn a_disconnected_publication_reports_not_connected_and_no_revision() {
    let mut publication = observing(None);
    publication.connection = PublicationConnection::Disconnected;
    publication.source = PublicationSource::Unavailable;
    let report = SupportReport::from_publication(&publication, &batch(Vec::new(), None));
    let rendered = report.render();
    assert!(rendered.contains("Connection: not connected"), "{rendered}");
    assert!(
        rendered.contains("Projection revision: unknown (no projection commit was published)"),
        "{rendered}"
    );
    assert!(!rendered.contains("Projection revision: 0"), "{rendered}");
}

#[test]
fn diagnostic_summary_preserves_the_gap_counts_and_exhaustion() {
    let batch = DiagnosticBatch {
        events: vec![
            event(9, DiagnosticKind::Activity),
            event(
                10,
                DiagnosticKind::Connection(PublicationConnection::Observing),
            ),
            event(11, DiagnosticKind::Error(PublicationError::Transport)),
            event(12, DiagnosticKind::Resync),
        ],
        event_gap: Some(EventGap { from: 3, to: 8 }),
        latest_sequence: 12,
        exhausted: false,
    };
    let summary = app_core::DiagnosticsSummary::from_batch(&batch);
    assert_eq!(summary.retained_events, 4);
    assert_eq!(summary.oldest_sequence, Some(9));
    assert_eq!(summary.event_gap, Some(EventGap { from: 3, to: 8 }));
    assert_eq!(summary.counts.activity, 1);
    assert_eq!(summary.counts.connection, 1);
    assert_eq!(summary.counts.error, 1);
    assert_eq!(summary.counts.resync, 1);
    let rendered = SupportReport::from_publication(&observing(None), &batch).render();
    assert!(rendered.contains("Event gap: 3-8"), "{rendered}");
}

#[test]
fn render_stays_inside_the_line_and_report_bounds() {
    let mut publication = observing(Some("42"));
    publication.last_error = Some(PublicationError::QueueOverflow);
    let mut report = SupportReport::from_publication(&publication, &batch(Vec::new(), None));
    report.runtime_version = ReportValue::Available("x".repeat(8_000));
    let rendered = report.render();
    assert!(
        rendered.len() <= MAX_SUPPORT_REPORT_BYTES,
        "the report exceeded its byte bound: {}",
        rendered.len()
    );
    assert!(rendered.lines().count() <= MAX_REPORT_LINES);
    for line in rendered.lines() {
        assert!(
            line.len() <= MAX_DIAGNOSTIC_LINE_BYTES,
            "a line exceeded its byte bound: {} bytes",
            line.len()
        );
    }
    // Per-line caps keep the fixed field set inside the bound without dropping a
    // field, so the guarantee is structural rather than a truncation notice.
    assert!(
        rendered.lines().count() <= MAX_REPORT_LINES,
        "the report must stay within its line budget"
    );
}

#[test]
fn synthetic_credentials_locators_and_private_paths_never_render() {
    let publication = observing(Some("42"));
    let mut report = SupportReport::from_publication(&publication, &batch(Vec::new(), None));
    report.runtime_version = ReportValue::Available("access_token=super-secret-value".to_owned());
    report.protocol_version =
        ReportValue::Available("https://objects.example/a?X-Amz-Signature=deadbeefcafe".to_owned());
    report.projection_revision =
        ReportValue::Available(r"C:\Users\operator\private\revision".to_owned());
    let rendered = report.render();
    for secret in ["super-secret-value", "deadbeefcafe", r"C:\Users\operator"] {
        assert!(
            !rendered.contains(secret),
            "the rendered report exposed {secret}:\n{rendered}"
        );
    }
}

#[test]
fn an_unavailable_desktop_explains_itself_and_states_no_measurement() {
    let report = SupportReport::unavailable("Disconnected — Runtime not configured");
    let rendered = report.render();
    assert!(rendered.contains("Unavailable: Disconnected — Runtime not configured"));
    assert!(rendered.contains("Retained events: 0"));
    assert!(rendered.contains("Latest sequence: 0"));
    assert!(
        rendered.contains("Runtime version: unknown (no observation session exists)"),
        "{rendered}"
    );
}
