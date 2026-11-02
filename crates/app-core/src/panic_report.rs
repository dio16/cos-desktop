//! Best-effort crash report on an ordinary Rust panic.
//!
//! The hook is installed once at startup. It writes only the closed
//! [`CrashReportInput`] fields (version, closed category, optional diagnostics);
//! the panic payload, backtrace and any error text are never read or stored.
//! I/O is best effort and a failure inside the hook never starts another panic.
//! OS-forced termination, `FailFast` and WER exceptions are outside this path.

use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::crash_report::{CrashCategory, CrashReportInput, CrashReportStore};

static INSTALLED: AtomicBool = AtomicBool::new(false);
static IN_HOOK: AtomicBool = AtomicBool::new(false);

/// Install the panic hook once; later calls are no-ops.
///
/// The previously installed hook is dropped without being called: the default
/// hook prints the panic payload and backtrace, which must not leave the process.
pub fn install(desktop_version: &'static str) {
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    let previous = panic::take_hook();
    drop(previous);
    panic::set_hook(Box::new(move |_info| {
        // Re-entrancy: a panic raised while saving is swallowed, never recursive.
        if IN_HOOK.swap(true, Ordering::SeqCst) {
            return;
        }
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
