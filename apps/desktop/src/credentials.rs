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

    /// Per-test sequence, so two parallel tests can never share a service name
    /// even when the clock has coarse granularity.
    static STORE_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn unique_store(label: &str) -> WindowsCredentialStore {
        let sequence = STORE_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        WindowsCredentialStore {
            service_prefix: format!(
                "cos-desktop-test/{label}/{}/{sequence}/{nanos}",
                std::process::id()
            ),
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
        let store = unique_store("roundtrip");
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
    /// of files under two roots the test creates itself. Deliberately not a
    /// whole-machine claim — see the per-root report and plan §19.115 limits.
    #[test]
    fn scans_a_bounded_set_of_files_without_a_plaintext_copy() {
        let store = unique_store("scan");
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

        // The two required roots depend on nothing but the test binary's own
        // directory: two sibling directories that cannot overlap, each holding
        // known probe files so the walk is verifiable.
        let binary_dir = std::env::current_exe()
            .expect("the test binary path")
            .parent()
            .expect("a binary directory")
            .to_path_buf();
        let base = binary_dir.join(format!(
            "sec1-scan-{}-{}",
            std::process::id(),
            unique_token("dir")
        ));
        let root_a = base.join("a");
        let root_b = base.join("b");
        let cleanup = ScanRootCleanup(base.clone());
        std::fs::create_dir_all(&root_a).expect("scan root a is creatable");
        std::fs::create_dir_all(&root_b).expect("scan root b is creatable");
        let probe = b"probe file with no credential copy\n";
        for root in [&root_a, &root_b] {
            for index in 0..3 {
                std::fs::write(root.join(format!("probe-{index}.txt")), probe)
                    .expect("a probe file is writable");
            }
        }

        let needle = token.as_bytes();
        let mut report = ScanReport::default();
        let required_scanned = scan_required_roots(
            &[(&root_a, "required-a"), (&root_b, "required-b")],
            needle,
            &mut report,
        );
        // Optional roots never gate the test: app data and the system temp
        // directory are scanned when they exist and recorded when absent, so a
        // missing LOCALAPPDATA/TEMP cannot fail it.
        match app_core::preferences::default_path()
            .and_then(|path| path.parent().map(Path::to_path_buf))
        {
            Some(app_data) => scan_optional_root(&app_data, "app-data", needle, &mut report),
            None => {
                report.roots_absent += 1;
                report.optional_roots_skipped += 1;
            }
        }
        scan_optional_root(&std::env::temp_dir(), "temp", needle, &mut report);
        drop(cleanup);

        for scan in &report.roots {
            eprintln!(
                "credential plaintext scan root: root={} files_scanned={} oversize_skipped={} capped={} unreadable_entries={}",
                scan.label,
                scan.files_scanned,
                scan.oversize_skipped_too_large,
                scan.capped,
                scan.unreadable_entries
            );
        }
        eprintln!(
            "credential plaintext scan summary: roots_attempted={} roots_scanned={} roots_absent={} roots_unreadable={} optional_skipped={} files_scanned={} oversize_skipped={} unreadable_entries={} walk_capped={}",
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

        // Both required roots were walked fully (no cap), with no unreadable
        // entries, and each really scanned its three probe files.
        assert_eq!(required_scanned, 2, "both required roots were scanned");
        assert_eq!(
            report.roots_unreadable, 0,
            "no required root was unreadable"
        );
        for (label, probes) in [("required-a", 3usize), ("required-b", 3usize)] {
            let scan = report
                .roots
                .iter()
                .find(|scan| scan.label == label)
                .unwrap_or_else(|| panic!("no per-root report for {label}"));
            assert!(!scan.capped, "root {label} was walked fully");
            assert_eq!(
                scan.unreadable_entries, 0,
                "root {label} had no unreadable entry"
            );
            assert!(
                scan.files_scanned >= probes,
                "root {label} scanned its {probes} probe files, got {}",
                scan.files_scanned
            );
        }
        assert!(
            report.files_scanned >= 6,
            "both roots' probe files were scanned"
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

    /// Removes the test-created scan tree when the test ends, including on
    /// failure, so no stray probe directory survives next to the test binary.
    struct ScanRootCleanup(std::path::PathBuf);

    impl Drop for ScanRootCleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Per-root scan accounting: the numbers below are the evidence, so every
    /// root reports its own coverage instead of sharing one global budget.
    #[derive(Debug, Default)]
    struct RootScan {
        /// Fixed label used in logs and failure messages; the real path is never
        /// printed (private-path policy, DC-MP-SEC2).
        label: &'static str,
        /// Files whose bytes were read and compared.
        files_scanned: usize,
        /// Files skipped because they exceed the 4 MiB scan bound.
        oversize_skipped_too_large: usize,
        /// The walk hit a per-directory/per-root cap before finishing.
        capped: bool,
        /// Entries whose metadata or bytes could not be read.
        unreadable_entries: usize,
    }

    /// What the bounded scan actually covered; recorded so the evidence cannot
    /// be read as a whole-machine guarantee.
    #[derive(Debug, Default)]
    struct ScanReport {
        /// Roots the test attempted to scan.
        roots_attempted: usize,
        /// Roots whose directory could be opened and walked.
        roots_scanned: usize,
        /// Optional roots that do not exist on this machine.
        roots_absent: usize,
        /// Required roots that could not be opened.
        roots_unreadable: usize,
        /// Optional roots skipped because they are missing or unreadable.
        optional_roots_skipped: usize,
        /// Files whose bytes were read and compared (sum over roots).
        files_scanned: usize,
        /// Files skipped because they exceed the 4 MiB scan bound (sum).
        oversize_skipped_too_large: usize,
        /// Entries whose metadata or bytes could not be read (sum).
        unreadable_entries: usize,
        /// At least one root hit its cap.
        walk_capped: bool,
        /// Per-root reports, in scan order.
        roots: Vec<RootScan>,
    }

    impl ScanReport {
        fn absorb(&mut self, scan: RootScan) {
            self.files_scanned += scan.files_scanned;
            self.oversize_skipped_too_large += scan.oversize_skipped_too_large;
            self.unreadable_entries += scan.unreadable_entries;
            self.walk_capped |= scan.capped;
            self.roots.push(scan);
        }
    }

    /// Scans the roots the test itself created; returns how many were scanned.
    /// A root that cannot be opened is recorded as unreadable, never counted as
    /// scanned.
    fn scan_required_roots(
        roots: &[(&Path, &'static str)],
        needle: &[u8],
        report: &mut ScanReport,
    ) -> usize {
        let mut scanned = 0;
        for (root, label) in roots {
            report.roots_attempted += 1;
            match scan_root(root, label, needle) {
                Some(scan) => {
                    report.roots_scanned += 1;
                    scanned += 1;
                    report.absorb(scan);
                }
                None => report.roots_unreadable += 1,
            }
        }
        scanned
    }

    /// Optional root: scanned when readable, otherwise recorded as skipped. The
    /// test never requires an environment directory to exist.
    fn scan_optional_root(
        root: &Path,
        label: &'static str,
        needle: &[u8],
        report: &mut ScanReport,
    ) {
        if !root.exists() {
            report.roots_absent += 1;
            report.optional_roots_skipped += 1;
            return;
        }
        report.roots_attempted += 1;
        match scan_root(root, label, needle) {
            Some(scan) => {
                report.roots_scanned += 1;
                report.absorb(scan);
            }
            None => report.optional_roots_skipped += 1,
        }
    }

    /// Determinism guard: an absent optional root and an unreadable required
    /// root are recorded, never counted as scanned, and never fail the scan for
    /// an environment reason.
    #[test]
    fn an_absent_or_unreadable_root_is_recorded_and_never_counted_as_scanned() {
        let missing = Path::new("C:/__cos_sec1_no_such_root__/nested");
        let mut required = ScanReport::default();
        assert_eq!(
            scan_required_roots(&[(missing, "required-missing")], b"needle", &mut required),
            0
        );
        assert_eq!(required.roots_attempted, 1);
        assert_eq!(required.roots_scanned, 0);
        assert_eq!(required.roots_unreadable, 1);

        let mut optional = ScanReport::default();
        scan_optional_root(missing, "optional-missing", b"needle", &mut optional);
        assert_eq!(optional.roots_scanned, 0);
        assert_eq!(optional.roots_absent, 1);
        assert_eq!(optional.optional_roots_skipped, 1);
        assert_eq!(optional.roots_unreadable, 0);
    }

    /// Per-directory entry budget and per-root file budget. Both are checked
    /// per root, so one large root cannot consume another root's coverage.
    const DIRECTORY_ENTRY_BUDGET: usize = 2000;
    const ROOT_FILE_BUDGET: usize = 2000;
    const MAX_SCAN_DEPTH: usize = 16;
    const MAX_SCANNED_FILE_BYTES: u64 = 4 * 1024 * 1024;

    /// Bounded scan of one root; `None` when the root itself cannot be opened.
    fn scan_root(root: &Path, label: &'static str, needle: &[u8]) -> Option<RootScan> {
        std::fs::read_dir(root).ok()?;
        let mut scan = RootScan {
            label,
            ..RootScan::default()
        };
        walk(root, needle, &mut scan, 0);
        Some(scan)
    }

    /// Recursive walk with per-root accounting and bounded depth/count.
    fn walk(directory: &Path, needle: &[u8], scan: &mut RootScan, depth: usize) {
        if depth >= MAX_SCAN_DEPTH {
            scan.capped = true;
            return;
        }
        let Ok(entries) = std::fs::read_dir(directory) else {
            scan.unreadable_entries += 1;
            return;
        };
        let mut entries = entries;
        let mut taken = 0usize;
        loop {
            if scan.files_scanned >= ROOT_FILE_BUDGET {
                scan.capped = true;
                return;
            }
            let entry = match entries.next() {
                Some(Ok(entry)) => entry,
                Some(Err(_)) => {
                    // A directory entry that exists but cannot be enumerated is
                    // unverified, not clean: count it instead of dropping it.
                    scan.unreadable_entries += 1;
                    continue;
                }
                None => return,
            };
            if taken >= DIRECTORY_ENTRY_BUDGET {
                scan.capped = true;
                return;
            }
            taken += 1;
            let Ok(file_type) = entry.file_type() else {
                scan.unreadable_entries += 1;
                continue;
            };
            let path = entry.path();
            if file_type.is_dir() {
                walk(&path, needle, scan, depth + 1);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                scan.unreadable_entries += 1;
                continue;
            };
            if metadata.len() > MAX_SCANNED_FILE_BYTES {
                scan.oversize_skipped_too_large += 1;
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                scan.unreadable_entries += 1;
                continue;
            };
            scan.files_scanned += 1;
            let carries_token = bytes.windows(needle.len()).any(|window| window == needle);
            assert!(
                !carries_token,
                "a plaintext credential was found while scanning root '{}'",
                scan.label
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
