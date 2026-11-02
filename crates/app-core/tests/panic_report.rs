//! Ordinary Rust panic capture: one best-effort report, no payload, no recursive save.
//!
//! Each test runs the helper test below in an isolated child process with its own
//! LOCALAPPDATA, so the real Desktop never crashes and the real data directory is
//! untouched.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use app_core::{CrashCategory, CrashReportInput, CrashReportStore, MAX_CRASH_REPORTS};

const HELPER: &str = "panic_report_helper_modes";
const SECRET: &str = "synthetic-secret-XYZ";

fn unique() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0)
}

fn isolated_local(name: &str) -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "panic-report-{name}-{}-{}",
        std::process::id(),
        unique()
    ))
}

fn reports_dir(local: &Path) -> PathBuf {
    local.join("cos-desktop").join("crash-reports")
}

fn report_files(local: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(reports_dir(local))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with("report-") && name.ends_with(".txt"))
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

fn spawn(mode: &str, local: &Path) -> Output {
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", HELPER, "--nocapture", "--quiet"])
        .env("COS_PANIC_HELPER", mode)
        .env("LOCALAPPDATA", local)
        .output()
        .unwrap()
}

/// Child mode selector; the parent never panics in its own process.
#[test]
fn panic_report_helper_modes() {
    let Ok(mode) = std::env::var("COS_PANIC_HELPER") else {
        return;
    };
    match mode.as_str() {
        "panic" => {
            app_core::panic_report::install("9.9.9-test");
            panic!("{SECRET}");
        }
        "double" => {
            app_core::panic_report::install("9.9.9-test");
            app_core::panic_report::install("9.9.9-test");
            panic!("{SECRET}");
        }
        "normal" => {
            app_core::panic_report::install("9.9.9-test");
        }
        _ => {}
    }
}

#[test]
fn a_panic_writes_one_report_without_the_payload() {
    let local = isolated_local("panic");
    let output = spawn("panic", &local);
    assert_eq!(output.status.code(), Some(101), "the child panicked");
    let files = report_files(&local);
    assert_eq!(files.len(), 1, "exactly one report");
    let text = std::fs::read_to_string(&files[0]).unwrap();
    assert!(text.contains("Desktop version: 9.9.9-test"));
    assert!(text.contains("Crash category: unknown"));
    assert!(
        !text.contains(SECRET),
        "the panic payload must never reach the report"
    );
    assert!(!text.to_lowercase().contains("backtrace"));
    // The replaced hook must not print the payload either: the child's stderr would
    // otherwise carry the very secret the report is careful to drop.
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        !stderr.contains(SECRET),
        "the panic payload must never reach stderr"
    );
    assert!(
        !stderr.to_lowercase().contains("note: run with"),
        "the default backtrace hint must not be printed"
    );
}

#[test]
fn a_normal_exit_writes_no_report() {
    let local = isolated_local("normal");
    let output = spawn("normal", &local);
    assert!(output.status.success(), "the helper exits normally");
    assert!(
        report_files(&local).is_empty(),
        "no report for a normal exit"
    );
}

#[test]
fn a_double_install_still_writes_one_report() {
    let local = isolated_local("double");
    let output = spawn("double", &local);
    assert_eq!(output.status.code(), Some(101));
    assert_eq!(report_files(&local).len(), 1, "the hook is installed once");
}

#[test]
fn a_blocked_store_does_not_recurse_or_change_the_exit() {
    let local = isolated_local("blocked");
    let dir = local.join("cos-desktop");
    std::fs::create_dir_all(&dir).unwrap();
    // A plain file where the dedicated directory must be: the existing store
    // refuses it (NotADirectory), and the hook must not panic again.
    std::fs::write(reports_dir(&local), b"not a directory").unwrap();
    let output = spawn("panic", &local);
    assert_eq!(output.status.code(), Some(101), "still the original panic");
    assert!(reports_dir(&local).is_file(), "the store was not replaced");
}

#[test]
fn the_existing_three_report_limit_is_kept() {
    let local = isolated_local("limit");
    std::fs::create_dir_all(&local).unwrap();
    let store = CrashReportStore::open_at(reports_dir(&local)).unwrap();
    for index in 0..MAX_CRASH_REPORTS {
        store
            .save(&CrashReportInput {
                desktop_version: "pre-existing",
                category: CrashCategory::Unknown,
                diagnostics: None,
            })
            .unwrap();
        let _ = index;
    }
    assert_eq!(store.list().unwrap().len(), MAX_CRASH_REPORTS);
    let output = spawn("panic", &local);
    assert_eq!(output.status.code(), Some(101));
    let files = report_files(&local);
    assert_eq!(files.len(), MAX_CRASH_REPORTS, "retention stays at the cap");
    // A fresh process can still read the retained reports.
    let reopened = CrashReportStore::open_at(reports_dir(&local)).unwrap();
    assert!(reopened.list().unwrap().len() <= MAX_CRASH_REPORTS);
    assert!(reopened.read(reopened.list().unwrap()[0].id).is_ok());
}
