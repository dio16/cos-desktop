//! Best-effort crash report on an ordinary Rust panic.
//!
//! The hook is installed once at startup. It writes only the closed
//! [`CrashReportInput`] fields (version, closed category, optional diagnostics);
//! the panic payload, backtrace and any error text are never read or stored.
//! I/O is best effort: the hook body only performs operations that return `Result`, so
//! an ordinary I/O failure cannot start another panic. This is not a defence against a
//! second panic raised while the hook runs - Rust aborts the process in that case - and
//! no such guarantee is made. OS-forced termination, `FailFast` and WER exceptions are
//! outside this path.

use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::crash_report::{CrashCategory, CrashReportInput, CrashReportStore};

static INSTALLED: AtomicBool = AtomicBool::new(false);
static IN_HOOK: AtomicBool = AtomicBool::new(false);

/// Install the panic hook once; later calls are no-ops.
///
/// `set_hook` replaces the installed hook directly, without first restoring the default
/// hook, so there is no window in which the payload-printing hook is active again.
pub fn install(desktop_version: &'static str) {
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    panic::set_hook(Box::new(move |_info| {
        // Re-entrancy: a panic raised while saving is swallowed, never recursive.
        if IN_HOOK.swap(true, Ordering::SeqCst) {
            return;
        }
        // A guard for ordinary unwinding inside the hook body; a panic raised while a
        // panic hook is running aborts the process instead, so this is not a second-panic
        // defence and nothing here depends on it for that.
        let _ = panic::catch_unwind(AssertUnwindSafe(|| {
            let input = CrashReportInput {
                desktop_version,
                category: CrashCategory::Unknown,
                diagnostics: None,
            };
            if let Ok(store) = CrashReportStore::open_default() {
                let _ = store.save(&input);
            }
        }));
        IN_HOOK.store(false, Ordering::SeqCst);
    }));
}
