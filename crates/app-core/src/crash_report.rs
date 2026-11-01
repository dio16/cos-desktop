//! Bounded, redacted crash-report storage (ADR-0017 DD-22).
//!
//! The store owns one dedicated subdirectory under the existing application data
//! directory (`preferences::default_path()`), keeps at most
//! [`MAX_CRASH_REPORTS`] managed reports of at most [`MAX_CRASH_REPORT_BYTES`]
//! each, rotates the oldest managed report before a fourth write, and never
//! touches files it did not create. A report is written to a staging file inside
//! the same directory and only then renamed to its final name, so a failed or
//! interrupted write never appears as a listed report.
//!
//! Report content is built from closed fields only: the caller-supplied desktop
//! version, a closed [`CrashCategory`] and the ring's safe diagnostic summary.
//! There is no free-text field, so a panic payload or remote text has no way into
//! a stored report. Every rendered line is still passed through the existing
//! diagnostic redactor, and the result is re-redacted before commit: if the text
//! is not stable under redaction it is refused instead of written.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::diagnostics::{DiagnosticBatch, DiagnosticCounts};
use crate::preferences;
use crate::redaction::{MAX_DIAGNOSTIC_LINE_BYTES, redact_diagnostic_text_within};

/// Maximum number of managed crash reports retained.
pub const MAX_CRASH_REPORTS: usize = 3;

/// Maximum bytes of one stored crash report.
pub const MAX_CRASH_REPORT_BYTES: usize = 5 * 1024 * 1024;

/// Dedicated subdirectory of the application data directory.
pub const CRASH_REPORT_DIR: &str = "crash-reports";

/// Managed file name prefix; only this exact shape is inspected or evicted.
pub const REPORT_PREFIX: &str = "report-";

/// Managed file name extension.
pub const REPORT_EXTENSION: &str = "txt";

/// Staging prefix; never listed as a report.
const STAGING_PREFIX: &str = ".staged-";

/// Closed crash category. No caller-supplied text is accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrashCategory {
    /// Failure while the application was starting.
    Startup,
    /// Failure in the observation/publication path.
    Observation,
    /// Failure in media presentation.
    Media,
    /// Failure in rendering or layout.
    Render,
    /// Failure during shutdown.
    Shutdown,
    /// A failure the product cannot classify further.
    Unknown,
}

impl CrashCategory {
    /// Stable token written into the report.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::Observation => "observation",
            Self::Media => "media",
            Self::Render => "render",
            Self::Shutdown => "shutdown",
            Self::Unknown => "unknown",
        }
    }
}

/// Bounded input for one report. Every field is closed or redacted at render time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrashReportInput<'a> {
    /// Desktop package version supplied by the owning binary.
    pub desktop_version: &'a str,
    /// Closed failure category.
    pub category: CrashCategory,
    /// Safe diagnostic summary, when the ring was read.
    pub diagnostics: Option<&'a DiagnosticBatch>,
}

/// One managed report on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrashReportMeta {
    /// Sequence number parsed from the managed file name.
    pub id: u64,
    /// Size in bytes.
    pub bytes: u64,
}

/// Closed storage failures.
#[derive(Debug)]
pub enum CrashStoreError {
    /// The application data directory cannot be derived or created.
    Unavailable,
    /// The destination cannot be used as a dedicated directory.
    NotADirectory,
    /// The report is larger than [`MAX_CRASH_REPORT_BYTES`].
    TooLarge {
        /// Requested bytes.
        bytes: usize,
        /// The ceiling.
        limit: usize,
    },
    /// Redaction could not be shown to be stable; nothing was written.
    UnsafeContent,
    /// A managed report could not be read back.
    InvalidReport,
    /// Underlying I/O failure.
    Io(std::io::Error),
}

impl std::fmt::Display for CrashStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable => formatter.write_str("no application data directory"),
            Self::NotADirectory => formatter.write_str("the report location is not a directory"),
            Self::TooLarge { bytes, limit } => {
                write!(
                    formatter,
                    "report size {bytes} exceeds the {limit} byte limit"
                )
            }
            Self::UnsafeContent => {
                formatter.write_str("the report cannot be proven free of sensitive content")
            }
            Self::InvalidReport => formatter.write_str("the managed report is invalid"),
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for CrashStoreError {}

impl From<std::io::Error> for CrashStoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// One dedicated crash-report directory.
#[derive(Clone, Debug)]
pub struct CrashReportStore {
    root: PathBuf,
}

impl CrashReportStore {
    /// Derive the store from the existing application data location.
    ///
    /// The parent of `preferences::default_path()` is the only base used; there
    /// is no caller-supplied path and no fallback to another location.
    pub fn open_default() -> Result<Self, CrashStoreError> {
        let preferences = preferences::default_path().ok_or(CrashStoreError::Unavailable)?;
        let root = derived_root(&preferences).ok_or(CrashStoreError::Unavailable)?;
        let store = Self { root };
        store.prepare()?;
        Ok(store)
    }

    /// Open an explicit dedicated directory. Only tests and explicit callers use
    /// this; the directory must not be an existing file.
    pub fn open_at(root: impl Into<PathBuf>) -> Result<Self, CrashStoreError> {
        let store = Self { root: root.into() };
        store.prepare()?;
        Ok(store)
    }

    /// The dedicated directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn prepare(&self) -> Result<(), CrashStoreError> {
        if self.root.exists() && !self.root.is_dir() {
            return Err(CrashStoreError::NotADirectory);
        }
        fs::create_dir_all(&self.root)?;
        self.prune()?;
        Ok(())
    }

    /// Managed reports, oldest first. Unknown entries are never returned.
    pub fn list(&self) -> Result<Vec<CrashReportMeta>, CrashStoreError> {
        let mut reports = self.managed_reports()?;
        reports.sort_by_key(|report| report.id);
        Ok(reports)
    }

    /// Read one managed report by sequence.
    pub fn read(&self, id: u64) -> Result<String, CrashStoreError> {
        let path = self.root.join(managed_name(id));
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(CrashStoreError::InvalidReport);
            }
            Err(error) => return Err(CrashStoreError::Io(error)),
        };
        if !meta.file_type().is_file() || meta.len() > MAX_CRASH_REPORT_BYTES as u64 {
            return Err(CrashStoreError::InvalidReport);
        }
        let bytes = fs::read(&path)?;
        if bytes.len() > MAX_CRASH_REPORT_BYTES {
            return Err(CrashStoreError::InvalidReport);
        }
        String::from_utf8(bytes).map_err(|_| CrashStoreError::InvalidReport)
    }

    /// Render, bound and atomically store one report.
    pub fn save(&self, input: &CrashReportInput<'_>) -> Result<CrashReportMeta, CrashStoreError> {
        self.prepare()?;
        let body = render_report(input)?;
        let bytes = body.as_bytes();
        if bytes.len() > MAX_CRASH_REPORT_BYTES {
            return Err(CrashStoreError::TooLarge {
                bytes: bytes.len(),
                limit: MAX_CRASH_REPORT_BYTES,
            });
        }
        // Rotation happens before the write: exactly the oldest managed report is
        // removed when the retained count has reached the ceiling.
        self.prune()?;
        let mut reports = self.managed_reports()?;
        reports.sort_by_key(|report| report.id);
        if reports.len() >= MAX_CRASH_REPORTS {
            let excess = reports.len() + 1 - MAX_CRASH_REPORTS;
            for report in reports.iter().take(excess) {
                let path = self.root.join(managed_name(report.id));
                if fs::symlink_metadata(&path)
                    .map(|meta| meta.file_type().is_file())
                    .unwrap_or(false)
                {
                    fs::remove_file(&path)?;
                }
            }
        }
        let id = reports
            .last()
            .map_or(1, |report| report.id.saturating_add(1));
        self.commit(id, bytes)?;
        Ok(CrashReportMeta {
            id,
            bytes: bytes.len() as u64,
        })
    }

    /// Remove managed reports beyond the retention ceiling. Unknown files,
    /// directories and symlinks are never touched.
    pub fn prune(&self) -> Result<(), CrashStoreError> {
        let mut reports = self.managed_reports()?;
        reports.sort_by_key(|report| report.id);
        if reports.len() > MAX_CRASH_REPORTS {
            for report in reports.iter().take(reports.len() - MAX_CRASH_REPORTS) {
                let path = self.root.join(managed_name(report.id));
                if fs::symlink_metadata(&path)
                    .map(|meta| meta.file_type().is_file())
                    .unwrap_or(false)
                {
                    fs::remove_file(&path)?;
                }
            }
        }
        // A managed report larger than the ceiling cannot be a valid report; it is
        // removed as unreadable managed state rather than exposed.
        for report in self.managed_reports()? {
            if report.bytes > MAX_CRASH_REPORT_BYTES as u64 {
                let path = self.root.join(managed_name(report.id));
                if fs::symlink_metadata(&path)
                    .map(|meta| meta.file_type().is_file())
                    .unwrap_or(false)
                {
                    fs::remove_file(&path)?;
                }
            }
        }
        Ok(())
    }

    fn managed_reports(&self) -> Result<Vec<CrashReportMeta>, CrashStoreError> {
        let mut reports = Vec::new();
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Vec::new());
            }
            Err(error) => return Err(CrashStoreError::Io(error)),
        };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let Some(id) = managed_id(name) else {
                continue;
            };
            // Symlinks and directories are not managed reports even when named like
            // one; only a regular file is inspected.
            let meta = match fs::symlink_metadata(entry.path()) {
                Ok(meta) => meta,
                Err(_) => continue,
            };
            if !meta.file_type().is_file() {
                continue;
            }
            reports.push(CrashReportMeta {
                id,
                bytes: meta.len(),
            });
        }
        Ok(reports)
    }

    fn commit(&self, id: u64, bytes: &[u8]) -> Result<(), CrashStoreError> {
        if bytes.len() > MAX_CRASH_REPORT_BYTES {
            return Err(CrashStoreError::TooLarge {
                bytes: bytes.len(),
                limit: MAX_CRASH_REPORT_BYTES,
            });
        }
        let staging = self.root.join(staging_name());
        {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&staging)?;
            file.write_all(bytes)?;
            file.sync_all()?;
        }
        let final_path = self.root.join(managed_name(id));
        let result = fs::rename(&staging, &final_path);
        if result.is_err() {
            let _ = fs::remove_file(&staging);
            return result.map_err(CrashStoreError::Io);
        }
        Ok(())
    }
}

/// The only base used by [`CrashReportStore::open_default`]: the dedicated
/// subdirectory next to the existing preferences file. No other path is accepted.
fn derived_root(preferences_path: &Path) -> Option<PathBuf> {
    preferences_path
        .parent()
        .filter(|base| !base.as_os_str().is_empty())
        .map(|base| base.join(CRASH_REPORT_DIR))
}

fn staging_name() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        "{STAGING_PREFIX}{}-{}-{sequence}.tmp",
        std::process::id(),
        now_stamp()
    )
}

fn now_stamp() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0)
}

fn managed_name(id: u64) -> String {
    format!("{REPORT_PREFIX}{id:010}.{REPORT_EXTENSION}")
}

fn managed_id(name: &str) -> Option<u64> {
    let rest = name.strip_prefix(REPORT_PREFIX)?;
    let digits = rest.strip_suffix(&format!(".{REPORT_EXTENSION}"))?;
    if digits.len() != 10 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Render one bounded report from closed fields.
///
/// Every line is redacted with the existing diagnostic redactor and the whole
/// body is re-redacted: instability under redaction means the content cannot be
/// proven safe and is refused before any write.
fn render_report(input: &CrashReportInput<'_>) -> Result<String, CrashStoreError> {
    // Raw control characters cannot be redacted with certainty (they could forge a
    // line boundary); such an input is refused instead of silently normalized.
    if input
        .desktop_version
        .chars()
        .any(|character| character.is_control())
    {
        return Err(CrashStoreError::UnsafeContent);
    }
    let mut lines: Vec<String> = Vec::with_capacity(12);
    lines.push(format!("Desktop version: {}", input.desktop_version));
    lines.push(format!("Crash category: {}", input.category.as_str()));
    match input.diagnostics {
        Some(batch) => {
            lines.push(format!("Retained events: {}", batch.retained_events));
            lines.push(format!("Latest sequence: {}", batch.latest_sequence));
            lines.push(format!(
                "Oldest retained: {}",
                batch
                    .oldest_retained_sequence
                    .map_or_else(|| "unknown".to_owned(), |value| value.to_string())
            ));
            lines.push(format!(
                "Event gap: {}",
                match batch.event_gap {
                    Some(gap) => format!(
                        "{}-{} ({} lost)",
                        gap.from,
                        gap.to,
                        gap.to.saturating_sub(gap.from) + 1
                    ),
                    None => "none".to_owned(),
                }
            ));
            lines.push(format!("Counts: {}", counts_line(batch.retained_counts)));
        }
        None => {
            lines.push("Retained events: unknown (the diagnostic ring was not read)".to_owned());
        }
    }
    let mut body = String::new();
    for (index, line) in lines.iter().enumerate() {
        let once = redact_diagnostic_text_within(line, MAX_DIAGNOSTIC_LINE_BYTES);
        // Per-line idempotence: a line that changes on a second redaction pass cannot
        // be proven safe (the redactor must already have classified everything in it).
        if redact_diagnostic_text_within(&once, MAX_DIAGNOSTIC_LINE_BYTES) != once {
            return Err(CrashStoreError::UnsafeContent);
        }
        if index > 0 {
            body.push('\n');
        }
        body.push_str(&once);
    }
    Ok(body)
}

fn counts_line(counts: DiagnosticCounts) -> String {
    format!(
        "activity {}, connection {}, error {}, resync {}",
        counts.activity, counts.connection, counts.error, counts.resync
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn temp_root(name: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "cos-crash-unit-{}-{name}-{sequence}",
            std::process::id()
        ))
    }

    #[test]
    fn the_root_is_the_dedicated_directory_beside_the_preferences_file() {
        let derived = derived_root(Path::new("C:/base/cos-desktop/preferences.json"))
            .expect("a parent directory");
        assert_eq!(derived, Path::new("C:/base/cos-desktop/crash-reports"));
        assert_eq!(derived_root(Path::new("preferences.json")), None);
    }

    #[test]
    fn a_report_at_the_byte_limit_is_committed_and_one_byte_over_is_refused() {
        let root = temp_root("bytes");
        let store = CrashReportStore::open_at(&root).expect("the dedicated directory");
        let allowed = vec![b'x'; MAX_CRASH_REPORT_BYTES];
        store.commit(1, &allowed).expect("a report at the limit");
        let reports = store.list().expect("the managed reports");
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].bytes, MAX_CRASH_REPORT_BYTES as u64);

        let over = vec![b'x'; MAX_CRASH_REPORT_BYTES + 1];
        let error = store
            .commit(2, &over)
            .expect_err("one byte over is refused");
        assert!(matches!(
            error,
            CrashStoreError::TooLarge {
                bytes,
                limit
            } if bytes == MAX_CRASH_REPORT_BYTES + 1 && limit == MAX_CRASH_REPORT_BYTES
        ));
        assert_eq!(
            store.list().expect("the managed reports").len(),
            1,
            "a refused report is not published"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_input_that_cannot_be_proven_safe_is_refused_before_writing() {
        let root = temp_root("unsafe");
        let store = CrashReportStore::open_at(&root).expect("the dedicated directory");
        let poisoned = "9.9.9\u{0}payload";
        let error = store
            .save(&CrashReportInput {
                desktop_version: poisoned,
                category: CrashCategory::Startup,
                diagnostics: None,
            })
            .expect_err("control characters are not storable");
        assert!(matches!(error, CrashStoreError::UnsafeContent));
        assert!(store.list().expect("the managed reports").is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn startup_prunes_only_managed_reports_beyond_the_ceiling() {
        let root = temp_root("prune");
        let store = CrashReportStore::open_at(&root).expect("the dedicated directory");
        for id in 1..=5u64 {
            store
                .commit(id, b"managed")
                .expect("a directly committed managed report");
        }
        assert_eq!(store.list().expect("the managed reports").len(), 5);
        let reopened = CrashReportStore::open_at(&root).expect("reopened");
        let reports = reopened.list().expect("the managed reports");
        assert_eq!(reports.len(), MAX_CRASH_REPORTS);
        assert_eq!(
            reports.iter().map(|report| report.id).collect::<Vec<_>>(),
            vec![3, 4, 5],
            "only the oldest managed reports are removed"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
