//! Explicit Fixture-only technical probe; not a product import command or format-support promise.
//! Run: cargo run --locked -p cos-desktop --example media_picker -- --fixture-root DIR
//! Only direct regular files in that operator-supplied local fixture directory may be opened.
use desktop_protocol::RepresentationKind;
use gpui::{
    App, AppContext, Bounds, Context, FocusHandle, IntoElement, PathPromptOptions, Render, Role,
    SharedString, Task, TitlebarOptions, Window, WindowBounds, WindowOptions, div, prelude::*, px,
    rgb, size,
};
use media::{
    Acquisition, Cancellation, DecodePolicy, LocalId, LocalLimits, LocalProvenance, MediaCache,
    MediaError, Orientation, PixelSize, RepresentationKey, RepresentationSource,
};
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
};

#[path = "media_picker/boundary.rs"]
mod boundary;
use boundary::FixtureRoot;
use std::sync::Arc;

struct PickerProbe {
    root: Arc<FixtureRoot>,
    _locked_fixture: Option<File>,
    state: SharedString,
    focus: FocusHandle,
    busy: bool,
    serial: u64,
    cancel: Cancellation,
    task: Option<Task<()>>,
}
impl Drop for PickerProbe {
    fn drop(&mut self) {
        self.cancel.cancel();
        eprintln!("event=probe-drop local-resources-released");
    }
}
impl PickerProbe {
    fn select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.focus.focus(window, cx);
        self.busy = true;
        self.serial += 1;
        self.cancel = Cancellation::default();
        self.state = "Fixture: native picker pending; cancel is not success".into();
        let root = self.root.clone();
        let revision = self.serial;
        let cancel = self.cancel.clone();
        // The focused probe window is the native dialog owner at this exact call.
        let selected = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Select local Fixture".into()),
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let status = match selected.await {
                Ok(Ok(None)) => "Fixture: Cancelled; no file opened, no accepted bytes".to_owned(),
                Ok(Ok(Some(paths))) if paths.len() == 1 => {
                    let path = paths.into_iter().next();
                    match path {
                        Some(path) => {
                            cx.background_executor()
                                .spawn(async move {
                                    match acquire_local(&root, &path, revision, &cancel) {
                                        Ok(status) => status,
                                        Err(error) => format!(
                                            "Fixture: unavailable {error}; no accepted bytes"
                                        ),
                                    }
                                })
                                .await
                        }
                        None => "Fixture: unavailable empty picker result".to_owned(),
                    }
                }
                Ok(Ok(Some(_))) => "Fixture: unavailable multiple selection".to_owned(),
                Ok(Err(_)) | Err(_) => "Fixture: unavailable native picker error".to_owned(),
            };
            eprintln!("event=picker-result {status}");
            let _ = this.update(cx, |this, cx| {
                this.state = status.into();
                this.busy = false;
                cx.notify();
            });
        }));
        cx.notify();
    }
}

fn acquire_local(
    root: &FixtureRoot,
    path: &Path,
    revision: u64,
    cancel: &Cancellation,
) -> Result<String, MediaError> {
    // The actual opened object is validated before any encoded read/cache reservation.
    let file = root.open_selected(path)?;
    let provenance = LocalProvenance::Fixture {
        fixture: LocalId::new("img1-native")?,
        manifest: LocalId::new("operator-fixture-root-v1")?,
        source: LocalId::new("selected-file")?,
    };
    let key = RepresentationKey {
        source: RepresentationSource::Local {
            provenance,
            revision,
        },
        kind: RepresentationKind::Preview,
        size: PixelSize::new(32, 32)?,
        orientation: Orientation::Normal,
        policy: DecodePolicy {
            decoder: LocalId::new("not-decoded")?,
            colour: LocalId::new("not-transformed")?,
            generation: 1,
        },
    };
    let mut cache = MediaCache::new(LocalLimits {
        max_input_bytes: 1024 * 1024,
        max_live_bytes: 1024 * 1024,
        max_live_entries: 1,
        max_pixels: 64 * 1024 * 1024,
    })?;
    let bytes = cache.acquire_file(
        Acquisition {
            key,
            expected: None,
        },
        file,
        cancel,
    )?;
    let status = format!(
        "Fixture: encoded header admitted; {} bytes; {:?}; {}; NO decode / NO Runtime",
        bytes.bytes().len(),
        bytes.verification(),
        bytes.computed_digest()
    );
    cache.clear();
    drop(bytes);
    let usage = cache.usage()?;
    eprintln!(
        "event=encoded-release bytes={} entries={}",
        usage.bytes, usage.entries
    );
    Ok(status)
}

impl Render for PickerProbe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().id("media-probe").role(Role::Application).aria_label("IMG1 Fixture technical feasibility")
            .size_full().p_6().bg(rgb(0x20242a)).text_color(rgb(0xf0f0f0)).flex().flex_col().gap_4()
            .child("IMG1 — Fixture local input / native picker technical probe")
            .child("Not a Product import action. PNG signature only; no image decode, rendering, external acquisition or Runtime.")
            .child("Only direct regular files in the explicit local Fixture directory; limit 1 MiB.")
            .child(div().id("select-fixture").role(Role::Button)
                .aria_label(if self.busy {"Picker/acquisition pending"} else {"Select local Fixture"})
                .track_focus(&self.focus).p_3().border_1().border_color(rgb(0xd9b76b))
                .child(if self.busy {"Picker/acquisition pending"} else {"Select local Fixture"})
                .on_click(cx.listener(|this,_,window,cx|this.select(window,cx))))
            .child(div().id("probe-result").role(Role::Label).aria_label(self.state.clone()).aria_value(self.state.clone())
                .child(self.state.clone()))
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let root = match (args.next().as_deref(), args.next()) {
        (Some("--fixture-root"), Some(root)) => PathBuf::from(root),
        _ => {
            eprintln!("Usage: media_picker --fixture-root DIR [--lock-file NAME]");
            std::process::exit(2);
        }
    };
    let root = match root.canonicalize() {
        Ok(root) if root.is_dir() => root,
        _ => {
            eprintln!("Invalid Fixture directory");
            std::process::exit(2);
        }
    };
    #[cfg(windows)]
    if !matches!(root.components().next(),Some(std::path::Component::Prefix(p)) if matches!(p.kind(),std::path::Prefix::Disk(_)|std::path::Prefix::VerbatimDisk(_)))
    {
        eprintln!("Only an explicit local disk Fixture directory is permitted");
        std::process::exit(2);
    }
    let root = match FixtureRoot::open(&root) {
        Ok(root) => Arc::new(root),
        Err(error) => {
            eprintln!("Fixture root unavailable: {error}");
            std::process::exit(2);
        }
    };
    let lock = match (args.next().as_deref(), args.next()) {
        (None, None) => None,
        (Some("--lock-file"), Some(name))
            if Path::new(&name).components().count() == 1 && !name.contains(['/', '\\']) =>
        {
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                match OpenOptions::new()
                    .read(true)
                    .share_mode(0)
                    .open(root.path().join(name))
                {
                    Ok(file) => Some(file),
                    Err(_) => {
                        eprintln!("Cannot lock owned Fixture input");
                        std::process::exit(2);
                    }
                }
            }
            #[cfg(not(windows))]
            {
                let _ = name;
                eprintln!("Exclusive Fixture lock is Windows-only");
                std::process::exit(2);
            }
        }
        _ => {
            eprintln!("Invalid probe arguments");
            std::process::exit(2);
        }
    };
    if args.next().is_some() {
        eprintln!("Unexpected probe argument");
        std::process::exit(2);
    }
    gpui_platform::application().run(move |cx: &mut App| {
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let bounds = Bounds::centered(None, size(px(980.0), px(430.0)), cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("IMG1 Fixture picker probe".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|cx| PickerProbe {
                    root,
                    _locked_fixture: lock,
                    state: "Fixture: no selection; no accepted bytes".into(),
                    focus: cx.focus_handle().tab_stop(true),
                    busy: false,
                    serial: 0,
                    cancel: Cancellation::default(),
                    task: None,
                })
            },
        );
        if opened.is_err() {
            eprintln!("Native probe window unavailable");
            cx.quit();
        }
        cx.activate(true);
    });
    eprintln!("event=probe-exit Runtime-not-started");
}
