//! Application-local Fixture probe boundary, not a production filesystem sandbox.
use media::MediaError;
use std::{
    fs::File,
    path::{Path, PathBuf},
};

#[cfg(windows)]
use {
    filepath::FilePath,
    std::{
        fs::OpenOptions,
        os::windows::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
        },
    },
};

/// Pins the explicit ordinary directory and its actual handle-derived location.
/// No delete/write sharing prevents replacing that root while the probe owns it.
pub struct FixtureRoot {
    #[cfg(windows)]
    directory: File,
    path: PathBuf,
}
impl FixtureRoot {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn open(path: &Path) -> Result<Self, MediaError> {
        #[cfg(windows)]
        {
            let expected = local_disk_path(path)?;
            let directory = OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(&expected)?;
            let metadata = directory.metadata()?;
            if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            {
                return Err(MediaError::InvalidMetadata);
            }
            let actual = local_disk_path(&directory.path()?)?;
            if actual != expected {
                return Err(MediaError::InvalidMetadata);
            }
            Ok(Self {
                directory,
                path: actual,
            })
        }
        #[cfg(not(windows))]
        {
            let _ = path;
            Err(MediaError::Unavailable)
        }
    }

    pub fn open_selected(&self, selected: &Path) -> Result<File, MediaError> {
        #[cfg(windows)]
        {
            let selected = self.selected_path(selected)?;
            // Inspect the opened reparse object rather than following a swapped final link.
            // Denying write/delete sharing also keeps this file stable during encoded acquisition.
            let file = OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .open(&selected)?;
            self.validate_opened(&selected, file)
        }
        #[cfg(not(windows))]
        {
            let _ = selected;
            Err(MediaError::Unavailable)
        }
    }

    /// Verify this actual opened object. A previously checked path is not evidence of containment.
    /// Consumes and closes rejected File capabilities before any media read/cache allocation.
    #[cfg(windows)]
    pub fn validate_opened(&self, selected: &Path, file: File) -> Result<File, MediaError> {
        let selected = self.selected_path(selected)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(MediaError::NotRegularFile);
        }
        let actual = local_disk_path(&file.path()?)?;
        let current_root = local_disk_path(&self.directory.path()?)?;
        if current_root != self.path || actual.parent() != Some(self.path()) || actual != selected {
            return Err(MediaError::InvalidMetadata);
        }
        Ok(file)
    }

    #[cfg(windows)]
    fn selected_path(&self, selected: &Path) -> Result<PathBuf, MediaError> {
        let selected = local_disk_path(selected)?;
        if selected.parent() != Some(self.path())
            || selected
                .file_name()
                .is_none_or(|name| name.encode_wide().any(|c| c == 0 || c == u16::from(b':')))
        {
            return Err(MediaError::InvalidMetadata);
        }
        Ok(selected)
    }
}

#[cfg(windows)]
const FILE_SHARE_READ: u32 = 0x00000001;
#[cfg(windows)]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x00200000;
#[cfg(windows)]
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x02000000;
#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x00000400;

/// Normalize only DOS/extended-DOS spelling, without a second path-based filesystem lookup.
/// Unknown/nonlocal/relative/parent traversal forms fail closed; Unicode OsStr is preserved.
#[cfg(windows)]
fn local_disk_path(path: &Path) -> Result<PathBuf, MediaError> {
    use std::path::{Component, Prefix};
    let mut components = path.components();
    let drive = match components.next() {
        Some(Component::Prefix(p)) => match p.kind() {
            Prefix::Disk(d) | Prefix::VerbatimDisk(d) => d,
            _ => return Err(MediaError::InvalidMetadata),
        },
        _ => return Err(MediaError::InvalidMetadata),
    };
    if components.next() != Some(Component::RootDir) {
        return Err(MediaError::InvalidMetadata);
    }
    let mut result = PathBuf::from(format!("{}:\\", char::from(drive)));
    for component in components {
        match component {
            Component::Normal(name) => result.push(name),
            _ => return Err(MediaError::InvalidMetadata),
        }
    }
    Ok(result)
}
