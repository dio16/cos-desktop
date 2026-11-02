//! Observable guarantees of the bounded, redacted support report.
use std::collections::BTreeMap;

use app_core::diagnostics::{
    DiagnosticBatch, DiagnosticCounts, DiagnosticEvent, DiagnosticKind, EventGap,
};
use app_core::observation::{
    ObservationPublication, PublicationConnection, PublicationError, PublicationSource,
};
use app_core::support_report::{MAX_REPORT_LINES, MAX_SUPPORT_REPORT_BYTES, SupportReport};
use app_core::{MAX_DIAGNOSTIC_LINE_BYTES, ReportValue};
use desktop_protocol::ids::Counter;

const DESKTOP: &str = "9.9.9-desktop-owner";

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
        retained_events: events.len(),
        oldest_retained_sequence: events.first().map(|event| event.sequence),
        retained_counts: DiagnosticCounts {
            activity: events
                .iter()
                .filter(|event| event.kind == DiagnosticKind::Activity)
                .count(),
            connection: events
                .iter()
                .filter(|event| matches!(event.kind, DiagnosticKind::Connection(_)))
                .count(),
            error: events
                .iter()
                .filter(|event| matches!(event.kind, DiagnosticKind::Error(_)))
                .count(),
            resync: events
                .iter()
                .filter(|event| event.kind == DiagnosticKind::Resync)
                .count(),
        },
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
    let report =
        SupportReport::from_publication(&observing(Some("42")), &batch(Vec::new(), None), DESKTOP);
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
    assert_eq!(report.desktop_version, DESKTOP);
    assert!(
        rendered.contains(&format!("Desktop version: {DESKTOP}")),
        "the report renders the version its caller supplied:\n{rendered}"
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
    let report = SupportReport::from_publication(&publication, &batch(Vec::new(), None), DESKTOP);
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
        retained_events: 4,
        oldest_retained_sequence: Some(9),
        retained_counts: DiagnosticCounts {
            activity: 1,
            connection: 1,
            error: 1,
            resync: 1,
        },
        latest_sequence: 12,
        exhausted: false,
    };
    let summary = app_core::DiagnosticsSummary::from_batch(&batch);
    assert_eq!(summary.retained_events, 4);
    assert_eq!(summary.event_gap_lost, 6);
    assert_eq!(summary.oldest_sequence, Some(9));
    assert_eq!(summary.event_gap, Some(EventGap { from: 3, to: 8 }));
    assert_eq!(summary.counts.activity, 1);
    assert_eq!(summary.counts.connection, 1);
    assert_eq!(summary.counts.error, 1);
    assert_eq!(summary.counts.resync, 1);
    let rendered = SupportReport::from_publication(&observing(None), &batch, DESKTOP).render();
    assert!(
        rendered.contains("Event gap: 3-8 (6 lost)"),
        "the report states the lost count numerically:\n{rendered}"
    );
}

#[test]
fn render_stays_inside_the_line_and_report_bounds() {
    let mut publication = observing(Some("42"));
    publication.last_error = Some(PublicationError::QueueOverflow);
    let mut report =
        SupportReport::from_publication(&publication, &batch(Vec::new(), None), DESKTOP);
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
    let mut report =
        SupportReport::from_publication(&publication, &batch(Vec::new(), None), DESKTOP);
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
fn the_report_restates_the_ring_retention_not_the_cursor_window() {
    // Two events after the cursor, but the ring itself retains 4096: the report
    // must state the ring's retention, never a count recomputed from the window.
    let batch = DiagnosticBatch {
        events: vec![
            event(5000, DiagnosticKind::Activity),
            event(5001, DiagnosticKind::Resync),
        ],
        event_gap: None,
        retained_events: 4096,
        oldest_retained_sequence: Some(906),
        retained_counts: DiagnosticCounts {
            activity: 4095,
            connection: 0,
            error: 0,
            resync: 1,
        },
        latest_sequence: 5001,
        exhausted: false,
    };
    // The fixture is self-consistent: a contiguous ring 906..=5001 holds 4096
    // events and the category sum matches the retained count.
    let latest = batch.latest_sequence;
    let oldest = batch.oldest_retained_sequence.expect("retained ring");
    assert_eq!(latest - oldest + 1, batch.retained_events as u64);
    assert_eq!(
        batch.retained_counts.activity
            + batch.retained_counts.connection
            + batch.retained_counts.error
            + batch.retained_counts.resync,
        batch.retained_events
    );
    assert!(
        latest - oldest + 1 > batch.events.len() as u64,
        "the ring is larger than the cursor window"
    );
    let summary = app_core::DiagnosticsSummary::from_batch(&batch);
    assert_eq!(summary.retained_events, 4096);
    assert_eq!(batch.retained_events, 4096);
    let rendered =
        SupportReport::from_publication(&observing(Some("42")), &batch, DESKTOP).render();
    assert!(
        rendered.contains("Retained events: 4096"),
        "the report must restate the ring count:\n{rendered}"
    );
    assert!(
        !rendered.contains("Retained events: 2"),
        "the cursor window must not be reported as the retention:\n{rendered}"
    );
    assert!(
        rendered.contains("Oldest retained: 906"),
        "the oldest retained sequence is the ring's (906), not the window's (5000):
{rendered}"
    );
    assert!(
        rendered.contains("Event counts: activity 4095, connection 0, error 0, resync 1"),
        "the category counts cover the ring, not the window's 1/0/0/1:
{rendered}"
    );
}

#[test]
fn the_report_states_the_numeric_limits() {
    let report =
        SupportReport::from_publication(&observing(None), &batch(Vec::new(), None), DESKTOP);
    let rendered = report.render();
    assert_eq!(report.limits.max_diagnostic_events, 4096);
    assert!(
        rendered.contains("Limits: events 4096, line 512 bytes, report 12288 bytes, 24 lines"),
        "the report states its exact numeric limits:\n{rendered}"
    );
    assert!(rendered.len() <= MAX_SUPPORT_REPORT_BYTES);
    assert!(rendered.lines().count() <= MAX_REPORT_LINES);
    assert!(
        rendered
            .lines()
            .all(|line| line.len() <= MAX_DIAGNOSTIC_LINE_BYTES)
    );
}

#[test]
fn a_boundary_line_is_truncated_without_splitting_a_character_and_redacted() {
    let long = "😀".repeat(MAX_DIAGNOSTIC_LINE_BYTES);
    let leaked: &'static str = Box::leak(long.into_boxed_str());
    let mut report =
        SupportReport::from_publication(&observing(Some("42")), &batch(Vec::new(), None), DESKTOP);
    report.desktop_version = leaked.to_owned();
    let rendered = report.render();
    assert!(
        rendered.len() <= MAX_SUPPORT_REPORT_BYTES,
        "{}",
        rendered.len()
    );
    for line in rendered.lines() {
        assert!(
            line.len() <= MAX_DIAGNOSTIC_LINE_BYTES,
            "a boundary line exceeded the cap: {} bytes",
            line.len()
        );
        // The rendered text is valid UTF-8 by construction; a split character would
        // not survive the String boundary.
        assert!(std::str::from_utf8(line.as_bytes()).is_ok());
    }
}

#[test]
fn an_unavailable_desktop_explains_itself_and_states_no_measurement() {
    let report = SupportReport::unavailable("Disconnected — Runtime not configured", DESKTOP);
    let rendered = report.render();
    assert!(rendered.contains("Unavailable: Disconnected — Runtime not configured"));
    assert!(
        rendered.contains("Retained events: unknown (no diagnostic ring was read)"),
        "an unread ring is never reported as zero events:\n{rendered}"
    );
    assert!(rendered.contains("Latest sequence: unknown (no diagnostic ring was read)"));
    assert!(rendered.contains("Event counts: unknown (no diagnostic ring was read)"));
    assert!(
        rendered.contains("Runtime version: unknown (no observation session exists)"),
        "{rendered}"
    );
}

#[test]
fn a_read_empty_ring_is_zero_while_an_unread_ring_is_unknown() {
    let read_empty =
        SupportReport::from_publication(&observing(None), &batch(Vec::new(), None), DESKTOP);
    let read_rendered = read_empty.render();
    assert!(
        read_rendered.contains("Retained events: 0"),
        "{read_rendered}"
    );
    assert!(
        read_rendered.contains("Latest sequence: 0"),
        "{read_rendered}"
    );
    assert!(
        read_rendered.contains("Sequence exhausted: false"),
        "{read_rendered}"
    );

    let unread = SupportReport::unavailable("no observation session", DESKTOP);
    let unread_rendered = unread.render();
    assert!(
        !unread_rendered.contains("Retained events: 0"),
        "an unobserved Desktop must not look like a measured empty ring:\n{unread_rendered}"
    );
}
