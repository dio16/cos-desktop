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

/// Highest sequence that still fits the fixed ten-digit managed name.
pub const MAX_REPORT_ID: u64 = 9_999_999_999;

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
    /// The managed-name sequence space is exhausted.
    IdExhausted,
    /// A failed write could not be cleaned up; a staging file may remain.
    StagingCleanup {
        /// The original write/sync/finalize error.
        write: Box<std::io::Error>,
        /// The error from removing the staging file.
        cleanup: Box<std::io::Error>,
    },
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
            Self::IdExhausted => formatter.write_str("the report sequence space is exhausted"),
            Self::StagingCleanup { write, cleanup } => write!(
                formatter,
                "staging cleanup failed ({cleanup}) after a failed write ({write})"
            ),
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
        self.check_root()?;
        fs::create_dir_all(&self.root)?;
        // Re-check after creation: the only accepted root is a real directory, never a
        // link, junction or other reparse point that would redirect writes or eviction.
        self.check_root()?;
        self.prune()?;
        Ok(())
    }

    /// The root must be a real directory; links, junctions and files are refused.
    fn check_root(&self) -> Result<(), CrashStoreError> {
        match fs::symlink_metadata(&self.root) {
            Ok(meta) if is_linked(&meta) || !meta.is_dir() => Err(CrashStoreError::NotADirectory),
            Ok(_) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(CrashStoreError::Io(error)),
        }
    }

    /// Managed reports, oldest first. Unknown entries are never returned.
    pub fn list(&self) -> Result<Vec<CrashReportMeta>, CrashStoreError> {
        self.check_root()?;
        let mut reports = self.managed_reports()?;
        reports.sort_by_key(|report| report.id);
        Ok(reports)
    }

    /// Read one managed report by sequence.
    pub fn read(&self, id: u64) -> Result<String, CrashStoreError> {
        self.check_root()?;
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
        self.prune()?;
        let mut reports = self.managed_reports()?;
        reports.sort_by_key(|report| report.id);
        // The next sequence is validated before any eviction: a save that cannot be
        // written must not destroy an existing report.
        let id = reports
            .last()
            .map_or(1, |report| report.id.saturating_add(1));
        if id > MAX_REPORT_ID {
            return Err(CrashStoreError::IdExhausted);
        }
        // Rotation happens only after the write is known to be feasible.
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
        self.commit(id, bytes)?;
        Ok(CrashReportMeta {
            id,
            bytes: bytes.len() as u64,
        })
    }

    /// Remove managed reports beyond the retention ceiling. Unknown files,
    /// directories and symlinks are never touched.
    pub fn prune(&self) -> Result<(), CrashStoreError> {
        // The root may have been replaced after the store was created; pruning must not
        // follow a link/junction into another directory.
        self.check_root()?;
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
        let staging = self.root.join(staging_name());
        self.commit_via(id, bytes, &staging)
    }

    fn commit_via(&self, id: u64, bytes: &[u8], staging: &Path) -> Result<(), CrashStoreError> {
        if id > MAX_REPORT_ID {
            return Err(CrashStoreError::IdExhausted);
        }
        if bytes.len() > MAX_CRASH_REPORT_BYTES {
            return Err(CrashStoreError::TooLarge {
                bytes: bytes.len(),
                limit: MAX_CRASH_REPORT_BYTES,
            });
        }
        // `create_new` refuses an existing name; in that case this call did not create
        // the file and must never remove it as part of recovery.
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(staging)
        {
            Ok(file) => file,
            Err(error) => return Err(CrashStoreError::Io(error)),
        };
        let outcome = (|| -> std::io::Result<()> {
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);
            fs::rename(staging, self.root.join(managed_name(id)))
        })();
        if let Err(write_error) = outcome {
            // Only a staging file this call created is cleaned up; a cleanup failure is
            // reported instead of being silently ignored.
            match fs::remove_file(staging) {
                Ok(()) => {}
                Err(remove_error) if remove_error.kind() == std::io::ErrorKind::NotFound => {}
                Err(remove_error) => {
                    return Err(CrashStoreError::StagingCleanup {
                        write: Box::new(write_error),
                        cleanup: Box::new(remove_error),
                    });
                }
            }
            return Err(CrashStoreError::Io(write_error));
        }
        Ok(())
    }
}

/// `FILE_ATTRIBUTE_REPARSE_POINT`: a link, junction or other reparse point.
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

/// Pure link/reparse-point predicate, testable without creating a link.
fn is_link_or_reparse(attributes: u32, is_symlink: bool) -> bool {
    is_symlink || attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

/// Whether metadata describes a link or another reparse point (never a real directory).
fn is_linked(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        is_link_or_reparse(meta.file_attributes(), meta.file_type().is_symlink())
    }
    #[cfg(not(windows))]
    {
        is_link_or_reparse(0, meta.file_type().is_symlink())
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

    #[test]
    fn a_linked_store_directory_is_refused() {
        let root = temp_root("linked");
        let target = temp_root("linked-target");
        std::fs::create_dir_all(&target).expect("the target directory");
        #[cfg(windows)]
        let mut created = std::os::windows::fs::symlink_dir(&target, &root).is_ok();
        #[cfg(windows)]
        for _ in 0..10 {
            if created {
                break;
            }
            // A junction is also a reparse point and needs no special privilege.
            created = std::process::Command::new("cmd")
                .args([
                    "/C",
                    "mklink",
                    "/J",
                    &root.to_string_lossy(),
                    &target.to_string_lossy(),
                ])
                .status()
                .map(|status| status.success())
                .unwrap_or(false);
            if !created {
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        }
        #[cfg(not(windows))]
        let created = std::os::unix::fs::symlink(&target, &root).is_ok();
        if !created {
            // The reviewer requires this verification to fail when it cannot run: a
            // skipped link check is not evidence that linked roots are rejected.
            eprintln!("SKIP: link creation is unavailable in this environment");
            let _ = std::fs::remove_dir_all(&target);
            panic!("the linked-root rejection could not be verified: link creation is unavailable");
        }
        let error =
            CrashReportStore::open_at(&root).expect_err("a linked store directory must be refused");
        assert!(matches!(error, CrashStoreError::NotADirectory));
        assert!(
            std::fs::read_dir(&target)
                .expect("the target directory")
                .next()
                .is_none(),
            "nothing is written through the link"
        );
        let _ = std::fs::remove_file(&root);
        let _ = std::fs::remove_dir_all(&target);
    }

    #[test]
    fn a_failed_finalize_removes_the_staging_file() {
        let root = temp_root("staging");
        let store = CrashReportStore::open_at(&root).expect("the dedicated directory");
        // A directory at the final managed name makes the finalize rename fail.
        let blocker = root.join(managed_name(2));
        std::fs::create_dir(&blocker).expect("the blocking directory");
        let error = store
            .commit(2, b"staged body")
            .expect_err("finalizing over a directory fails");
        assert!(matches!(error, CrashStoreError::Io(_)));
        let staged: Vec<String> = std::fs::read_dir(&root)
            .expect("the report directory")
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.starts_with(STAGING_PREFIX))
            .collect();
        assert!(
            staged.is_empty(),
            "a failed write leaves no staging file: {staged:?}"
        );
        assert!(blocker.is_dir(), "the blocking directory is untouched");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_sequence_stops_at_the_ten_digit_name_limit() {
        let root = temp_root("id-limit");
        let store = CrashReportStore::open_at(&root).expect("the dedicated directory");
        store
            .commit(MAX_REPORT_ID, b"last valid report")
            .expect("the last ten-digit name");
        assert_eq!(store.list().expect("the managed reports").len(), 1);
        let error = store
            .save(&CrashReportInput {
                desktop_version: "9.9.9",
                category: CrashCategory::Unknown,
                diagnostics: None,
            })
            .expect_err("the next id would not fit the managed name");
        assert!(matches!(error, CrashStoreError::IdExhausted));
        assert_eq!(
            store.list().expect("the managed reports").len(),
            1,
            "no unmanaged eleventh-digit name is written"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_link_predicate_covers_reparse_points_without_creating_one() {
        // Ordinary directory attributes: FILE_ATTRIBUTE_DIRECTORY (0x10).
        assert!(!is_link_or_reparse(0x10, false));
        // Reparse-point attribute, as reported for a junction.
        assert!(is_link_or_reparse(FILE_ATTRIBUTE_REPARSE_POINT, false));
        // A symlink is recognised even if the attribute word is unavailable.
        assert!(is_link_or_reparse(0, true));
        // Combined flags remain linked.
        assert!(is_link_or_reparse(
            0x10 | FILE_ATTRIBUTE_REPARSE_POINT,
            false
        ));
    }

    #[test]
    fn prune_refuses_a_store_directory_replaced_by_a_link() {
        let root = temp_root("prune-link");
        let target = temp_root("prune-link-target");
        std::fs::create_dir_all(&target).expect("the target directory");
        let target_report = target.join(managed_name(1));
        std::fs::write(&target_report, b"must survive").expect("a target-side report");
        let store = CrashReportStore::open_at(&root).expect("the dedicated directory");
        std::fs::remove_dir_all(&root).expect("the real directory is removed");
        #[cfg(windows)]
        let created = std::os::windows::fs::symlink_dir(&target, &root).is_ok()
            || std::process::Command::new("cmd")
                .args([
                    "/C",
                    "mklink",
                    "/J",
                    &root.to_string_lossy(),
                    &target.to_string_lossy(),
                ])
                .status()
                .map(|status| status.success())
                .unwrap_or(false);
        #[cfg(not(windows))]
        let created = std::os::unix::fs::symlink(&target, &root).is_ok();
        if !created {
            eprintln!("SKIP: link creation is unavailable in this environment");
            let _ = std::fs::remove_dir_all(&target);
            panic!("the prune rejection could not be verified: link creation is unavailable");
        }
        let error = store
            .prune()
            .expect_err("pruning through a linked root is refused");
        assert!(matches!(error, CrashStoreError::NotADirectory));
        assert!(
            target_report.exists(),
            "a target-side managed report is never deleted through the link"
        );
        let _ = std::fs::remove_dir(&root);
        let _ = std::fs::remove_dir_all(&target);
    }

    #[test]
    fn id_exhaustion_keeps_all_retained_reports() {
        let root = temp_root("id-keeps");
        let store = CrashReportStore::open_at(&root).expect("the dedicated directory");
        for id in (MAX_REPORT_ID - 2)..=MAX_REPORT_ID {
            store.commit(id, b"retained").expect("a retained report");
        }
        let before = store.list().expect("the managed reports");
        assert_eq!(before.len(), MAX_CRASH_REPORTS);
        let error = store
            .save(&CrashReportInput {
                desktop_version: "9.9.9",
                category: CrashCategory::Unknown,
                diagnostics: None,
            })
            .expect_err("the sequence is exhausted");
        assert!(matches!(error, CrashStoreError::IdExhausted));
        let after = store.list().expect("the managed reports");
        assert_eq!(
            after, before,
            "a refused save does not evict an existing report"
        );
        for report in &after {
            assert!(
                root.join(managed_name(report.id)).exists(),
                "every retained report is still on disk"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_staging_collision_keeps_the_existing_file() {
        let root = temp_root("staging-collision");
        let store = CrashReportStore::open_at(&root).expect("the dedicated directory");
        let staging = root.join(".staged-collision.tmp");
        std::fs::write(&staging, b"existing staging content").expect("an existing file");
        let error = store
            .commit_via(1, b"new body", &staging)
            .expect_err("create_new refuses the collision");
        assert!(matches!(error, CrashStoreError::Io(_)));
        assert_eq!(
            std::fs::read(&staging).expect("the existing staging file"),
            b"existing staging content",
            "a collision never deletes a file this call did not create"
        );
        assert!(
            store.list().expect("the managed reports").is_empty(),
            "the collision does not publish a report"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
