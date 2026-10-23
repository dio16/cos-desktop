//! Pin every component before resolving the next one. Reparse objects are never followed.
//! Reuses the existing Windows media-picker boundary's safe filepath/OpenOptions pattern.
use filepath::FilePath;
use std::{
    fs::{File, OpenOptions},
    io,
    os::windows::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Component, Path, PathBuf, Prefix},
};

const BACKUP: u32 = 0x0200_0000;
const NO_FOLLOW: u32 = 0x0020_0000;
const REPARSE: u32 = 0x400;

pub fn local_path(path: &Path) -> io::Result<PathBuf> {
    let mut parts = path.components();
    let drive = match parts.next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => drive,
            _ => return Err(io::ErrorKind::InvalidInput.into()),
        },
        _ => return Err(io::ErrorKind::InvalidInput.into()),
    };
    if parts.next() != Some(Component::RootDir) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut result = PathBuf::from(format!("{}:\\", char::from(drive)));
    for part in parts {
        match part {
            Component::Normal(name)
                if !name.encode_wide().any(|c| c == 0 || c == u16::from(b':')) =>
            {
                result.push(name)
            }
            _ => return Err(io::ErrorKind::InvalidInput.into()),
        }
    }
    Ok(result)
}

pub struct PinnedPath {
    // Ancestors remain alive even if the final path is moved out of this struct.
    #[allow(dead_code)]
    parents: Vec<File>,
    pub file: File,
    path: PathBuf,
}

fn open(path: &Path, directory: bool) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .custom_flags(NO_FOLLOW | if directory { BACKUP } else { 0 })
        .open(path)?;
    let metadata = file.metadata()?;
    if metadata.file_attributes() & REPARSE != 0
        || metadata.is_dir() != directory
        || (!directory && !metadata.is_file())
    {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    if local_path(&file.path()?)? != path {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok(file)
}

impl PinnedPath {
    pub fn existing(path: &Path, directory: bool) -> io::Result<Self> {
        let path = local_path(path)?;
        let mut ancestors: Vec<_> = path.ancestors().skip(1).collect();
        ancestors.reverse();
        let mut parents = Vec::with_capacity(ancestors.len());
        for ancestor in ancestors {
            parents.push(open(ancestor, true)?);
        }
        let file = open(&path, directory)?;
        Ok(Self {
            parents,
            file,
            path,
        })
    }

    pub fn verify(&self) -> io::Result<()> {
        // Full ancestry is non-reparse and denies write/delete sharing for its lifetime.
        // Checking the held object's actual path is additional evidence, not a raced exists().
        if local_path(&self.file.path()?)? != self.path {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

pub fn selftest() -> io::Result<()> {
    let root = std::env::temp_dir().join(format!("r0-pin-{}", std::process::id()));
    std::fs::create_dir(&root)?;
    let directory = root.join("owned");
    std::fs::create_dir(&directory)?;
    let pin = PinnedPath::existing(&directory, true)?;
    let path = directory.join("fixture.bin");
    std::fs::write(&path, b"pinned")?;
    let file = PinnedPath::existing(&path, false)?;
    let write_denied = OpenOptions::new().write(true).open(&path).is_err();
    let delete_denied = std::fs::remove_file(&path).is_err();
    let run_rename_denied = std::fs::rename(&directory, root.join("moved")).is_err();
    let ancestor_rename_denied = std::fs::rename(&root, root.with_extension("moved")).is_err();
    file.verify()?;
    pin.verify()?;
    drop(file);
    drop(pin);
    std::fs::remove_dir_all(&root)?;
    println!(
        "{{\"write_denied\":{write_denied},\"delete_denied\":{delete_denied},\"run_rename_denied\":{run_rename_denied},\"ancestor_rename_denied\":{ancestor_rename_denied}}}"
    );
    if write_denied && delete_denied && run_rename_denied && ancestor_rename_denied {
        Ok(())
    } else {
        Err(io::ErrorKind::PermissionDenied.into())
    }
}
