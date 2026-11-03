//! Desktop-local process ownership, independent of Runtime and preferences data.
use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AcquireError {
    AlreadyRunning,
    /// The guard could not be established. The reason is a closed set so the
    /// failure can be reported without exposing any path or OS error text.
    Unavailable(UnavailableReason),
}

/// Which step of the ownership guard failed. Values are fixed and carry no data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnavailableReason {
    NoDefaultPath,
    NoParent,
    RelativeDirectory,
    CreateDirectory,
    OpenDirectory,
    DirectoryMetadata,
    NotADirectory,
    DirectoryIsReparsePoint,
    OpenLockFile,
    LockMetadata,
    LockIsNotFile,
    LockIsReparsePoint,
    LockError,
}

impl UnavailableReason {
    /// Stable token for the startup message; never contains a path.
    pub(crate) fn token(self) -> &'static str {
        match self {
            Self::NoDefaultPath => "no-default-path",
            Self::NoParent => "no-parent",
            Self::RelativeDirectory => "relative-directory",
            Self::CreateDirectory => "create-directory",
            Self::OpenDirectory => "open-directory",
            Self::DirectoryMetadata => "directory-metadata",
            Self::NotADirectory => "not-a-directory",
            Self::DirectoryIsReparsePoint => "directory-reparse-point",
            Self::OpenLockFile => "open-lock-file",
            Self::LockMetadata => "lock-metadata",
            Self::LockIsNotFile => "lock-is-not-file",
            Self::LockIsReparsePoint => "lock-reparse-point",
            Self::LockError => "lock-error",
        }
    }
}

/// The OS releases both handles on normal return and process termination.
/// Keep the lock file: its existence is not ownership, and unlinking it would
/// allow a different file object to acquire the same application identity.
pub(crate) struct InstanceGuard {
    _lock: File,
    #[cfg(windows)]
    _directory: File,
}

impl InstanceGuard {
    pub(crate) fn acquire() -> Result<Self, AcquireError> {
        let path = app_core::preferences::default_path()
            .ok_or(AcquireError::Unavailable(UnavailableReason::NoDefaultPath))?;
        let directory = path
            .parent()
            .ok_or(AcquireError::Unavailable(UnavailableReason::NoParent))?;
        if !directory.is_absolute() {
            return Err(AcquireError::Unavailable(
                UnavailableReason::RelativeDirectory,
            ));
        }
        Self::in_directory(directory)
    }

    fn in_directory(directory: &Path) -> Result<Self, AcquireError> {
        std::fs::create_dir_all(directory)
            .map_err(|_| AcquireError::Unavailable(UnavailableReason::CreateDirectory))?;
        #[cfg(windows)]
        let directory_guard = {
            use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
            // Open the actual directory entry, not a reparse target; withhold
            // delete sharing so the ownership namespace cannot be renamed.
            let handle = OpenOptions::new()
                .read(true)
                .share_mode(0x3) // FILE_SHARE_READ | FILE_SHARE_WRITE
                .custom_flags(0x0220_0000) // BACKUP_SEMANTICS | OPEN_REPARSE_POINT
                .open(directory)
                .map_err(|_| AcquireError::Unavailable(UnavailableReason::OpenDirectory))?;
            let metadata = handle
                .metadata()
                .map_err(|_| AcquireError::Unavailable(UnavailableReason::DirectoryMetadata))?;
            if !metadata.is_dir() {
                return Err(AcquireError::Unavailable(UnavailableReason::NotADirectory));
            }
            if metadata.file_attributes() & 0x400 != 0 {
                return Err(AcquireError::Unavailable(
                    UnavailableReason::DirectoryIsReparsePoint,
                ));
            }
            handle
        };
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(0x3).custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
        }
        let lock = options
            .open(directory.join("desktop.instance"))
            .map_err(|_| AcquireError::Unavailable(UnavailableReason::OpenLockFile))?;
        let metadata = lock
            .metadata()
            .map_err(|_| AcquireError::Unavailable(UnavailableReason::LockMetadata))?;
        if !metadata.is_file() {
            return Err(AcquireError::Unavailable(UnavailableReason::LockIsNotFile));
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err(AcquireError::Unavailable(
                    UnavailableReason::LockIsReparsePoint,
                ));
            }
        }
        match lock.try_lock() {
            Ok(()) => Ok(Self {
                _lock: lock,
                #[cfg(windows)]
                _directory: directory_guard,
            }),
            Err(TryLockError::WouldBlock) => Err(AcquireError::AlreadyRunning),
            Err(TryLockError::Error(_)) => {
                Err(AcquireError::Unavailable(UnavailableReason::LockError))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc, Barrier,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    };
    use std::time::Duration;

    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Directory(std::path::PathBuf);
    impl Directory {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!(
                "cos-instance-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )))
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn ownership_is_the_os_lock_not_the_persistent_file() {
        let directory = Directory::new();
        let owner = InstanceGuard::in_directory(&directory.0).expect("first owner");
        assert_eq!(
            InstanceGuard::in_directory(&directory.0).err(),
            Some(AcquireError::AlreadyRunning)
        );
        drop(owner);
        assert!(directory.0.join("desktop.instance").exists());
        let restarted = InstanceGuard::in_directory(&directory.0).expect("released ownership");
        drop(restarted);
    }

    #[test]
    fn concurrent_acquisition_has_exactly_one_owner() {
        let directory = Directory::new();
        let start = Arc::new(Barrier::new(3));
        let (results, received) = mpsc::channel();
        let mut joins = Vec::new();
        let mut releases = Vec::new();
        for _ in 0..2 {
            let path = directory.0.clone();
            let start = Arc::clone(&start);
            let results = results.clone();
            let (release, wait) = mpsc::channel();
            releases.push(release);
            joins.push(std::thread::spawn(move || {
                start.wait();
                let owner = InstanceGuard::in_directory(&path);
                results
                    .send(owner.as_ref().map(|_| ()).map_err(|e| *e))
                    .expect("result");
                if owner.is_ok() {
                    // Timeout bounds a broken test; it is not the contention oracle.
                    let _ = wait.recv_timeout(Duration::from_secs(5));
                }
            }));
        }
        start.wait();
        let outcomes = [
            received
                .recv_timeout(Duration::from_secs(5))
                .expect("first result"),
            received
                .recv_timeout(Duration::from_secs(5))
                .expect("second result"),
        ];
        for release in releases {
            let _ = release.send(());
        }
        for join in joins {
            join.join().expect("acquisition joins");
        }
        assert_eq!(outcomes.iter().filter(|r| r.is_ok()).count(), 1);
        assert_eq!(
            outcomes
                .iter()
                .filter(|r| **r == Err(AcquireError::AlreadyRunning))
                .count(),
            1
        );
    }

    #[cfg(windows)]
    #[test]
    fn owned_file_and_directory_cannot_be_replaced() {
        let directory = Directory::new();
        let owner = InstanceGuard::in_directory(&directory.0).expect("owner");
        let lock = directory.0.join("desktop.instance");
        assert!(std::fs::remove_file(&lock).is_err());
        assert!(std::fs::rename(&lock, directory.0.join("replacement")).is_err());
        assert!(std::fs::rename(&directory.0, directory.0.with_extension("moved")).is_err());
        assert_eq!(
            InstanceGuard::in_directory(&directory.0).err(),
            Some(AcquireError::AlreadyRunning)
        );
        drop(owner);
    }

    #[test]
    fn every_unavailable_reason_has_a_stable_path_free_token() {
        use UnavailableReason::*;
        let all = [
            NoDefaultPath,
            NoParent,
            RelativeDirectory,
            CreateDirectory,
            OpenDirectory,
            DirectoryMetadata,
            NotADirectory,
            DirectoryIsReparsePoint,
            OpenLockFile,
            LockMetadata,
            LockIsNotFile,
            LockIsReparsePoint,
            LockError,
        ];
        let mut seen = std::collections::BTreeSet::new();
        for reason in all {
            let token = reason.token();
            assert!(!token.is_empty());
            assert!(token.chars().all(|c| c.is_ascii_lowercase() || c == '-'));
            // Tokens carry no path separators or drive letters.
            assert!(!token.contains('\\') && !token.contains('/') && !token.contains(':'));
            assert!(seen.insert(token), "duplicate token {token}");
        }
        assert_eq!(seen.len(), all.len());
    }

    #[test]
    fn invalid_namespace_is_unavailable_not_a_claimed_collision() {
        let directory = Directory::new();
        std::fs::write(&directory.0, b"keep").expect("non-directory namespace");
        assert_eq!(
            InstanceGuard::in_directory(&directory.0).err(),
            Some(AcquireError::Unavailable(UnavailableReason::OpenDirectory))
        );
        assert_eq!(
            std::fs::read(&directory.0).expect("original bytes"),
            b"keep"
        );
        std::fs::remove_file(&directory.0).expect("remove test file");
    }
}
