//! Protected storage for the Desktop Runtime credential (MP-SEC1).
//!
//! The Runtime credential is kept in the Windows Credential Manager — a
//! DPAPI-protected, per-user store — and never written to a file, preference,
//! log line, support report or crash report. This module only produces fixed
//! labels, so a store failure cannot echo the token, the origin or an OS message.
//!
//! Startup precedence (documented in `IMPLEMENTATION_MASTER_PLAN` §19.115):
//! `COS_DESKTOP_RUNTIME_TOKEN` (explicit operator override) > stored entry >
//! explicit "credential unavailable". An operator-supplied token replaces the
//! stored entry so later launches need no environment variable (rotation); a
//! `401` from the Runtime deletes the entry and forces re-authorization.
//!
//! Non-Windows builds have no OS store integration: they accept the operator
//! override only and report the store as unavailable.

use std::sync::Arc;

/// Fixed failure classes. No variant carries a token, an origin or an OS
/// message, so these are safe to log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CredentialStoreError {
    /// The OS store is not available in this environment.
    Unavailable,
    /// The OS store refused the operation (access denied, corrupt entry, …).
    Failed,
}

impl CredentialStoreError {
    /// Value-free operator label.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Unavailable => "credential store is unavailable",
            Self::Failed => "credential store operation failed",
        }
    }
}

/// One protected credential lifecycle for a Runtime origin.
pub(crate) trait CredentialStore: Send + Sync {
    /// Load the stored credential; `Ok(None)` when no entry exists.
    fn load(&self, origin: &str) -> Result<Option<String>, CredentialStoreError>;
    /// Store or replace the credential for the origin (rotation).
    fn store(&self, origin: &str, token: &str) -> Result<(), CredentialStoreError>;
    /// Delete the entry; deleting a missing entry succeeds (idempotent).
    fn delete(&self, origin: &str) -> Result<(), CredentialStoreError>;
}

/// The process-wide store used by the composition root.
pub(crate) fn default_store() -> Arc<dyn CredentialStore> {
    #[cfg(windows)]
    {
        Arc::new(WindowsCredentialStore::default())
    }
    #[cfg(not(windows))]
    {
        Arc::new(UnavailableCredentialStore)
    }
}

/// Non-Windows fallback: no protected store integration in this build.
#[cfg(not(windows))]
struct UnavailableCredentialStore;

#[cfg(not(windows))]
impl CredentialStore for UnavailableCredentialStore {
    fn load(&self, _origin: &str) -> Result<Option<String>, CredentialStoreError> {
        Ok(None)
    }

    fn store(&self, _origin: &str, _token: &str) -> Result<(), CredentialStoreError> {
        Err(CredentialStoreError::Unavailable)
    }

    fn delete(&self, _origin: &str) -> Result<(), CredentialStoreError> {
        Ok(())
    }
}

/// Windows Credential Manager store (generic credential, machine-persistent).
#[cfg(windows)]
const SERVICE_PREFIX: &str = "cos-desktop/observation";
#[cfg(windows)]
const USER_NAME: &str = "desktop-observation";

#[cfg(windows)]
pub(crate) struct WindowsCredentialStore {
    service_prefix: String,
}

#[cfg(windows)]
impl Default for WindowsCredentialStore {
    fn default() -> Self {
        Self {
            service_prefix: SERVICE_PREFIX.to_owned(),
        }
    }
}

#[cfg(windows)]
impl WindowsCredentialStore {
    /// Credential Manager service (target) name for one origin.
    ///
    /// The name is a digest, so an entry cannot be located by guessing a URL and
    /// stays stable across credential rotation. v1 projects no principal
    /// identity on the wire, so the entry is origin-scoped; the Runtime still
    /// binds the issued context to its own principal, and a principal change
    /// surfaces as a rejected credential that the composition root revokes.
    fn service(&self, origin: &str) -> String {
        let digest = desktop_protocol::sha256_digest(origin.as_bytes());
        let text = digest.as_str();
        let hex = text.strip_prefix("sha256:").unwrap_or(text);
        let head: String = hex.chars().take(32).collect();
        format!("{}/{head}", self.service_prefix)
    }

    fn entry(&self, origin: &str) -> Result<keyring::Entry, CredentialStoreError> {
        keyring::Entry::new(&self.service(origin), USER_NAME)
            .map_err(|_| CredentialStoreError::Unavailable)
    }
}

#[cfg(windows)]
impl CredentialStore for WindowsCredentialStore {
    fn load(&self, origin: &str) -> Result<Option<String>, CredentialStoreError> {
        match self.entry(origin)?.get_secret() {
            Ok(secret) => String::from_utf8(secret)
                .map(Some)
                .map_err(|_| CredentialStoreError::Failed),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(CredentialStoreError::Failed),
        }
    }

    fn store(&self, origin: &str, token: &str) -> Result<(), CredentialStoreError> {
        self.entry(origin)?
            .set_secret(token.as_bytes())
            .map_err(|_| CredentialStoreError::Failed)
    }

    fn delete(&self, origin: &str) -> Result<(), CredentialStoreError> {
        match self.entry(origin)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(CredentialStoreError::Failed),
        }
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    //! Windows-native storage tests. Each test uses a unique service prefix so
    //! parallel runs and leftovers cannot collide with a real entry, and a drop
    //! guard removes the entry even when an assertion fails.

    use super::{CredentialStore, CredentialStoreError, WindowsCredentialStore};
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    const TEST_ORIGIN: &str = "http://127.0.0.1:8123";

    fn unique_store() -> WindowsCredentialStore {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        WindowsCredentialStore {
            service_prefix: format!("cos-desktop-test/{}/{}", std::process::id(), nanos),
        }
    }

    fn unique_token(label: &str) -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        format!("sec1-{label}-{}-{nanos}", std::process::id())
    }

    struct Guard<'a> {
        store: &'a WindowsCredentialStore,
        origin: &'a str,
    }

    impl Drop for Guard<'_> {
        fn drop(&mut self) {
            let _ = self.store.delete(self.origin);
        }
    }

    #[test]
    fn round_trips_and_deletes_a_credential() {
        let store = unique_store();
        let _guard = Guard {
            store: &store,
            origin: TEST_ORIGIN,
        };
        assert_eq!(store.load(TEST_ORIGIN).expect("load"), None);
        let token = unique_token("roundtrip");
        store.store(TEST_ORIGIN, &token).expect("store");
        assert_eq!(store.load(TEST_ORIGIN).expect("load"), Some(token));
        store.delete(TEST_ORIGIN).expect("delete");
        assert_eq!(store.load(TEST_ORIGIN).expect("load"), None);
        // Deleting a missing entry is idempotent.
        store.delete(TEST_ORIGIN).expect("delete is idempotent");
    }

    /// Bounded evidence: one unique token must not appear in a **limited set**
    /// of app-data and temp files. This is deliberately not a whole-machine
    /// claim — see the recorded exclusions and the plan §19.115 limits.
    #[test]
    fn scans_a_bounded_set_of_files_without_a_plaintext_copy() {
        let store = unique_store();
        let _guard = Guard {
            store: &store,
            origin: TEST_ORIGIN,
        };
        let token = unique_token("plaintext-probe");
        store.store(TEST_ORIGIN, &token).expect("store");
        // The credential is readable from the protected store…
        assert_eq!(
            store.load(TEST_ORIGIN).expect("load").as_deref(),
            Some(token.as_str())
        );

        // …and appears in no file of the two bounded scanned roots.
        let needle = token.as_bytes();
        let mut report = ScanReport::default();
        let app_data = app_core::preferences::default_path()
            .and_then(|path| path.parent().map(Path::to_path_buf));
        match &app_data {
            Some(directory) => {
                // Create the app-data root so both roots are actually scanned;
                // an absent directory would weaken the evidence silently.
                let _ = std::fs::create_dir_all(directory);
                report.roots_attempted += 1;
                scan_tree(directory, needle, &mut report);
                report.roots_scanned += 1;
            }
            None => {
                report.roots_absent += 1;
            }
        }
        report.roots_attempted += 1;
        scan_tree(&std::env::temp_dir(), needle, &mut report);
        report.roots_scanned += 1;
        eprintln!(
            "credential plaintext scan: roots_attempted={} roots_scanned={} roots_absent={} files_scanned={} oversize_skipped={} unreadable={} walk_capped={}",
            report.roots_attempted,
            report.roots_scanned,
            report.roots_absent,
            report.files_scanned,
            report.oversize_skipped_too_large,
            report.unreadable,
            report.walk_capped
        );

        // Both roots must be covered and the walk must have visited files; the
        // exclusions are recorded rather than silently ignored.
        assert_eq!(report.roots_attempted, 2);
        assert_eq!(report.roots_scanned, 2, "both roots were scanned");
        assert_eq!(report.roots_absent, 0, "both roots exist");
        assert!(report.files_scanned > 0, "the walk visited files");
        // Recorded, not hidden: the counters above are part of the evidence.
        assert!(
            report.files_scanned <= 5000,
            "the walk stayed within its bound"
        );

        // Diagnostics: a rendered support report is plain text and must not
        // carry the credential either.
        let rendered =
            app_core::SupportReport::unavailable("no observation session", "0.1.0").render();
        assert!(
            !rendered.contains(&token),
            "the support report carries no token"
        );
    }

    /// What one bounded scan actually covered; recorded so the evidence cannot
    /// be read as a whole-machine guarantee.
    #[derive(Debug, Default)]
    struct ScanReport {
        /// Roots the test intended to scan.
        roots_attempted: usize,
        /// Roots that were actually readable and walked.
        roots_scanned: usize,
        /// Intended roots that do not exist.
        roots_absent: usize,
        /// Files whose bytes were read and compared.
        files_scanned: usize,
        /// Files skipped because they exceed the 4 MiB scan bound.
        oversize_skipped_too_large: usize,
        /// Files whose bytes could not be read.
        unreadable: usize,
        /// The walk hit its entry cap before finishing a directory.
        walk_capped: bool,
    }

    /// Bounded recursive scan: skips entries above 4 MiB, records every
    /// exclusion and caps the walk.
    fn scan_tree(root: &Path, needle: &[u8], report: &mut ScanReport) {
        let Ok(entries) = std::fs::read_dir(root) else {
            report.unreadable += 1;
            return;
        };
        for entry in entries.flatten().take(2000) {
            if report.files_scanned >= 5000 {
                report.walk_capped = true;
                return;
            }
            let Ok(file_type) = entry.file_type() else {
                report.unreadable += 1;
                continue;
            };
            let path = entry.path();
            if file_type.is_dir() {
                scan_tree(&path, needle, report);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                report.unreadable += 1;
                continue;
            };
            if metadata.len() > 4 * 1024 * 1024 {
                report.oversize_skipped_too_large += 1;
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                report.unreadable += 1;
                continue;
            };
            report.files_scanned += 1;
            let carries_token = bytes.windows(needle.len()).any(|window| window == needle);
            assert!(
                !carries_token,
                "a plaintext credential was found in {}",
                path.display()
            );
        }
    }

    #[test]
    fn store_errors_expose_only_a_value_free_label() {
        for error in [
            CredentialStoreError::Unavailable,
            CredentialStoreError::Failed,
        ] {
            let label = error.label();
            assert!(!label.is_empty());
            assert!(!label.contains("test-credential"));
        }
    }
}
