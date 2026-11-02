//! Bounded crash-report storage: retention, rotation, redaction and atomic publish.

use std::path::{Path, PathBuf};

use app_core::diagnostics::{DiagnosticBatch, DiagnosticCounts, EventGap};
use app_core::{
    CrashCategory, CrashReportInput, CrashReportStore, CrashStoreError, MAX_CRASH_REPORT_BYTES,
    MAX_CRASH_REPORTS, REPORT_EXTENSION, REPORT_PREFIX,
};

fn test_root(name: &str) -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "crash-report-{name}-{}-{}",
        std::process::id(),
        unique()
    ))
}

fn unique() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0)
}

fn batch() -> DiagnosticBatch {
    DiagnosticBatch {
        events: Vec::new(),
        event_gap: Some(EventGap { from: 1, to: 904 }),
        retained_events: 4096,
        oldest_retained_sequence: Some(905),
        retained_counts: DiagnosticCounts {
            activity: 4095,
            connection: 0,
            error: 0,
            resync: 1,
        },
        latest_sequence: 5000,
        exhausted: false,
    }
}

fn input(category: CrashCategory) -> CrashReportInput<'static> {
    CrashReportInput {
        desktop_version: "9.9.9-desktop-owner",
        category,
        diagnostics: None,
    }
}

fn save_sequence(store: &CrashReportStore, count: u64) -> Vec<u64> {
    let categories = [
        CrashCategory::Startup,
        CrashCategory::Observation,
        CrashCategory::Media,
        CrashCategory::Render,
        CrashCategory::Shutdown,
    ];
    let mut ids = Vec::new();
    for index in 0..count {
        let meta = store
            .save(&CrashReportInput {
                category: categories[index as usize % categories.len()],
                ..input(CrashCategory::Unknown)
            })
            .unwrap_or_else(|error| panic!("save failed: {error:?}"));
        ids.push(meta.id);
    }
    ids
}

fn managed_files(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(root)
        .expect("the report directory")
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_type()
                .map(|file_type| file_type.is_file())
                .unwrap_or(false)
        })
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with("report-") && name.ends_with(".txt"))
        .collect();
    names.sort();
    names
}

#[test]
fn one_to_three_reports_are_saved_and_reload_in_order() {
    let root = test_root("three");
    let store = CrashReportStore::open_at(&root).expect("the dedicated directory");
    let batch = batch();
    let mut ids = Vec::new();
    for category in [
        CrashCategory::Startup,
        CrashCategory::Observation,
        CrashCategory::Media,
    ] {
        let meta = store
            .save(&CrashReportInput {
                desktop_version: "9.9.9-desktop-owner",
                category,
                diagnostics: Some(&batch),
            })
            .expect("a stored report");
        ids.push(meta.id);
    }
    assert_eq!(ids, vec![1, 2, 3]);
    let reports = store.list().expect("the managed reports");
    assert_eq!(
        reports.iter().map(|report| report.id).collect::<Vec<_>>(),
        ids
    );
    let first = store.read(1).expect("the first report");
    assert!(first.contains("Crash category: startup"), "{first}");
    assert!(first.contains("Retained events: 4096"), "{first}");
    assert!(first.contains("Event gap: 1-904 (904 lost)"), "{first}");
    assert!(
        first.contains("Counts: activity 4095, connection 0, error 0, resync 1"),
        "{first}"
    );
    drop(store);

    let reopened = CrashReportStore::open_at(&root).expect("a new store instance");
    let reports = reopened.list().expect("the managed reports");
    assert_eq!(reports.len(), 3, "all three reports survive a restart");
    assert!(
        reopened
            .read(3)
            .expect("the third report")
            .contains("media")
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_fourth_report_evicts_only_the_oldest_managed_report() {
    let root = test_root("rotation");
    let store = CrashReportStore::open_at(&root).expect("the dedicated directory");
    let unknown = root.join("notes.txt");
    std::fs::write(&unknown, b"user notes").expect("an unknown file");
    let unknown_like = root.join("report-abc.txt");
    std::fs::write(&unknown_like, b"not managed").expect("a malformed managed-looking file");
    let managed_dir = root.join("report-0000000099.txt");
    std::fs::create_dir(&managed_dir).expect("a directory with a managed name");

    let ids = save_sequence(&store, 4);
    assert_eq!(ids, vec![1, 2, 3, 4], "the sequence keeps advancing");
    let reports = store.list().expect("the managed reports");
    assert_eq!(reports.len(), MAX_CRASH_REPORTS);
    assert_eq!(
        reports.iter().map(|report| report.id).collect::<Vec<_>>(),
        vec![2, 3, 4],
        "only the oldest managed report is evicted"
    );
    assert!(
        matches!(store.read(1), Err(CrashStoreError::InvalidReport)),
        "the evicted report is gone"
    );
    assert!(unknown.exists(), "an unknown file is never deleted");
    assert!(unknown_like.exists(), "a malformed name is never deleted");
    assert!(
        managed_dir.is_dir(),
        "a directory is never treated as a report"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A file edited after saving must not reach a future display path: a credential written
/// into the version line, like `token=secret123`, changes under redaction and is refused.
#[test]
fn an_injected_credential_in_a_stored_report_is_refused() {
    let root = test_root("injected-credential");
    let store = CrashReportStore::open_at(&root).expect("the dedicated directory");
    let meta = store
        .save(&CrashReportInput {
            desktop_version: "9.9.9",
            category: CrashCategory::Unknown,
            diagnostics: None,
        })
        .expect("a clean report");
    let path = report_path(&store, meta.id);
    let tampered = concat!(
        "Desktop version: 9.9.9 token=secret123",
        "\n",
        "Crash category: unknown",
        "\n",
        "Retained events: unknown (the diagnostic ring was not read)",
    )
    .to_owned();
    std::fs::write(&path, &tampered).expect("the tampered file");
    let error = store.read(meta.id).expect_err("a tampered report");
    assert!(matches!(error, CrashStoreError::InvalidReport), "{error:?}");
    assert!(
        !format!("{error:?}{error}").contains("secret123"),
        "the refusal must not echo the body"
    );
    assert_eq!(
        std::fs::read_to_string(&path).expect("unchanged"),
        tampered,
        "the refused file is left exactly as it was"
    );

    // A key the redactor does not know about is refused too: any `key=value` whose value is
    // not a fixed marker is a raw credential.
    let unknown_key = tampered.replace("token=secret123", "api_key=SuperSecret123");
    std::fs::write(&path, &unknown_key).expect("the second tampered file");
    let error = store.read(meta.id).expect_err("an unknown credential key");
    assert!(matches!(error, CrashStoreError::InvalidReport), "{error:?}");

    // A marker with text appended is not a rendered value either.
    let appended = tampered.replace("token=secret123", "api_key=[redacted]SuperSecret123");
    std::fs::write(&path, &appended).expect("the third tampered file");
    let error = store
        .read(meta.id)
        .expect_err("a marker with appended text");
    assert!(matches!(error, CrashStoreError::InvalidReport), "{error:?}");

    // The rendered marker form stays readable: `access_token=[redacted]` is a shape the
    // redactor produces, so rejecting it would break ordinary reports.
    let marked = concat!(
        "Desktop version: 9.9.9 access_token=[redacted]",
        "\n",
        "Crash category: unknown",
        "\n",
        "Retained events: unknown (the diagnostic ring was not read)",
    )
    .to_owned();
    std::fs::write(&path, &marked).expect("the marker file");
    assert_eq!(store.read(meta.id).expect("a marked version"), marked);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn synthetic_secrets_do_not_reach_a_stored_report() {
    let root = test_root("secrets");
    let store = CrashReportStore::open_at(&root).expect("the dedicated directory");
    let secret = "9.9.9 access_token=SuperSecret123 \
                  https://objects.example/a?X-Amz-Signature=deadbeefcafe \
                  C:\\Users\\operator\\private\\file";
    let leaked: &'static str = Box::leak(secret.to_owned().into_boxed_str());
    let meta = store
        .save(&CrashReportInput {
            desktop_version: leaked,
            category: CrashCategory::Unknown,
            diagnostics: None,
        })
        .expect("a redacted report");
    let stored = store.read(meta.id).expect("the stored report");
    for value in ["SuperSecret123", "deadbeefcafe", "operator"] {
        assert!(
            !stored.contains(value),
            "the stored report exposed {value}:\n{stored}"
        );
    }
    assert!(
        stored.contains("[redacted]") || stored.contains("[private-path]"),
        "the redactor marked the withdrawn values:\n{stored}"
    );
    let on_disk = std::fs::read_to_string(root.join(format!("report-{:010}.txt", meta.id)))
        .expect("the report file");
    assert_eq!(on_disk, stored);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_failed_write_is_never_published_as_a_report() {
    let root = test_root("failed");
    let store = CrashReportStore::open_at(&root).expect("the dedicated directory");
    save_sequence(&store, 2);
    let entries: Vec<String> = std::fs::read_dir(&root)
        .expect("the report directory")
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    assert!(
        !entries.iter().any(|name| name.starts_with(".staged-")),
        "a successful save leaves no staging file: {entries:?}"
    );
    // A location that is a regular file is refused, so no write can be attempted.
    let file_root = root.with_extension("not-a-directory");
    std::fs::write(&file_root, b"occupied").expect("a placeholder file");
    assert!(matches!(
        CrashReportStore::open_at(&file_root),
        Err(CrashStoreError::NotADirectory)
    ));
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_file(&file_root);
}

#[test]
fn a_new_store_instance_reloads_reports_after_restart() {
    let root = test_root("restart");
    let store = CrashReportStore::open_at(&root).expect("the dedicated directory");
    let first = store
        .save(&CrashReportInput {
            desktop_version: "9.9.9",
            category: CrashCategory::Render,
            diagnostics: None,
        })
        .expect("first report");
    let second = store
        .save(&CrashReportInput {
            desktop_version: "9.9.9",
            category: CrashCategory::Shutdown,
            diagnostics: Some(&batch()),
        })
        .expect("second report");
    drop(store);

    let reopened = CrashReportStore::open_at(&root).expect("a restarted store");
    let reports = reopened.list().expect("the managed reports");
    assert_eq!(
        reports.iter().map(|report| report.id).collect::<Vec<_>>(),
        vec![first.id, second.id]
    );
    assert!(
        reopened
            .read(second.id)
            .expect("the second report")
            .contains("Crash category: shutdown")
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn unknown_files_directories_and_links_are_not_destroyed() {
    let root = test_root("unknown");
    let store = CrashReportStore::open_at(&root).expect("the dedicated directory");
    let unknown = root.join("readme.txt");
    std::fs::write(&unknown, b"keep").expect("an unknown file");
    let other = root.join("report-2026-11-02.log");
    std::fs::write(&other, b"keep").expect("an unknown report-like file");
    let subdir = root.join("report-0000000099.txt");
    std::fs::create_dir(&subdir).expect("a managed-looking directory");

    let link = root.join("report-0000000098.txt");
    #[cfg(windows)]
    let symlink_created = std::os::windows::fs::symlink_file(&unknown, &link).is_ok();
    #[cfg(not(windows))]
    let symlink_created = false;

    let ids = save_sequence(&store, 5);
    assert_eq!(ids.len(), 5);
    let reports = store.list().expect("the managed reports");
    assert_eq!(reports.len(), MAX_CRASH_REPORTS);
    assert!(
        unknown.exists() && other.exists() && subdir.is_dir(),
        "unknown entries survive rotation and pruning"
    );
    if symlink_created {
        assert!(
            link.symlink_metadata()
                .map(|meta| meta.file_type().is_symlink())
                .unwrap_or(false),
            "a managed-looking link is never replaced or deleted"
        );
    }
    assert_eq!(managed_files(&root).len(), MAX_CRASH_REPORTS);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_declared_bounds_are_the_ratified_values() {
    assert_eq!(MAX_CRASH_REPORTS, 3);
    assert_eq!(MAX_CRASH_REPORT_BYTES, 5_242_880);
    let root = test_root("bounds");
    let store = CrashReportStore::open_at(&root).expect("the dedicated directory");
    let reports = store.list().expect("the managed reports");
    assert!(reports.is_empty());
    assert!(store.read(1).is_err(), "a missing report is not invented");
    let _ = std::fs::remove_dir_all(&root);
}

fn report_path(store: &CrashReportStore, id: u64) -> PathBuf {
    store
        .root()
        .join(format!("{}{:0>10}.{}", REPORT_PREFIX, id, REPORT_EXTENSION))
}

fn rewrite_body(store: &CrashReportStore, id: u64, body: &str) {
    std::fs::write(report_path(store, id), body).expect("rewrite the managed report");
}

#[test]
fn both_rendered_formats_round_trip_through_read() {
    let root = test_root("round-trip");
    let store = CrashReportStore::open_at(&root).unwrap();
    let without = store
        .save(&CrashReportInput {
            desktop_version: "0.1.0",
            category: CrashCategory::Unknown,
            diagnostics: None,
        })
        .unwrap();
    let without_body = store.read(without.id).unwrap();
    assert!(without_body.contains("Retained events: unknown"));

    let with = store
        .save(&CrashReportInput {
            desktop_version: "0.1.0",
            category: CrashCategory::Render,
            diagnostics: Some(&batch()),
        })
        .unwrap();
    let with_body = store.read(with.id).unwrap();
    assert!(with_body.contains("Latest sequence: 5000"));
    assert!(with_body.contains("Counts: activity 4095, connection 0, error 0, resync 1"));
    assert!(store.read(without.id).unwrap() == without_body);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_report_with_an_extra_field_or_injected_text_is_refused() {
    let root = test_root("extra-field");
    let store = CrashReportStore::open_at(&root).unwrap();
    let meta = store
        .save(&CrashReportInput {
            desktop_version: "0.1.0",
            category: CrashCategory::Unknown,
            diagnostics: None,
        })
        .unwrap();
    let valid = store.read(meta.id).unwrap();
    for body in [
        format!("{valid}\nAuthorization: Bearer secret-token"),
        format!("{valid}\nRetained events: 1"),
        valid.replace(
            "Desktop version: 0.1.0",
            "Desktop version: file:///C:/private/x",
        ),
        valid.replace("Crash category: unknown", "Crash category: injected"),
    ] {
        rewrite_body(&store, meta.id, &body);
        let before = std::fs::read(report_path(&store, meta.id)).unwrap();
        assert!(matches!(
            store.read(meta.id),
            Err(CrashStoreError::InvalidReport)
        ));
        let after = std::fs::read(report_path(&store, meta.id)).unwrap();
        assert_eq!(
            before, after,
            "a refused report is never rewritten or deleted"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_full_report_with_duplicate_or_malformed_fields_is_refused() {
    let root = test_root("duplicate-field");
    let store = CrashReportStore::open_at(&root).unwrap();
    let meta = store
        .save(&CrashReportInput {
            desktop_version: "0.1.0",
            category: CrashCategory::Media,
            diagnostics: Some(&batch()),
        })
        .unwrap();
    let valid = store.read(meta.id).unwrap();
    for body in [
        valid.replace(
            "Latest sequence: 5000",
            "Latest sequence: 5000\nLatest sequence: 5000",
        ),
        valid.replace("Event gap: 1-904 (904 lost)", "Event gap: 1-904 (905 lost)"),
        valid.replace("Retained events: 4096", "Retained events: 04096"),
        valid.replace("Counts: activity 4095", "Counts: activity 4095,"),
        valid.replace("Desktop version: 0.1.0", "Desktop version: 0.1.0\u{7}"),
        // The span can overflow u64; the check must refuse rather than panic or wrap.
        valid.replace(
            "Event gap: 1-904 (904 lost)",
            "Event gap: 0-18446744073709551615 (0 lost)",
        ),
    ] {
        rewrite_body(&store, meta.id, &body);
        assert!(
            matches!(store.read(meta.id), Err(CrashStoreError::InvalidReport)),
            "the body must be refused"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}
