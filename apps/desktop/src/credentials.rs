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
    /// of files under the app-data root, a test-created sibling root and (when
    /// the system temp directory is available) the temp root. This is
    /// deliberately not a whole-machine claim — see the recorded exclusions and
    /// the plan §19.115 limits.
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

        // Required roots are created by the test, so the scan never depends on
        // an environment directory existing: the app-data root and a dedicated
        // sibling directory that also carries one harmless probe file.
        let app_data = app_core::preferences::default_path()
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .expect("the app-data root is resolvable on this platform");
        std::fs::create_dir_all(&app_data).expect("the app-data root is creatable");
        let scan_root = app_data.join(format!(
            "sec1-scan-{}-{}",
            std::process::id(),
            unique_token("dir")
        ));
        std::fs::create_dir_all(&scan_root).expect("the test scan root is creatable");
        let cleanup = ScanRootCleanup(scan_root.clone());
        std::fs::write(
            scan_root.join("probe.txt"),
            b"no credential here
",
        )
        .expect("the probe file is writable");

        let needle = token.as_bytes();
        let mut report = ScanReport::default();
        let required_scanned = scan_required_roots(&[&app_data, &scan_root], needle, &mut report);
        // Optional third root: the system temp directory, scanned only when it
        // exists and is readable. Its absence is recorded, never hidden and
        // never required (gate environments do not guarantee it).
        scan_optional_root(&std::env::temp_dir(), needle, &mut report);
        drop(cleanup);

        eprintln!(
            "credential plaintext scan: roots_attempted={} roots_scanned={} roots_absent={} roots_unreadable={} optional_skipped={} files_scanned={} oversize_skipped={} unreadable_entries={} walk_capped={}",
            report.roots_attempted,
            report.roots_scanned,
            report.roots_absent,
            report.roots_unreadable,
            report.optional_roots_skipped,
            report.files_scanned,
            report.oversize_skipped_too_large,
            report.unreadable_entries,
            report.walk_capped
        );

        // The two required roots were created above and must both be readable;
        // the walk visited at least the probe file.
        assert_eq!(required_scanned, 2, "both required roots were scanned");
        assert_eq!(report.roots_unreadable, 0, "no passed root was unreadable");
        assert!(report.files_scanned > 0, "the walk visited files");
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

    /// Removes the test-created scan root when the test ends, including on
    /// failure, so no stray probe directory survives in app data.
    struct ScanRootCleanup(std::path::PathBuf);

    impl Drop for ScanRootCleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Scans the roots the test itself created; returns how many were scanned.
    /// A root that cannot be opened is recorded as unreadable, never counted as
    /// scanned.
    fn scan_required_roots(roots: &[&Path], needle: &[u8], report: &mut ScanReport) -> usize {
        let mut scanned = 0;
        for root in roots {
            report.roots_attempted += 1;
            if scan_tree(root, needle, report) {
                report.roots_scanned += 1;
                scanned += 1;
            } else {
                report.roots_unreadable += 1;
            }
        }
        scanned
    }

    /// Optional root: scanned when readable, otherwise recorded as skipped. The
    /// test never requires an environment directory to exist.
    fn scan_optional_root(root: &Path, needle: &[u8], report: &mut ScanReport) {
        if !root.exists() {
            report.roots_absent += 1;
            report.optional_roots_skipped += 1;
            return;
        }
        report.roots_attempted += 1;
        if scan_tree(root, needle, report) {
            report.roots_scanned += 1;
        } else {
            report.roots_unreadable += 1;
            report.optional_roots_skipped += 1;
        }
    }

    /// Determinism guard: an absent optional root and an unreadable required
    /// root are recorded, never counted as scanned, and never fail the scan for
    /// an environment reason.
    #[test]
    fn an_absent_or_unreadable_root_is_recorded_and_never_counted_as_scanned() {
        let missing = Path::new("C:/__cos_sec1_no_such_root__/nested");
        let mut required = ScanReport::default();
        assert_eq!(scan_required_roots(&[missing], b"needle", &mut required), 0);
        assert_eq!(required.roots_attempted, 1);
        assert_eq!(required.roots_scanned, 0);
        assert_eq!(required.roots_unreadable, 1);

        let mut optional = ScanReport::default();
        scan_optional_root(missing, b"needle", &mut optional);
        assert_eq!(optional.roots_scanned, 0);
        assert_eq!(optional.roots_absent, 1);
        assert_eq!(optional.optional_roots_skipped, 1);
        assert_eq!(optional.roots_unreadable, 0);
    }

    /// What one bounded scan actually covered; recorded so the evidence cannot
    /// be read as a whole-machine guarantee.
    #[derive(Debug, Default)]
    struct ScanReport {
        /// Roots the test attempted to scan.
        roots_attempted: usize,
        /// Roots whose directory could be opened and walked.
        roots_scanned: usize,
        /// Optional roots that do not exist on this machine.
        roots_absent: usize,
        /// Roots passed by the test that could not be opened.
        roots_unreadable: usize,
        /// Optional roots skipped because they are missing or unreadable.
        optional_roots_skipped: usize,
        /// Files whose bytes were read and compared.
        files_scanned: usize,
        /// Files skipped because they exceed the 4 MiB scan bound.
        oversize_skipped_too_large: usize,
        /// Entries whose metadata or bytes could not be read.
        unreadable_entries: usize,
        /// The walk hit its entry cap before finishing a directory.
        walk_capped: bool,
    }

    /// Bounded recursive scan: skips entries above 4 MiB, records every
    /// exclusion and caps the walk. Returns `false` when the root itself cannot
    /// be opened, so the caller counts it as unreadable instead of scanned.
    fn scan_tree(root: &Path, needle: &[u8], report: &mut ScanReport) -> bool {
        let Ok(entries) = std::fs::read_dir(root) else {
            return false;
        };
        for entry in entries.flatten().take(2000) {
            if report.files_scanned >= 5000 {
                report.walk_capped = true;
                return true;
            }
            let Ok(file_type) = entry.file_type() else {
                report.unreadable_entries += 1;
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
                report.unreadable_entries += 1;
                continue;
            };
            if metadata.len() > 4 * 1024 * 1024 {
                report.oversize_skipped_too_large += 1;
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                report.unreadable_entries += 1;
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
        true
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
