//! Closed local preferences for Desktop restoration.
//!
//! Only the DC-MP-REC3 field list is serialized. Canonical projections,
//! credentials, media locators and Runtime commands never enter this file.

use crate::navigation::Surface;
use desktop_protocol::{CandidateRef, TaskId};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_SERIAL: AtomicU64 = AtomicU64::new(1);

/// Current closed schema version. Unknown versions restore defaults.
pub const SCHEMA_VERSION: u32 = 1;
/// Bound on the complete preferences document.
pub const MAX_PREFERENCES_BYTES: u64 = 64 * 1024;
/// Bounded saved zoom/pan entries.
pub const MAX_ZOOM_PAN: usize = 64;
/// Bounded command-palette recents.
pub const MAX_PALETTE_RECENTS: usize = 32;
/// Maximum stored string length for closed preference labels.
pub const MAX_LABEL_BYTES: usize = 128;

/// Logical window geometry plus the DPR observed when it was saved.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowGeometry {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub dpr: f32,
    pub maximized: bool,
}

/// Fixed panel widths that the current shell exposes.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PanelWidths {
    pub rail: f32,
    pub inspector: f32,
}

impl Default for PanelWidths {
    fn default() -> Self {
        Self {
            rail: 210.0,
            inspector: 310.0,
        }
    }
}

/// One locally viewed candidate's saved zoom/pan.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZoomPan {
    pub candidate: CandidateRef,
    pub zoom: f32,
    pub pan_x: f32,
    pub pan_y: f32,
}

/// Complete closed local preference document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    pub schema_version: u32,
    #[serde(default)]
    pub window: Option<WindowGeometry>,
    #[serde(default)]
    pub surface: Surface,
    #[serde(default)]
    pub last_task_id: Option<TaskId>,
    #[serde(default)]
    pub viewed: Option<CandidateRef>,
    #[serde(default)]
    pub compare: Option<[CandidateRef; 2]>,
    #[serde(default)]
    pub panel_widths: PanelWidths,
    #[serde(default)]
    pub zoom_pan: Vec<ZoomPan>,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_density")]
    pub density: String,
    #[serde(default = "default_text_scale")]
    pub text_scale: f32,
    #[serde(default)]
    pub palette_recents: Vec<String>,
}

fn default_theme() -> String {
    "system".to_owned()
}

fn default_density() -> String {
    "comfortable".to_owned()
}

fn default_text_scale() -> f32 {
    1.0
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            window: None,
            surface: Surface::Artifact,
            last_task_id: None,
            viewed: None,
            compare: None,
            panel_widths: PanelWidths::default(),
            zoom_pan: Vec::new(),
            theme: default_theme(),
            density: default_density(),
            text_scale: default_text_scale(),
            palette_recents: Vec::new(),
        }
    }
}

/// Result of loading the local store. A notice is a bounded diagnostic, not a
/// crash or a partially merged document.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoadedPreferences {
    pub preferences: Preferences,
    pub notice: Option<&'static str>,
}

/// Per-user preferences path. No path is guessed outside the OS app-data root.
pub fn default_path() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .map(|base| base.join("cos-desktop").join("preferences.json"))
}

/// Load the closed schema, restoring defaults on corruption or an unknown
/// version. A valid file is never partially merged with defaults.
pub fn load(path: &Path) -> LoadedPreferences {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return LoadedPreferences::default();
        }
        Err(_) => {
            return LoadedPreferences {
                preferences: Preferences::default(),
                notice: Some("Preferences unavailable; defaults restored."),
            };
        }
    };
    // The bound is enforced on the opened handle, not a prior metadata
    // observation: at most MAX+1 bytes are ever acquired.
    let mut bytes = Vec::new();
    let read = match file
        .take(MAX_PREFERENCES_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
    {
        Ok(read) => read,
        Err(_) => {
            return LoadedPreferences {
                preferences: Preferences::default(),
                notice: Some("Preferences unreadable; defaults restored."),
            };
        }
    };
    if read as u64 > MAX_PREFERENCES_BYTES {
        return LoadedPreferences {
            preferences: Preferences::default(),
            notice: Some("Preferences ignored: file exceeds the bounded size."),
        };
    }
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => {
            return LoadedPreferences {
                preferences: Preferences::default(),
                notice: Some("Preferences corrupt; defaults restored."),
            };
        }
    };
    let mut preferences: Preferences = match serde_json::from_str(&text) {
        Ok(preferences) => preferences,
        Err(_) => {
            return LoadedPreferences {
                preferences: Preferences::default(),
                notice: Some("Preferences corrupt; defaults restored."),
            };
        }
    };
    if preferences.schema_version != SCHEMA_VERSION {
        return LoadedPreferences {
            preferences: Preferences::default(),
            notice: Some("Preferences schema unknown; defaults restored."),
        };
    }
    if !preferences.bounds_ok() {
        return LoadedPreferences {
            preferences: Preferences::default(),
            notice: Some("Preferences exceeded bounds; defaults restored."),
        };
    }
    preferences.sanitize();
    LoadedPreferences {
        preferences,
        notice: None,
    }
}

impl Preferences {
    fn bounds_ok(&self) -> bool {
        let valid_label =
            |value: &str| value.len() <= MAX_LABEL_BYTES && !value.chars().any(char::is_control);
        valid_label(&self.theme)
            && valid_label(&self.density)
            && self.text_scale.is_finite()
            && (0.8..=2.0).contains(&self.text_scale)
            && self.zoom_pan.len() <= MAX_ZOOM_PAN
            && self.palette_recents.len() <= MAX_PALETTE_RECENTS
            && self.palette_recents.iter().all(|entry| valid_label(entry))
            && self.zoom_pan.iter().all(|item| {
                item.zoom.is_finite()
                    && item.pan_x.is_finite()
                    && item.pan_y.is_finite()
                    && (0.1..=8.0).contains(&item.zoom)
            })
            && self.panel_widths.rail.is_finite()
            && self.panel_widths.inspector.is_finite()
            && self.window.is_none_or(|window| {
                window.x.is_finite()
                    && window.y.is_finite()
                    && window.width.is_finite()
                    && window.height.is_finite()
                    && window.dpr.is_finite()
            })
    }

    fn sanitize(&mut self) {
        self.panel_widths.rail = self.panel_widths.rail.clamp(140.0, 600.0);
        self.panel_widths.inspector = self.panel_widths.inspector.clamp(180.0, 700.0);
        self.text_scale = self.text_scale.clamp(0.8, 2.0);
        if let Some(window) = self.window.as_mut() {
            window.width = window.width.clamp(1000.0, 10_000.0);
            window.height = window.height.clamp(680.0, 10_000.0);
            window.dpr = window.dpr.clamp(0.5, 4.0);
        }
    }
}

/// Bounded JSON writer: serialization fails once the fixed document cap is
/// exceeded, so no oversized output buffer is retained.
#[derive(Default)]
struct BoundedOutput {
    bytes: Vec<u8>,
}

impl Write for BoundedOutput {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        if self.bytes.len().saturating_add(buffer.len()) as u64 > MAX_PREFERENCES_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "preferences exceed the bounded size",
            ));
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Atomically write the bounded preferences document. A failed write leaves the
/// previous file intact and removes the temporary file.
pub fn save_atomic(path: &Path, preferences: &Preferences) -> Result<(), std::io::Error> {
    let Some(parent) = path.parent() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "preferences path has no parent",
        ));
    };
    std::fs::create_dir_all(parent)?;
    // Validate the borrowed document before any input-proportional clone.
    // The complete serialized size, including fixture IDs that bypass the
    // normal opaque-ID length path, is admitted through the same bounded
    // writer first; only then is a bounded normalized copy made.
    if !preferences.bounds_ok() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "preferences exceed the bounded schema",
        ));
    }
    let mut borrowed = BoundedOutput::default();
    serde_json::to_writer(&mut borrowed, preferences)?;
    let mut sanitized = preferences.clone();
    sanitized.schema_version = SCHEMA_VERSION;
    sanitized.sanitize();
    let mut output = BoundedOutput::default();
    serde_json::to_writer(&mut output, &sanitized)?;
    let temp = path.with_file_name(format!(
        ".preferences.{}.{}.tmp",
        std::process::id(),
        TEMP_SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    let write = || -> Result<(), std::io::Error> {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(&output.bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)
    };
    match write() {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_file(&temp);
            Err(error)
        }
    }
}

/// Logical display rectangle used for removal/reorder-safe clamping.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DisplayRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Clamp saved geometry to the currently present display set.
///
/// A window that still intersects a present display is moved fully inside it;
/// otherwise it is centered on the primary display. The minimum usable size is
/// retained. DPR is metadata only; positioning uses logical pixels.
pub fn clamp_geometry(saved: Option<WindowGeometry>, displays: &[DisplayRect]) -> WindowGeometry {
    let default = WindowGeometry {
        x: 0.0,
        y: 0.0,
        width: 1280.0,
        height: 800.0,
        dpr: 1.0,
        maximized: false,
    };
    let was_saved = saved.is_some();
    let mut window = saved.unwrap_or(default);
    window.width = window.width.clamp(1000.0, 10_000.0);
    window.height = window.height.clamp(680.0, 10_000.0);
    window.dpr = window.dpr.clamp(0.5, 4.0);
    let Some(primary) = displays.first().copied() else {
        window.x = 0.0;
        window.y = 0.0;
        window.maximized = false;
        return window;
    };
    let intersects = |display: DisplayRect| {
        let left = window.x.max(display.x);
        let top = window.y.max(display.y);
        let right = (window.x + window.width).min(display.x + display.width);
        let bottom = (window.y + window.height).min(display.y + display.height);
        (right - left).max(0.0) * (bottom - top).max(0.0)
    };
    let best = displays
        .iter()
        .copied()
        .max_by(|left, right| intersects(*left).total_cmp(&intersects(*right)))
        .filter(|display| intersects(*display) >= 64.0 * 64.0);
    let target = best.unwrap_or(primary);
    // A saved window larger than the selected present display is resized to
    // that display. If the display itself is smaller than the configured
    // minimum, containment wins and the platform minimum may be unachievable;
    // no off-screen extent is silently retained.
    window.width = window.width.min(target.width);
    window.height = window.height.min(target.height);
    if best.is_none() || !was_saved {
        window.x = target.x + (target.width - window.width).max(0.0) / 2.0;
        window.y = target.y + (target.height - window.height).max(0.0) / 2.0;
    } else {
        window.x = window.x.clamp(
            target.x,
            (target.x + target.width - window.width).max(target.x),
        );
        window.y = window.y.clamp(
            target.y,
            (target.y + target.height - window.height).max(target.y),
        );
    }
    window
}

#[cfg(test)]
mod tests {
    use super::{
        DisplayRect, Preferences, SCHEMA_VERSION, WindowGeometry, clamp_geometry, load, save_atomic,
    };
    use crate::navigation::Surface;
    use desktop_protocol::{ArtifactId, CandidateId, CandidateRef, RunId, TaskId};
    use std::io::Write;
    use std::path::PathBuf;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "cos-desktop-preferences-{}-{name}.json",
            std::process::id()
        ))
    }

    fn candidate(label: &str) -> CandidateRef {
        CandidateRef {
            task_id: TaskId::fixture("task/Task-184"),
            run_id: RunId::fixture("run/Task-184/1"),
            iteration: 3,
            candidate_id: CandidateId::fixture(format!("candidate/{label}")),
            artifact_ref: ArtifactId::fixture(format!("artifact/{label}")),
        }
    }

    #[test]
    fn valid_preferences_round_trip_and_replace_atomically() {
        let path = temp_path("round-trip");
        let _ = std::fs::remove_file(&path);
        let mut preferences = Preferences {
            surface: Surface::Flow,
            last_task_id: Some(TaskId::fixture("task/Task-184")),
            viewed: Some(candidate("e")),
            compare: Some([candidate("a"), candidate("b")]),
            ..Preferences::default()
        };
        preferences.window = Some(WindowGeometry {
            x: 40.0,
            y: 50.0,
            width: 1200.0,
            height: 760.0,
            dpr: 1.5,
            maximized: false,
        });
        save_atomic(&path, &preferences).expect("first save");
        let first = load(&path);
        assert_eq!(first.preferences, preferences);
        preferences.surface = Surface::Compare;
        save_atomic(&path, &preferences).expect("replacement save");
        assert_eq!(load(&path).preferences, preferences);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn corrupt_unknown_version_and_unknown_fields_restore_defaults() {
        let path = temp_path("corrupt");
        std::fs::write(&path, "{not-json").expect("write corrupt");
        assert_eq!(load(&path).preferences, Preferences::default());
        std::fs::write(&path, r#"{"schema_version":99}"#).expect("write version");
        assert_eq!(load(&path).preferences, Preferences::default());
        std::fs::write(
            &path,
            r#"{"schema_version":1,"unexpected":{"canonical":"must not load"}}"#,
        )
        .expect("write unknown field");
        assert_eq!(load(&path).preferences, Preferences::default());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn serialized_bytes_contain_only_closed_fields() {
        let path = temp_path("closed");
        let preferences = Preferences {
            last_task_id: Some(TaskId::fixture("task/Task-184")),
            viewed: Some(candidate("e")),
            ..Preferences::default()
        };
        save_atomic(&path, &preferences).expect("save");
        let text = std::fs::read_to_string(&path).expect("read");
        for forbidden in [
            "credential",
            "token",
            "media_locator",
            "snapshot",
            "signed_url",
        ] {
            assert!(
                !text.contains(forbidden),
                "forbidden field present: {forbidden}"
            );
        }
        let parsed: Preferences = serde_json::from_str(&text).expect("closed schema");
        assert_eq!(parsed.schema_version, SCHEMA_VERSION);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn repeated_saves_do_not_grow_or_duplicate_state() {
        let path = temp_path("repeat");
        let preferences = Preferences {
            viewed: Some(candidate("e")),
            ..Preferences::default()
        };
        save_atomic(&path, &preferences).expect("first save");
        let first_len = std::fs::metadata(&path).expect("metadata").len();
        for _ in 0..5 {
            save_atomic(&path, &preferences).expect("repeat save");
        }
        assert_eq!(std::fs::metadata(&path).expect("metadata").len(), first_len);
        assert_eq!(load(&path).preferences, preferences);
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(windows)]
    #[test]
    #[allow(clippy::permissions_set_readonly_false)]
    fn failed_atomic_replace_preserves_the_last_valid_file() {
        let path = temp_path("readonly");
        let first = Preferences {
            surface: Surface::Flow,
            ..Preferences::default()
        };
        save_atomic(&path, &first).expect("first save");
        let mut permissions = std::fs::metadata(&path).expect("metadata").permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions).expect("readonly");
        let changed = Preferences {
            surface: Surface::Compare,
            ..Preferences::default()
        };
        assert!(save_atomic(&path, &changed).is_err());
        let mut permissions = std::fs::metadata(&path).expect("metadata").permissions();
        permissions.set_readonly(false);
        std::fs::set_permissions(&path, permissions).expect("writable");
        assert_eq!(load(&path).preferences, first);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn bounded_read_accepts_exact_limit_and_rejects_larger_input() {
        let path = temp_path("bounded-read");
        let mut exact = br#"{"schema_version":1}"#.to_vec();
        exact.resize(super::MAX_PREFERENCES_BYTES as usize, b' ');
        std::fs::write(&path, &exact).expect("exact write");
        assert_eq!(load(&path).preferences, Preferences::default());
        let mut over = exact;
        over.push(b' ');
        std::fs::write(&path, &over).expect("over write");
        let loaded = load(&path);
        assert_eq!(loaded.preferences, Preferences::default());
        assert!(loaded.notice.is_some());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn oversized_in_memory_values_are_rejected_before_the_valid_file_is_replaced() {
        let path = temp_path("in-memory-bounds");
        let valid = Preferences::default();
        save_atomic(&path, &valid).expect("valid baseline");
        let huge_theme = Preferences {
            theme: "x".repeat(super::MAX_LABEL_BYTES + 1),
            ..Preferences::default()
        };
        let huge_recents = Preferences {
            palette_recents: vec!["x".to_owned(); super::MAX_PALETTE_RECENTS + 1],
            ..Preferences::default()
        };
        let huge_zoom = Preferences {
            zoom_pan: (0..=super::MAX_ZOOM_PAN)
                .map(|index| super::ZoomPan {
                    candidate: candidate(&format!("zoom-{index}")),
                    zoom: 1.0,
                    pan_x: 0.0,
                    pan_y: 0.0,
                })
                .collect(),
            ..Preferences::default()
        };
        let non_finite = Preferences {
            text_scale: f32::NAN,
            ..Preferences::default()
        };
        for invalid in [huge_theme, huge_recents, huge_zoom, non_finite] {
            assert!(save_atomic(&path, &invalid).is_err());
            assert_eq!(load(&path).preferences, valid);
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn complete_borrowed_document_is_bounded_before_clone() {
        let path = temp_path("complete-bound");
        let valid = Preferences::default();
        save_atomic(&path, &valid).expect("valid baseline");
        let huge_identity = Preferences {
            last_task_id: Some(TaskId::fixture(format!("task/{}", "x".repeat(70_000)))),
            ..Preferences::default()
        };
        assert!(save_atomic(&path, &huge_identity).is_err());
        let many_zoom = Preferences {
            zoom_pan: (0..=super::MAX_ZOOM_PAN)
                .map(|index| super::ZoomPan {
                    candidate: CandidateRef {
                        task_id: TaskId::fixture("task/Task-184"),
                        run_id: RunId::fixture("run/Task-184/1"),
                        iteration: 3,
                        candidate_id: CandidateId::fixture(format!(
                            "candidate/{index}/{}",
                            "x".repeat(1_200)
                        )),
                        artifact_ref: ArtifactId::fixture(format!("artifact/{index}")),
                    },
                    zoom: 1.0,
                    pan_x: 0.0,
                    pan_y: 0.0,
                })
                .collect(),
            ..Preferences::default()
        };
        assert!(save_atomic(&path, &many_zoom).is_err());
        assert_eq!(load(&path).preferences, valid);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn serialization_writer_refuses_to_exceed_the_document_bound() {
        let mut output = super::BoundedOutput::default();
        let chunk = vec![b'x'; super::MAX_PREFERENCES_BYTES as usize];
        output.write_all(&chunk).expect("exact write");
        assert!(output.write_all(b"x").is_err());
    }

    #[test]
    fn removed_display_recenters_and_present_display_clamps() {
        let primary = DisplayRect {
            x: 0.0,
            y: 0.0,
            width: 1280.0,
            height: 800.0,
        };
        let secondary = DisplayRect {
            x: 1280.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        };
        let removed = WindowGeometry {
            x: 9000.0,
            y: 9000.0,
            width: 1200.0,
            height: 760.0,
            dpr: 1.0,
            maximized: true,
        };
        let centered = clamp_geometry(Some(removed), &[primary, secondary]);
        assert!(
            centered.x >= primary.x && centered.x + centered.width <= primary.x + primary.width
        );
        assert!(
            centered.y >= primary.y && centered.y + centered.height <= primary.y + primary.height
        );
        assert!(centered.maximized);
        let oversized = WindowGeometry {
            x: 1800.0,
            y: 40.0,
            width: 1600.0,
            height: 760.0,
            dpr: 1.0,
            maximized: false,
        };
        let clamped = clamp_geometry(Some(oversized), &[primary, secondary]);
        assert!(clamped.x >= secondary.x);
        assert!(clamped.x + clamped.width <= secondary.x + secondary.width);
        let too_wide = WindowGeometry {
            x: 9000.0,
            y: 9000.0,
            width: 1600.0,
            height: 760.0,
            dpr: 1.0,
            maximized: true,
        };
        let fitted = clamp_geometry(Some(too_wide), &[primary]);
        assert_eq!(fitted.width, primary.width);
        assert_eq!(fitted.height, 760.0);
        assert!(fitted.x + fitted.width <= primary.x + primary.width);
        assert!(fitted.y + fitted.height <= primary.y + primary.height);
        assert!(fitted.maximized);
        let tiny = DisplayRect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        };
        let in_tiny = clamp_geometry(Some(too_wide), &[tiny]);
        assert_eq!((in_tiny.width, in_tiny.height), (800.0, 600.0));
        assert_eq!((in_tiny.x, in_tiny.y), (0.0, 0.0));
        let left = DisplayRect {
            x: -1920.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        };
        let negative = clamp_geometry(
            Some(WindowGeometry {
                x: -3000.0,
                y: 900.0,
                width: 1280.0,
                height: 800.0,
                dpr: 1.0,
                maximized: false,
            }),
            &[primary, left],
        );
        assert!(negative.x >= left.x && negative.x + negative.width <= left.x + left.width);
        assert!(negative.y >= left.y && negative.y + negative.height <= left.y + left.height);
        let no_displays = clamp_geometry(Some(too_wide), &[]);
        assert!(no_displays.width.is_finite() && no_displays.height.is_finite());
        assert!(!no_displays.maximized);
    }
}
