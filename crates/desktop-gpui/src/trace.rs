//! Debug-only fixture trace with one bounded, redacted product log route.
//!
//! The trace is disabled unless `COS_FIXTURE_TRACE` is set in a debug build.
//! It is correlation and geometry evidence, never authorization or canonical
//! truth. A saturating per-area event quota, a complete-line byte cap and
//! app-core redaction apply to every emitted line.

use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};

use app_core::{MAX_DIAGNOSTIC_LINE_BYTES, redact_diagnostic_text_within};

/// Maximum retained dock trace lines per process.
pub const MAX_DOCK_LINES: usize = 4096;

/// Maximum retained task-rail trace lines per process.
pub const MAX_RAIL_LINES: usize = 8192;

/// Emit one trace line if enabled and admitted by the saturating quota.
///
/// `fields` is evaluated only after admission, so the disabled or exhausted
/// route does no formatting. The complete emitted line, including the
/// `COS_FIXTURE_TRACE` prefix, area, fields and terminating newline, is bounded
/// by [`MAX_DIAGNOSTIC_LINE_BYTES`].
pub fn emit(
    enabled: bool,
    lines: &AtomicUsize,
    limit: usize,
    area: &'static str,
    fields: impl FnOnce() -> String,
) {
    if !enabled || !admit(lines, limit) {
        return;
    }
    let line = bounded_line(area, &fields(), MAX_DIAGNOSTIC_LINE_BYTES - 1);
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "{line}");
}

fn bounded_line(area: &'static str, fields: &str, limit: usize) -> String {
    redact_diagnostic_text_within(&format!("COS_FIXTURE_TRACE {area} {fields}"), limit)
}

/// Saturating admission: at most `limit` successful calls ever increment.
fn admit(lines: &AtomicUsize, limit: usize) -> bool {
    lines
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            (current < limit).then_some(current + 1)
        })
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::{MAX_DIAGNOSTIC_LINE_BYTES, MAX_DOCK_LINES, admit, bounded_line};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn disabled_or_exhausted_quota_does_not_increment_or_format() {
        let lines = AtomicUsize::new(0);
        assert!(!admit(&lines, 0));
        assert_eq!(lines.load(Ordering::Relaxed), 0);
        assert!(admit(&lines, MAX_DOCK_LINES));
        assert_eq!(lines.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn concurrent_admission_never_exceeds_the_quota() {
        let lines = Arc::new(AtomicUsize::new(MAX_DOCK_LINES - 4));
        let threads: Vec<_> = (0..16)
            .map(|_| {
                let lines = Arc::clone(&lines);
                std::thread::spawn(move || admit(&lines, MAX_DOCK_LINES))
            })
            .collect();
        let admitted = threads
            .into_iter()
            .map(|thread| thread.join().expect("admission thread"))
            .filter(|admitted| *admitted)
            .count();
        assert_eq!(admitted, 4);
        assert_eq!(lines.load(Ordering::Relaxed), MAX_DOCK_LINES);
    }

    #[test]
    fn complete_line_including_prefix_is_bounded() {
        let fields = "frame=1 ".repeat(200);
        let line = bounded_line(
            "candidate_dock_state",
            &fields,
            MAX_DIAGNOSTIC_LINE_BYTES - 1,
        );
        assert!(line.starts_with("COS_FIXTURE_TRACE candidate_dock_state "));
        assert!(line.len() < MAX_DIAGNOSTIC_LINE_BYTES);
        assert!(!line.contains('\n'));
    }
}
