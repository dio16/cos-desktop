//! R1-01–R1-04 use real Windows file handles, not native picker mocks.
//! No Native UI/input/AX success follows from these headless filesystem tests.
#![cfg(windows)]
#[path = "../examples/media_picker/boundary.rs"]
mod boundary;
use boundary::FixtureRoot;
use media::MediaError;
use std::{
    fs::{File, OpenOptions},
    io::Read,
    os::windows::fs::OpenOptionsExt,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Inputs {
    base: PathBuf,
    root: PathBuf,
    outside: PathBuf,
    selected: PathBuf,
}
impl Inputs {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!(
            "img1-boundary-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let root = base.join("fixture");
        std::fs::create_dir_all(&root).unwrap();
        // The probe reaches this boundary with an already canonical root (`main`
        // canonicalizes before opening) and the boundary compares the handle-derived
        // path. An 8.3 short component in `%TEMP%` would otherwise make the expected
        // and opened spellings disagree for a directory that is in fact correct.
        let root = root.canonicalize().unwrap();
        let base = root.parent().unwrap().to_path_buf();
        let selected = root.join("inside.png");
        let outside = base.join("outside.png");
        std::fs::write(&selected, b"\x89PNG\r\n\x1a\ninside").unwrap();
        std::fs::write(&outside, b"outside-secret-marker").unwrap();
        Self {
            base,
            root,
            selected,
            outside,
        }
    }
    fn root(&self) -> FixtureRoot {
        FixtureRoot::open(&self.root).unwrap()
    }
}
impl Drop for Inputs {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.base).unwrap();
    }
}

#[test]
fn direct_regular_fixture_reads_exact_selected_bytes() {
    let inputs = Inputs::new();
    let root = inputs.root();
    let mut file = root.open_selected(&inputs.selected).unwrap();
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"\x89PNG\r\n\x1a\ninside");
}

#[test]
fn actual_outside_opened_object_cannot_pass_with_previously_valid_selected_path() {
    let inputs = Inputs::new();
    let root = inputs.root();
    // Deterministic TOCTOU outcome: selected name remains plausible, opened object is elsewhere.
    let opened_elsewhere = File::open(&inputs.outside).unwrap();
    assert_eq!(
        root.validate_opened(&inputs.selected, opened_elsewhere)
            .err(),
        Some(MediaError::InvalidMetadata)
    );
}

#[test]
fn outside_selected_path_is_rejected_even_for_regular_file() {
    let inputs = Inputs::new();
    let root = inputs.root();
    assert_eq!(
        root.open_selected(&inputs.outside).err(),
        Some(MediaError::InvalidMetadata)
    );
    assert_eq!(
        root.validate_opened(&inputs.outside, File::open(&inputs.outside).unwrap())
            .err(),
        Some(MediaError::InvalidMetadata)
    );
}

#[test]
fn rejected_no_delete_sharing_handle_is_closed_before_return() {
    let inputs = Inputs::new();
    let root = inputs.root();
    let file = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&inputs.outside)
        .unwrap();
    assert_eq!(
        root.validate_opened(&inputs.selected, file).err(),
        Some(MediaError::InvalidMetadata)
    );
    // Windows deletion would fail if the rejected no-FILE_SHARE_DELETE handle were retained.
    std::fs::remove_file(&inputs.outside).unwrap();
    std::fs::write(
        &inputs.outside,
        b"replacement-after-rejected-handle-release",
    )
    .unwrap();
    assert_eq!(
        std::fs::read(&inputs.outside).unwrap(),
        b"replacement-after-rejected-handle-release"
    );
}
