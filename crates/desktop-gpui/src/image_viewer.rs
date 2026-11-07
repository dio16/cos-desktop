//! Local-input MP-IMG5 viewer. Runtime acquisition is deliberately not implemented here.
//! The experimental tile mode is not an R4 adoption/residency claim; production uses an
//! explicitly labelled bounded preview until those evidence gates and MP-IMG6 are satisfied.
use crate::{
    contrast::{ContrastScheme, NORMAL_COLORS, Palette},
    image_upload::{
        ConsumerId, FRAME_EDGE, ImageUploadError, ImageUploadService, PaintStatus, PreparedImage,
        TileDemand, UploadRequest, ViewPaint,
    },
    view_transform::{Point, Rect, Size, ViewCamera, ViewTransform},
};
use gpui::{
    App, Bounds, Context, EventEmitter, FocusHandle, GpuDeviceState, IntoElement, KeyBinding,
    MouseButton, Pixels, Render, Role, SharedString, Subscription, Window, actions, canvas, div,
    fill, point, prelude::*, px, rgb, size,
};
use media::{Cancellation, MediaError, Orientation, RepresentationKey};
use std::sync::Arc;

actions!(
    image_viewer,
    [
        ViewerFit,
        ViewerOne,
        ViewerIn,
        ViewerOut,
        ViewerLeft,
        ViewerRight,
        ViewerUp,
        ViewerDown,
        ViewerFullscreen,
        ViewerEscape
    ]
);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("0", ViewerFit, Some("ImageViewer")),
        KeyBinding::new("1", ViewerOne, Some("ImageViewer")),
        KeyBinding::new("+", ViewerIn, Some("ImageViewer")),
        KeyBinding::new("=", ViewerIn, Some("ImageViewer")),
        KeyBinding::new("-", ViewerOut, Some("ImageViewer")),
        KeyBinding::new("left", ViewerLeft, Some("ImageViewer")),
        KeyBinding::new("right", ViewerRight, Some("ImageViewer")),
        KeyBinding::new("up", ViewerUp, Some("ImageViewer")),
        KeyBinding::new("down", ViewerDown, Some("ImageViewer")),
        KeyBinding::new("f11", ViewerFullscreen, Some("ImageViewer")),
        KeyBinding::new("escape", ViewerEscape, Some("ImageViewer")),
    ]);
}

/// Fast retirement polls run shortly after a release, then one persistent slow cadence applies so
/// a delayed last-detach is still collected without waking an idle window every frame.
const FAST_RETIREMENT_MILLIS: u64 = 100;
const SLOW_RETIREMENT_MILLIS: u64 = 500;
/// Number of fast sweeps before the slow cadence takes over for good.
const FAST_RETIREMENT_SWEEPS: u8 = 8;
/// Bound on consecutive transient tile-preparation retries for one viewport demand.
const TILE_RETRY_LIMIT: u8 = 20;

fn retirement_interval(attempts: u8, slow: bool) -> u64 {
    if !slow && attempts <= FAST_RETIREMENT_SWEEPS {
        FAST_RETIREMENT_MILLIS
    } else {
        SLOW_RETIREMENT_MILLIS
    }
}

fn device_status(state: GpuDeviceState) -> SharedString {
    match state {
        GpuDeviceState::Unsupported => {
            "GPU表示を利用できません: このバックエンドでは画像表示が未対応です。".into()
        }
        GpuDeviceState::Ready { .. } => {
            "画像を利用できません: 認可済みの画像入力がありません。実行基盤からの取得は未対応です。"
                .into()
        }
        GpuDeviceState::Recovering { generation } => {
            format!("GPUデバイスを復旧中です（世代 {generation}）。画像表示を一時停止しています。")
                .into()
        }
        GpuDeviceState::Unavailable { generation } => {
            format!("GPU表示を利用できません（世代 {generation}）。アプリを再起動してください。")
                .into()
        }
    }
}

fn initial_status(state: GpuDeviceState) -> SharedString {
    device_status(state)
}

/// A retired frame may be released only after two composed scenes postdate its detach.
fn scene_allows_release(scene_serial: u64, scene_at: u64) -> bool {
    scene_serial >= scene_at.saturating_add(2)
}

/// Which counter a retired frame's reference value came from. A live pane retires with its own
/// composed-scene count; a pane that left the draw tree retires with the window's composed-frame
/// count. The two counters are never compared with each other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReleaseClock {
    Pane(u64),
    Window(u64),
}

/// A detached frame plus the clock reading observed when it was retired.
struct Retired {
    frame: Arc<PreparedImage>,
    clock: ReleaseClock,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewerMode {
    PreviewOnly,
    ExperimentalTiles,
}

/// Local interaction only. Applying a peer camera does not emit another event.
#[derive(Clone, Copy, Debug)]
pub enum ViewerEvent {
    Camera(ViewCamera),
    Focused,
}

/// Per-pane CPU scene observation. Shared resources do not share this readiness or geometry.
#[derive(Clone, Copy, Debug)]
pub struct ViewerObservation {
    pub complete_in_scene: bool,
    pub scene_serial: u64,
    pub view_serial: u64,
    /// 1 follows the first thumbnail composition; 2 follows a bounded-preview composition.
    pub phase: u8,
    pub dpi: f64,
    pub viewport: Size,
    pub image_bounds: Option<Rect>,
}

/// GPU handles never leave this GPUI-boundary entity. Input must carry explicit local provenance.
pub struct ImageViewer {
    service: ImageUploadService,
    current: Option<Arc<PreparedImage>>,
    /// One bounded retained CPU source frame used for generation-specific recovery.
    retained: Option<Arc<PreparedImage>>,
    /// The exact identity this pane's host asked to present. Recovery may re-admit only this
    /// identity: a backup for a superseded or withheld candidate is dropped, never resurrected.
    requested: Option<(RepresentationKey, u64)>,
    cancel: Cancellation,
    /// Cancellation owned by the current viewport tile demand.
    tile_cancel: Cancellation,
    demand: Option<TileDemand>,
    demand_id: u64,
    tile_attempts: u8,
    /// Frames detached from this viewer but still reachable from the last presented scene.
    retiring: Vec<Retired>,
    /// Paint epochs observed for the current frame; a retirement needs a later composed scene.
    scene_serial: u64,
    complete_in_scene: bool,
    retirement_scheduled: bool,
    retirement_attempts: u8,
    retirement_slow: bool,
    serial: u64,
    view_serial: u64,
    revision: u64,
    generation: u64,
    device_state: GpuDeviceState,
    recovery_pending: bool,
    recovery_attempted_generation: Option<u64>,
    _device_subscription: Subscription,
    base_window_title: String,
    transform: Option<ViewTransform>,
    camera: ViewCamera,
    compare_pane: Option<usize>,
    dpi: f64,
    /// Handle used by the deferred retirement sweep, which must also run while not rendered.
    window: gpui::AnyWindowHandle,
    viewport: Bounds<Pixels>,
    focus: FocusHandle,
    drag: Option<gpui::Point<Pixels>>,
    mode: ViewerMode,
    phase: u8,
    status: SharedString,
    palette: Palette,
    consumer: ConsumerId,
}

impl ImageViewer {
    pub fn new(
        mode: ViewerMode,
        base_window_title: impl Into<String>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let device_state = cx.gpu_device_state();
        let base_window_title = base_window_title.into();
        let window = window.window_handle();
        let weak = cx.entity().downgrade();
        let device_subscription = cx.on_gpu_device_state_change(move |app| {
            let state = app.gpu_device_state();
            let _ = window.update(app, |_, window, app| {
                let _ = weak.update(app, |viewer, cx| {
                    viewer.device_state_changed(state, window, cx);
                });
            });
        });
        Self {
            service: ImageUploadService::global(),
            current: None,
            retained: None,
            requested: None,
            cancel: Cancellation::default(),
            tile_cancel: Cancellation::default(),
            demand: None,
            demand_id: 0,
            tile_attempts: 0,
            retiring: Vec::new(),
            scene_serial: 0,
            retirement_scheduled: false,
            retirement_attempts: 0,
            retirement_slow: false,
            serial: 0,
            view_serial: 0,
            revision: 0,
            generation: 0,
            device_state,
            recovery_pending: false,
            recovery_attempted_generation: None,
            _device_subscription: device_subscription,
            base_window_title,
            transform: None,
            camera: ViewCamera::default(),
            compare_pane: None,
            complete_in_scene: false,
            dpi: 0.0,
            window,
            viewport: Bounds::default(),
            focus: cx.focus_handle().tab_stop(true),
            drag: None,
            mode,
            phase: 0,
            status: initial_status(device_state),
            palette: Palette {
                scheme: ContrastScheme::Normal,
                colors: NORMAL_COLORS,
            },
            consumer: ConsumerId::allocate(),
        }
    }

    pub fn device_state(&self) -> GpuDeviceState {
        self.device_state
    }

    pub fn retained_prepared(&self) -> Option<Arc<PreparedImage>> {
        self.retained.clone()
    }

    /// Drop the recovery backup without claiming anything about the picture on screen.
    ///
    /// The host calls this when the candidate the pane represents is replaced or withheld: the
    /// identity the backup belongs to is no longer the one this pane may publish, so a later GPU
    /// generation change must not re-admit it. The CPU owner is released with it, otherwise a
    /// withdrawn candidate would keep occupying the retention ceiling forever.
    pub fn withdraw_recovery_backup(&mut self, cx: &mut Context<Self>) {
        if self.retained.is_none() && self.requested.is_none() {
            return;
        }
        let origin = self.retained.as_ref().map(|frame| frame.origin());
        self.retained = None;
        self.requested = None;
        self.recovery_pending = false;
        self.recovery_attempted_generation = None;
        if let Some(origin) = origin {
            self.release_backup(origin);
        }
        cx.notify();
    }

    /// Release the CPU owner of an origin this pane has stopped referencing.
    fn release_backup(&mut self, origin: u64) {
        let still_referenced = self
            .current
            .as_ref()
            .is_some_and(|frame| frame.origin() == origin)
            || self
                .retained
                .as_ref()
                .is_some_and(|frame| frame.origin() == origin)
            || self
                .retiring
                .iter()
                .any(|retired| retired.frame.origin() == origin);
        if !still_referenced {
            self.service.release_origin(origin);
        }
    }

    fn device_state_changed(
        &mut self,
        state: GpuDeviceState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.device_state == state {
            return;
        }
        let previous_generation = match self.device_state {
            GpuDeviceState::Ready { generation }
            | GpuDeviceState::Recovering { generation }
            | GpuDeviceState::Unavailable { generation } => Some(generation),
            GpuDeviceState::Unsupported => None,
        };
        let next_generation = match state {
            GpuDeviceState::Ready { generation }
            | GpuDeviceState::Recovering { generation }
            | GpuDeviceState::Unavailable { generation } => Some(generation),
            GpuDeviceState::Unsupported => None,
        };
        let invalidated = next_generation.is_some_and(|generation| {
            let advanced = previous_generation.is_none_or(|previous| generation > previous);
            if advanced {
                self.service.invalidate_device_generation(generation);
            }
            advanced
        });

        self.device_state = state;
        self.cancel.cancel();
        self.tile_cancel.cancel();
        self.cancel = Cancellation::default();
        self.tile_cancel = Cancellation::default();
        self.serial = self.serial.saturating_add(1);
        self.view_serial = self.view_serial.saturating_add(1);
        self.demand = None;
        self.tile_attempts = 0;
        self.recovery_pending = false;
        self.recovery_attempted_generation = None;
        self.complete_in_scene = false;
        self.drag = None;

        if invalidated {
            if let Some(frame) = self.current.take() {
                self.service.detach(&frame, self.consumer);
            }
        } else {
            self.release_current(cx);
        }

        // A pane embedded in a host window must not overwrite the host's native title; an empty
        // base title means "the embedding window owns the title".
        let title = |suffix: &str| match self.base_window_title.is_empty() {
            true => None,
            false => Some(format!("{}{suffix}", self.base_window_title)),
        };
        match state {
            GpuDeviceState::Ready { .. } => {
                if let Some(title) = title("") {
                    window.set_window_title(&title);
                }
                self.status = if self.retained.is_some() {
                    "GPUは利用可能です。保持済みの画像を次の表示要求時に復元します。".into()
                } else {
                    device_status(state)
                };
            }
            GpuDeviceState::Recovering { .. } => {
                if let Some(title) = title(" — GPU復旧中") {
                    window.set_window_title(&title);
                }
                self.status = device_status(state);
            }
            GpuDeviceState::Unavailable { .. } => {
                if let Some(title) = title(" — GPU表示不可: アプリを再起動してください")
                {
                    window.set_window_title(&title);
                }
                self.status = device_status(state);
            }
            GpuDeviceState::Unsupported => {
                if let Some(title) = title(" — GPUバックエンド未対応") {
                    window.set_window_title(&title);
                }
                self.status = device_status(state);
            }
        }
        cx.notify();
    }

    /// The shell owns the OS probe; child viewers consume its already-resolved palette.
    pub fn set_palette(&mut self, palette: Palette, cx: &mut Context<Self>) {
        if self.palette != palette {
            self.palette = palette;
            cx.notify();
        }
    }

    /// Compare owns its action row; each pane contributes only one keyboard stop.
    pub fn set_compare_pane(&mut self, index: usize, cx: &mut Context<Self>) {
        self.compare_pane = Some(index);
        self.focus = self.focus.clone().tab_index(10 + index as isize);
        cx.notify();
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub fn camera(&self) -> ViewCamera {
        self.camera
    }

    /// Apply the shared ideal camera without copying a peer's display-clamped center.
    pub fn set_camera(&mut self, camera: ViewCamera, cx: &mut Context<Self>) {
        if self.camera == camera {
            return;
        }
        if let Some(transform) = self.transform.as_mut()
            && transform.set_camera(camera).is_err()
        {
            return;
        }
        self.camera = camera;
        self.complete_in_scene = false;
        self.view_serial = self.view_serial.saturating_add(1);
        self.tile_cancel.cancel();
        self.demand = None;
        self.tile_attempts = 0;
        cx.notify();
    }

    pub fn prepared(&self) -> Option<Arc<PreparedImage>> {
        self.current.clone()
    }

    pub fn observation(&self) -> ViewerObservation {
        ViewerObservation {
            complete_in_scene: self.complete_in_scene,
            scene_serial: self.scene_serial,
            view_serial: self.view_serial,
            phase: self.phase,
            dpi: self.dpi,
            viewport: Size {
                width: f32::from(self.viewport.size.width) as f64,
                height: f32::from(self.viewport.size.height) as f64,
            },
            image_bounds: self
                .current
                .as_ref()
                .and(self.transform.as_ref())
                .map(ViewTransform::image_bounds),
        }
    }

    /// Attach an explicitly prepared exact resource; no source lookup or second decode occurs.
    /// The pane camera and focus survive candidate Swap.
    pub fn present_prepared(
        &mut self,
        frame: Arc<PreparedImage>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let state = window.gpu_device_state();
        if state != self.device_state {
            self.device_state_changed(state, window, cx);
        }
        let GpuDeviceState::Ready { generation } = state else {
            self.unavailable(device_status(state), cx);
            return false;
        };
        if frame.identity().device_generation != generation {
            self.unavailable(
                "画像を利用できません: 画像は現在のGPUデバイス世代に属していません。".into(),
                cx,
            );
            return false;
        }
        if frame.identity().key.orientation != Orientation::Normal {
            self.unavailable(
                "画像を利用できません: この経路では標準の向きだけが検証済みです。".into(),
                cx,
            );
            return false;
        }
        if self
            .current
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &frame))
        {
            return true;
        }
        if !self
            .service
            .attach(&frame, self.consumer, Some(window.window_handle()))
        {
            self.unavailable(
                "画像を利用できません: 保持または表示先の所有権を確認できません。".into(),
                cx,
            );
            return false;
        }
        self.retiring
            .retain(|retired| !Arc::ptr_eq(&retired.frame, &frame));
        self.cancel.cancel();
        self.tile_cancel.cancel();
        self.serial = self.serial.saturating_add(1);
        self.release_current(cx);
        self.revision = frame.identity().source_revision;
        self.generation = frame.identity().device_generation;
        self.phase = 0;
        self.demand = None;
        self.tile_attempts = 0;
        self.cancel = Cancellation::default();
        let preview = self.mode == ViewerMode::PreviewOnly || frame.minimum_mip() > 0;
        let previous_origin = self.retained.as_ref().map(|retained| retained.origin());
        self.retained = Some(Arc::clone(&frame));
        if let Some(previous) = previous_origin.filter(|previous| *previous != frame.origin()) {
            self.release_backup(previous);
        }
        self.requested = Some((
            frame.identity().key.clone(),
            frame.identity().source_revision,
        ));
        self.current = Some(frame);
        self.status = match preview {
            true => "ローカル画像: 低解像度プレビュー。原寸の詳細表示・1:1ではありません。",
            false => "ローカル画像・実験用タイル表示。GPU完了・常駐量・本番の画質受入は未計測。",
        }
        .into();
        cx.notify();
        true
    }

    /// Restore a retained candidate after a surface switch or device transition. Stale-generation
    /// frames are CPU recovery sources only; a new GPU-tagged frame is created lazily when visible.
    pub fn restore_prepared(
        &mut self,
        frame: Arc<PreparedImage>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let state = window.gpu_device_state();
        if state != self.device_state {
            self.device_state_changed(state, window, cx);
        }
        if frame.identity().key.orientation != Orientation::Normal {
            self.unavailable(
                "画像を利用できません: この経路では標準の向きだけが検証済みです。".into(),
                cx,
            );
            return false;
        }
        if matches!(state, GpuDeviceState::Ready { generation } if generation == frame.identity().device_generation)
        {
            return self.present_prepared(frame, window, cx);
        }
        self.cancel.cancel();
        self.tile_cancel.cancel();
        self.cancel = Cancellation::default();
        self.tile_cancel = Cancellation::default();
        self.serial = self.serial.saturating_add(1);
        self.release_current(cx);
        let previous_origin = self.retained.as_ref().map(|retained| retained.origin());
        self.retained = Some(Arc::clone(&frame));
        if let Some(previous) = previous_origin.filter(|previous| *previous != frame.origin()) {
            self.release_backup(previous);
        }
        self.requested = Some((
            frame.identity().key.clone(),
            frame.identity().source_revision,
        ));
        self.revision = frame.identity().source_revision;
        self.generation = frame.identity().device_generation;
        self.recovery_pending = false;
        self.recovery_attempted_generation = None;
        self.demand = None;
        self.tile_attempts = 0;
        self.phase = 0;
        self.complete_in_scene = false;
        self.status = match state {
            GpuDeviceState::Ready { .. } => {
                "保持済みのCPU画像を現在のGPU世代に再結合する準備をしています。".into()
            }
            _ => device_status(state),
        };
        cx.notify();
        true
    }

    pub fn unavailable(&mut self, reason: SharedString, cx: &mut Context<Self>) {
        self.cancel.cancel();
        self.tile_cancel.cancel();
        self.serial = self.serial.saturating_add(1);
        self.recovery_pending = false;
        self.recovery_attempted_generation = None;
        let origin = self.retained.as_ref().map(|frame| frame.origin());
        self.release_current(cx);
        self.retained = None;
        // The host withdrew this pane's picture: no recovery backup may resurrect it later.
        self.requested = None;
        if let Some(origin) = origin {
            self.release_backup(origin);
        }
        self.demand = None;
        self.status = reason;
        cx.notify();
    }

    pub fn supports_one_to_one(&self) -> bool {
        self.mode == ViewerMode::ExperimentalTiles
            && self
                .current
                .as_ref()
                .is_some_and(|frame| frame.minimum_mip() == 0)
    }

    pub fn physical_one_to_one(&mut self, cx: &mut Context<Self>) {
        self.one(cx);
    }

    pub fn zoom_by(&mut self, factor: f64, cursor: Option<Point>, cx: &mut Context<Self>) {
        self.zoom(factor, cursor, cx);
    }

    pub fn pan_by(&mut self, delta: Point, cx: &mut Context<Self>) {
        self.pan(delta.x, delta.y, cx);
    }

    /// Open a caller-supplied generated/fixture input off-thread, never a Runtime artifact substitute.
    pub fn load_generated(
        &mut self,
        request: UploadRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
        source: impl FnOnce(&Cancellation) -> Result<Arc<[u8]>, MediaError> + Send + 'static,
    ) {
        let state = window.gpu_device_state();
        if state != self.device_state {
            self.device_state_changed(state, window, cx);
        }
        let GpuDeviceState::Ready { generation } = state else {
            self.unavailable(device_status(state), cx);
            return;
        };
        if request.device_generation != generation
            || request.current_device_generation != generation
        {
            self.unavailable(
                format!(
                    "画像を利用できません: 要求されたGPU世代 {} は現在の世代 {} と一致しません。",
                    request.device_generation, generation
                )
                .into(),
                cx,
            );
            return;
        }
        if let Some(frame) = self.retained.clone().filter(|frame| {
            frame.identity().key == request.key
                && frame.identity().source_revision == request.source_revision
        }) {
            self.restore_prepared(frame, window, cx);
            return;
        }
        self.cancel.cancel();
        self.tile_cancel.cancel();
        self.demand = None;
        self.tile_attempts = 0;
        self.serial = self.serial.saturating_add(1);
        self.recovery_pending = false;
        self.recovery_attempted_generation = None;
        self.release_current(cx);
        self.retained = None;
        self.revision = request.source_revision;
        self.generation = generation;
        self.phase = 0;
        self.transform = None;
        self.camera = ViewCamera::default();
        self.cancel = Cancellation::default();
        if request.key.orientation != Orientation::Normal {
            self.status =
                "画像を利用できません: このアップロード経路では標準の向きだけが検証済みです。"
                    .into();
            cx.notify();
            return;
        }
        // Only this explicit host request owns publication; a later generation change may
        // re-admit this identity and nothing else.
        self.requested = Some((request.key.clone(), request.source_revision));
        self.status = "ローカル画像を準備中（実行基盤の成果物ではありません）".into();
        let serial = self.serial;
        let service = self.service.clone();
        let cleanup = service.clone();
        let task = service.prepare_view_generated(
            cx.background_executor(),
            request,
            self.cancel.clone(),
            self.mode == ViewerMode::PreviewOnly,
            source,
        );
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let mut unclaimed = result.as_ref().ok().cloned();
            let _ = this.update_in(cx, |this, window, cx| {
                if serial != this.serial
                    || window.gpu_device_state() != (GpuDeviceState::Ready { generation })
                    || this.device_state != (GpuDeviceState::Ready { generation })
                {
                    return;
                }
                match result {
                    Ok(frame)
                        if this.service.attach(
                            &frame,
                            this.consumer,
                            Some(window.window_handle()),
                        ) =>
                    {
                        this.retained = Some(Arc::clone(&frame));
                        this.current = Some(frame);
                        unclaimed = None;
                        this.status = match this.mode {
                            ViewerMode::PreviewOnly => "ローカル画像: 低解像度プレビュー。原寸の詳細表示・1:1ではありません。",
                            ViewerMode::ExperimentalTiles => "ローカル画像・実験用タイル表示。GPU完了・常駐量・本番の画質受入は未計測。",
                        }
                        .into();
                    }
                    Ok(_) => this.status = "画像を利用できません: 保持されていない画像です。".into(),
                    Err(error) => {
                        this.status = format!("画像を利用できません: {}", error.limitation()).into();
                    }
                }
                cx.notify();
            });
            if let Some(frame) = unclaimed {
                cleanup.discard_unpainted(&frame);
            }
        })
        .detach();
        cx.notify();
    }

    /// Re-admit a retained CPU source only when this viewer is actually rendered.
    fn ensure_recovery(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = window.gpu_device_state();
        if state != self.device_state {
            self.device_state_changed(state, window, cx);
        }
        let GpuDeviceState::Ready { generation } = state else {
            return;
        };
        if self.current.is_some() || self.recovery_pending {
            return;
        }
        let Some(previous) = self.retained.clone() else {
            return;
        };
        // Only the identity this pane's host last asked for may be published again. A backup
        // left from a superseded selection, a replaced revision, a different decoder policy, or
        // a withheld candidate is dropped here instead of being resurrected as the picture.
        match self.requested.as_ref() {
            Some((key, revision))
                if *key == previous.identity().key
                    && *revision == previous.identity().source_revision => {}
            _ => {
                self.retained = None;
                self.recovery_pending = false;
                self.recovery_attempted_generation = None;
                return;
            }
        }
        if previous.identity().device_generation == generation
            && self
                .service
                .attach(&previous, self.consumer, Some(window.window_handle()))
        {
            self.current = Some(Arc::clone(&previous));
            self.revision = previous.identity().source_revision;
            self.generation = generation;
            self.phase = 0;
            self.status = match self.mode {
                ViewerMode::PreviewOnly => {
                    "ローカル画像: 低解像度プレビュー。原寸の詳細表示・1:1ではありません。"
                }
                ViewerMode::ExperimentalTiles => {
                    "ローカル画像・実験用タイル表示。GPU完了・常駐量・本番の画質受入は未計測。"
                }
            }
            .into();
            cx.notify();
            return;
        }
        if self.recovery_attempted_generation == Some(generation) {
            return;
        }
        self.recovery_attempted_generation = Some(generation);
        self.recovery_pending = true;
        let serial = self.serial;
        let source_revision = previous.identity().source_revision;
        let source_key = previous.identity().key.clone();
        let cancel = self.cancel.clone();
        let service = self.service.clone();
        let cleanup = service.clone();
        let worker_service = service.clone();
        let recovery_source = Arc::clone(&previous);
        let task = cx.background_executor().spawn(async move {
            worker_service.recover_frame(&recovery_source, generation, &cancel)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let mut unclaimed = result.as_ref().ok().cloned();
            let _ = this.update_in(cx, |this, window, cx| {
                this.recovery_pending = false;
                if serial != this.serial
                    || window.gpu_device_state() != (GpuDeviceState::Ready { generation })
                    || this.device_state != (GpuDeviceState::Ready { generation })
                    || !this
                        .retained
                        .as_ref()
                        .is_some_and(|retained| Arc::ptr_eq(retained, &previous))
                    || this.requested.as_ref()
                        != Some(&(source_key.clone(), source_revision))
                {
                    return;
                }
                match result {
                    Ok(frame)
                        if frame.identity().key == source_key
                            && frame.identity().source_revision == source_revision
                            && frame.identity().device_generation == generation
                            && this.service.attach(
                                &frame,
                                this.consumer,
                                Some(window.window_handle()),
                            ) =>
                    {
                        this.retained = Some(Arc::clone(&frame));
                        this.current = Some(frame);
                        this.revision = source_revision;
                        this.generation = generation;
                        this.phase = 0;
                        unclaimed = None;
                        this.status = match this.mode {
                            ViewerMode::PreviewOnly => {
                                "ローカル画像: 低解像度プレビュー。原寸の詳細表示・1:1ではありません。"
                            }
                            ViewerMode::ExperimentalTiles => {
                                "ローカル画像・実験用タイル表示。GPU完了・常駐量・本番の画質受入は未計測。"
                            }
                        }
                        .into();
                    }
                    Ok(_) => {
                        this.status =
                            "画像を利用できません: 保持された入力を現在のGPU世代に再結合できません。"
                                .into();
                    }
                    Err(error) => {
                        this.status =
                            format!("画像を利用できません: {}", error.limitation()).into();
                    }
                }
                cx.notify();
            });
            if let Some(frame) = unclaimed {
                cleanup.discard_unpainted(&frame);
            }
        })
        .detach();
    }

    /// Call before removing the owning view/window. A painted payload is never released here: the
    /// scene that referenced it may still be presented and there is no completion signal.
    fn window_id(&self) -> u64 {
        self.window.window_id().as_u64()
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.complete_in_scene = false;
        self.cancel.cancel();
        self.tile_cancel.cancel();
        self.cancel = Cancellation::default();
        self.tile_cancel = Cancellation::default();
        self.demand = None;
        self.serial = self.serial.saturating_add(1);
        self.recovery_pending = false;
        self.recovery_attempted_generation = None;
        self.requested = None;
        let retained_origin = self.retained.as_ref().map(|frame| frame.origin());
        self.retained = None;
        let window_scenes = self.service.composed_frames(self.window_id());
        for retired in &mut self.retiring {
            if let ReleaseClock::Pane(_) = retired.clock {
                retired.clock = ReleaseClock::Window(window_scenes);
            }
        }
        let Some(frame) = self.current.take() else {
            if let Some(origin) = retained_origin {
                self.release_backup(origin);
            }
            self.schedule_retirement(cx);
            return;
        };
        self.service.detach(&frame, self.consumer);
        if let Some(origin) = retained_origin {
            self.release_backup(origin);
        }
        if !self.service.discard_unpainted(&frame)
            && !self
                .retiring
                .iter()
                .any(|retired| Arc::ptr_eq(&retired.frame, &frame))
        {
            self.retiring.push(Retired {
                frame,
                clock: ReleaseClock::Window(self.service.composed_frames(self.window_id())),
            });
            self.schedule_retirement(cx);
        }
    }

    /// Detach at once so no later scene adds these tiles, then drop the atlas entries only after
    /// two presented frames can no longer reference them. A frame another consumer still presents
    /// stays retired and is retried by the next sweep.
    fn release_current(&mut self, cx: &mut Context<Self>) {
        self.complete_in_scene = false;
        let Some(frame) = self.current.take() else {
            return;
        };
        self.service.detach(&frame, self.consumer);
        if !self
            .retiring
            .iter()
            .any(|retired| Arc::ptr_eq(&retired.frame, &frame))
        {
            self.retiring.push(Retired {
                frame,
                clock: ReleaseClock::Pane(self.scene_serial),
            });
        }
        self.schedule_retirement(cx);
    }

    /// Wait on a background timer, never on a chain of frame callbacks: an idle window must not be
    /// woken every frame just to poll a frame it cannot evict yet. The sweep uses this view's
    /// window handle, so it also runs while the Artifact surface is not being rendered.
    fn schedule_retirement(&mut self, cx: &mut Context<Self>) {
        if self.retirement_scheduled || self.retiring.is_empty() {
            return;
        }
        self.retirement_scheduled = true;
        let interval = retirement_interval(self.retirement_attempts, self.retirement_slow);
        let window = self.window;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(interval))
                .await;
            let _ = cx.update(|app| {
                window.update(app, |_, window, app| {
                    let _ = this.update(app, |this, cx| {
                        this.retirement_scheduled = false;
                        if this.retiring.is_empty() {
                            this.retirement_attempts = 0;
                            this.retirement_slow = false;
                            return;
                        }
                        this.evict_retired(window, cx);
                    });
                })
            });
        })
        .detach();
    }

    fn evict_retired(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut keep = Vec::new();
        let window_scenes = self.service.composed_frames(self.window_id());
        for retired in std::mem::take(&mut self.retiring) {
            // Each entry is compared only with the counter that produced its reading: a live pane
            // with its own composed scenes, a pane that left the draw tree with the window's.
            // Two later scenes prove the frame cannot appear in what is presented next; a hidden
            // window composes nothing, so its release waits for re-display.
            let released = match retired.clock {
                ReleaseClock::Pane(scene_at) => scene_allows_release(self.scene_serial, scene_at),
                ReleaseClock::Window(frame_at) => scene_allows_release(window_scenes, frame_at),
            };
            if !released {
                keep.push(retired);
                continue;
            }
            match self.service.evict(&retired.frame, window) {
                Ok(_) | Err(ImageUploadError::NotRetained) => {}
                Err(_) => {
                    // Another consumer still presents it, or release is not provable yet. The
                    // retained frame stays owned and charged and is retried on the next sweep;
                    // release diagnostics belong to the service counters, not to the picture's
                    // own status line (ADR-0015 Q14).
                    keep.push(retired);
                }
            }
        }
        self.retiring = keep;
        if self.retiring.is_empty() {
            self.retirement_attempts = 0;
            self.retirement_slow = false;
            return;
        }
        // This viewer keeps the release responsibility for as long as it lives: a fast sweep while
        // the conflict is likely to clear, then one persistent slow cadence so a later last-detach
        // is still collected. The window close sweep is the last boundary.
        self.retirement_attempts = self.retirement_attempts.saturating_add(1);
        if self.retirement_attempts > FAST_RETIREMENT_SWEEPS {
            self.retirement_slow = true;
        }
        self.schedule_retirement(cx);
    }

    /// Prepare only the tiles this viewport needs; a newer viewport supersedes the demand.
    ///
    /// Returns whether new work was started. A still-missing set after the retry budget is
    /// advanced by a one-second timer, never by repainting the frame.
    fn request_tiles(&mut self, demand: TileDemand, cx: &mut Context<Self>) -> bool {
        let Some(frame) = self.current.clone() else {
            return false;
        };
        if self.demand.as_ref() == Some(&demand) {
            return false;
        }
        self.demand = Some(demand.clone());
        self.demand_id = self.demand_id.wrapping_add(1);
        let demand_id = self.demand_id;
        self.tile_cancel.cancel();
        self.tile_cancel = Cancellation::default();
        let (serial, view_serial) = (self.serial, self.view_serial);
        let task = self.service.prepare_visible(
            cx.background_executor(),
            frame,
            demand,
            self.tile_cancel.clone(),
        );
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            let busy = matches!(outcome, Err(ImageUploadError::PreparationBusy));
            if busy {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
                    .await;
            }
            let mut limit = false;
            let _ = this.update(cx, |this, cx| {
                if (this.serial, this.view_serial, this.demand_id)
                    != (serial, view_serial, demand_id)
                {
                    return;
                }
                match outcome {
                    Ok(()) => {
                        this.demand = None;
                        this.tile_attempts = 0;
                        cx.notify();
                    }
                    Err(ImageUploadError::Cancelled) => {}
                    Err(ImageUploadError::PreparationBusy) => {
                        this.tile_attempts = this.tile_attempts.saturating_add(1);
                        if this.tile_attempts > TILE_RETRY_LIMIT {
                            this.status =
                                "画像の準備が混み合っています。自動で再試行します。".into();
                            limit = true;
                            cx.notify();
                        } else {
                            this.demand = None;
                            cx.notify();
                        }
                    }
                    // Keep the demand so an unchanged viewport does not retry every frame.
                    Err(error) => {
                        this.status = format!("画像の表示を制限: {}", error.limitation()).into();
                        cx.notify();
                    }
                }
            });
            if limit {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(1))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if (this.serial, this.view_serial, this.demand_id)
                        != (serial, view_serial, demand_id)
                    {
                        return;
                    }
                    this.tile_attempts = 0;
                    this.demand = None;
                    cx.notify();
                });
            }
        })
        .detach();
        true
    }

    fn change(&mut self, operation: impl FnOnce(&mut ViewTransform), cx: &mut Context<Self>) {
        self.complete_in_scene = false;
        self.view_serial = self.view_serial.saturating_add(1);
        self.tile_cancel.cancel();
        self.demand = None;
        self.tile_attempts = 0;
        if let Some(transform) = self.transform.as_mut() {
            operation(transform);
            self.camera = transform.camera();
            cx.emit(ViewerEvent::Camera(self.camera));
        }
        cx.notify();
    }
    fn zoom(&mut self, factor: f64, cursor: Option<Point>, cx: &mut Context<Self>) {
        self.change(
            |transform| {
                let viewport = transform.viewport();
                let cursor = cursor.unwrap_or(Point {
                    x: viewport.width / 2.0,
                    y: viewport.height / 2.0,
                });
                let _ = transform.zoom_at(factor, cursor);
            },
            cx,
        );
    }
    fn pan(&mut self, x: f64, y: f64, cx: &mut Context<Self>) {
        self.change(
            |transform| {
                let _ = transform.pan(Point { x, y });
            },
            cx,
        );
    }
    fn one(&mut self, cx: &mut Context<Self>) {
        if self.supports_one_to_one() {
            self.change(ViewTransform::physical_one_to_one, cx);
        }
    }
    fn fullscreen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.toggle_fullscreen();
        self.focus.focus(window, cx);
        cx.notify();
    }
}

impl Drop for ImageViewer {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.tile_cancel.cancel();
        // If the host omitted close, keep the registry/charged bytes; no completion is invented.
        for frame in self
            .current
            .iter()
            .chain(self.retiring.iter().map(|retired| &retired.frame))
        {
            self.service.detach(frame, self.consumer);
        }
    }
}

impl EventEmitter<ViewerEvent> for ImageViewer {}

impl Render for ImageViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_recovery(window, cx);
        let palette = self.palette;
        let preview = self.mode == ViewerMode::PreviewOnly
            || self
                .current
                .as_ref()
                .is_some_and(|frame| frame.minimum_mip() > 0);
        let description: SharedString =
            match (self.current.as_ref(), preview, self.transform.as_ref()) {
                (None, _, _) => "画像は未表示です".into(),
                (_, true, _) => "低解像度プレビュー（原寸・1:1ではありません）".into(),
                (_, false, Some(transform)) => {
                    let bounds = transform.image_bounds();
                    let viewport = transform.viewport();
                    let coverage = if bounds.width > viewport.width + 1.0e-6
                        || bounds.height > viewport.height + 1.0e-6
                    {
                        "画像の一部を切り抜いて表示"
                    } else {
                        "画像全体を表示"
                    };
                    format!(
                        "物理表示倍率 {:.1}% / DIP変換 {:.2} / {coverage}",
                        transform.effective_physical_scale() * 100.0,
                        window.scale_factor()
                    )
                    .into()
                }
                _ => "画像は未表示です".into(),
            };
        let frame = self.current.clone();
        let cancel = self.cancel.clone();
        let revision = self.revision;
        let generation = self.generation;
        let geometry = self.transform.clone();
        let camera = self.camera;
        let phase = self.phase;
        let pending_release = !self.retiring.is_empty();
        let entity = cx.entity().downgrade();
        let demand = (self.serial, self.view_serial);
        let previous_viewport = self.viewport;
        let previous_status = self.status.clone();
        let frame_canvas = canvas(
            |bounds, _, _| bounds,
            move |bounds, _, window, app| {
                window.paint_quad(fill(bounds, rgb(palette.colors.background)));
                let viewport = Size {
                    width: f32::from(bounds.size.width) as f64,
                    height: f32::from(bounds.size.height) as f64,
                };
                let dpi = f64::from(window.scale_factor());
                let mut transform = match geometry.clone() {
                    Some(transform) => transform,
                    None => match ViewTransform::new(
                        Size {
                            width: FRAME_EDGE as f64,
                            height: FRAME_EDGE as f64,
                        },
                        viewport,
                        dpi,
                    ) {
                        Ok(transform) => transform,
                        Err(_) => return,
                    },
                };
                if transform.set_viewport(viewport, dpi).is_err() {
                    return;
                }
                if transform.set_camera(camera).is_err() {
                    return;
                }
                let image = transform.image_bounds();
                let image_bounds = Bounds::new(
                    point(
                        bounds.origin.x + px(image.x as f32),
                        bounds.origin.y + px(image.y as f32),
                    ),
                    size(px(image.width as f32), px(image.height as f32)),
                );
                let mip = if phase == 0 {
                    4
                } else if phase == 1 || preview {
                    transform.mip_level().max(2)
                } else {
                    transform.mip_level()
                };
                let paint_ready = matches!(
                    window.gpu_device_state(),
                    GpuDeviceState::Ready { generation: active_generation }
                        if active_generation == generation
                );
                let needed = if paint_ready {
                    frame
                        .as_ref()
                        .map(|frame| frame.demand(image_bounds, bounds, mip.min(4) as u8))
                } else {
                    None
                };
                let result = if paint_ready {
                    frame.as_ref().map(|frame| {
                        frame.paint_view(
                            ViewPaint {
                                image_bounds,
                                viewport: bounds,
                                mip_level: mip.min(4) as u8,
                                source_revision: revision,
                                device_generation: generation,
                                cancel: &cancel,
                            },
                            window,
                        )
                    })
                } else {
                    None
                };
                let complete = matches!(result, Some(Ok(PaintStatus::CompleteInScene { .. })));
                let missing = matches!(result, Some(Ok(PaintStatus::Pending { .. })));
                let failure = match result {
                    Some(Err(_)) => Some("画像を利用できません: 描画に失敗しました。"),
                    Some(Ok(PaintStatus::DeviceUnavailable)) => {
                        Some(match window.gpu_device_state() {
                            GpuDeviceState::Unsupported => {
                                "GPU表示を利用できません: このバックエンドでは画像表示が未対応です。"
                            }
                            GpuDeviceState::Recovering { .. } => {
                                "GPUデバイスを復旧中です。画像表示を一時停止しています。"
                            }
                            GpuDeviceState::Unavailable { .. } => {
                                "GPU表示を利用できません。アプリを再起動してください。"
                            }
                            GpuDeviceState::Ready { .. } => {
                                "画像を利用できません: GPU描画の準備が完了していません。"
                            }
                        })
                    }
                    Some(Ok(
                        PaintStatus::Cancelled
                        | PaintStatus::StaleRevision { .. }
                        | PaintStatus::StaleDeviceGeneration { .. }
                        | PaintStatus::Evicted
                        | PaintStatus::DifferentWindow,
                    )) => Some(
                        "画像を利用できません: 現在の画像版・表示先に有効な画像ではありません。",
                    ),
                    _ => None,
                }
                .filter(|message| previous_status.as_ref() != *message);
                let changed = geometry.as_ref() != Some(&transform) || previous_viewport != bounds;
                let promote = complete && phase < if preview { 1 } else { 2 };
                if !changed
                    && !promote
                    && failure.is_none()
                    && !missing
                    && frame.is_none()
                    && !pending_release
                {
                    return;
                }
                let failure_present = failure.is_some();
                let entity = entity.clone();
                window.defer(app, move |_, app| {
                    let _ = entity.update(app, |this, cx| {
                        if (this.serial, this.view_serial) != demand {
                            return;
                        }
                        let resized =
                            this.viewport != bounds || (this.dpi - dpi).abs() > f64::EPSILON;
                        if resized {
                            // Viewport/DPI change first invalidates the previous viewport's demand,
                            // so the new request is never cancelled right after it starts.
                            this.view_serial = this.view_serial.saturating_add(1);
                            this.tile_cancel.cancel();
                            this.demand = None;
                            this.tile_attempts = 0;
                        }
                        let started =
                            missing && needed.is_some_and(|needed| this.request_tiles(needed, cx));
                        this.transform = Some(transform);
                        this.viewport = bounds;
                        this.dpi = dpi;
                        if promote {
                            this.phase += 1;
                        }
                        if let Some(message) = failure {
                            this.status = message.into();
                        }
                        // Repaint only on a real state change; a still-missing tile set is advanced
                        // by tile arrival or the bounded retry timer, never by repaint alone.
                        // Every composed scene counts, including one with no current image: the
                        // retired frame cannot appear in it, so an empty scene still proves the
                        // old reference is gone.
                        this.scene_serial = this.scene_serial.saturating_add(1);
                        this.complete_in_scene = complete;
                        if changed || promote || failure_present || started || resized {
                            cx.notify();
                        }
                    });
                });
            },
        )
        .size_full();
        let pane_label = self.compare_pane.map_or_else(
            || "画像表示".to_owned(),
            |index| format!("比較する{}側の画像", if index == 0 { "A" } else { "B" }),
        );
        let viewport = div().id("image-viewport").role(Role::Group).aria_label(SharedString::from(format!("{pane_label}。0: 全体、1: 物理等倍、+/-: 拡大縮小、矢印: 移動、F11: 全画面、Escape: 復帰")))
            .key_context("ImageViewer").track_focus(&self.focus).relative().flex_1().min_h_0().w_full().overflow_hidden()
            .border_1().border_color(rgb(if self.focus.is_focused(window) { palette.colors.focus } else { palette.colors.border }))
            .on_action(cx.listener(|this, _: &ViewerFit, _, cx| this.change(ViewTransform::fit, cx)))
            .on_action(cx.listener(|this, _: &ViewerOne, _, cx| this.one(cx)))
            .on_action(cx.listener(|this, _: &ViewerIn, _, cx| this.zoom(1.25, None, cx)))
            .on_action(cx.listener(|this, _: &ViewerOut, _, cx| this.zoom(0.8, None, cx)))
            .on_action(cx.listener(|this, _: &ViewerLeft, _, cx| this.pan(64.0, 0.0, cx)))
            .on_action(cx.listener(|this, _: &ViewerRight, _, cx| this.pan(-64.0, 0.0, cx)))
            .on_action(cx.listener(|this, _: &ViewerUp, _, cx| this.pan(0.0, 64.0, cx)))
            .on_action(cx.listener(|this, _: &ViewerDown, _, cx| this.pan(0.0, -64.0, cx)))
            .on_action(cx.listener(|this, _: &ViewerFullscreen, window, cx| this.fullscreen(window, cx)))
            .on_action(cx.listener(|this, _: &ViewerEscape, window, cx| { if window.is_fullscreen() { this.fullscreen(window, cx); } }))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| { this.drag = Some(event.position); this.focus.focus(window, cx); cx.emit(ViewerEvent::Focused); }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.drag = None))
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                if !event.dragging() { this.drag = None; return; }
                if let Some(previous) = this.drag.replace(event.position) { this.pan(f32::from(event.position.x - previous.x) as f64, f32::from(event.position.y - previous.y) as f64, cx); }
            }))
            .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                let delta = event.delta.pixel_delta(px(20.0));
                let cursor = Point { x: f32::from(event.position.x - this.viewport.origin.x) as f64, y: f32::from(event.position.y - this.viewport.origin.y) as f64 };
                this.zoom((f32::from(delta.y) as f64 * 0.005).clamp(-1.0, 1.0).exp(), Some(cursor), cx);
                cx.stop_propagation();
            })).child(frame_canvas);
        let button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .role(Role::Button)
                .aria_label(label)
                .px_3()
                .py_1()
                .border_1()
                .border_color(rgb(palette.colors.border))
                .cursor_pointer()
                .child(label)
        };
        div()
            .id("image-viewer")
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_2()
            .text_color(rgb(palette.colors.text))
            .bg(rgb(palette.colors.background))
            .when(self.compare_pane.is_none(), |element| {
                element.child(
                    div()
                        .flex()
                        .gap_2()
                        .child(button("viewer-fit", "全体表示 (0)").on_click(cx.listener(
                            |this, _, window, cx| {
                                this.change(ViewTransform::fit, cx);
                                this.focus.focus(window, cx);
                            },
                        )))
                        .child(crate::accessibility::DisabledState {
                            inner: button("viewer-one", "物理ピクセル1:1 (1)").on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.one(cx);
                                    this.focus.focus(window, cx);
                                }),
                            ),
                            disabled: preview,
                        })
                        .child(button("viewer-in", "拡大 (+)").on_click(cx.listener(
                            |this, _, window, cx| {
                                this.zoom(1.25, None, cx);
                                this.focus.focus(window, cx);
                            },
                        )))
                        .child(button("viewer-out", "縮小 (-)").on_click(cx.listener(
                            |this, _, window, cx| {
                                this.zoom(0.8, None, cx);
                                this.focus.focus(window, cx);
                            },
                        )))
                        .child(button("viewer-fullscreen", "全画面 / 復帰 (F11)").on_click(
                            cx.listener(|this, _, window, cx| this.fullscreen(window, cx)),
                        )),
                )
            })
            .child(
                div()
                    .id("viewer-status")
                    .role(Role::Label)
                    .aria_label(self.status.clone())
                    .aria_value(self.status.clone())
                    .child(self.status.clone()),
            )
            .child(
                div()
                    .id("viewer-scale")
                    .role(Role::Label)
                    .aria_label(description.clone())
                    .aria_value(description.clone())
                    .child(description),
            )
            .child(viewport)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retirement_polls_switch_from_fast_to_slow_without_stopping() {
        for attempt in 1..=FAST_RETIREMENT_SWEEPS {
            assert_eq!(retirement_interval(attempt, false), FAST_RETIREMENT_MILLIS);
        }
        // Once the fast budget is spent the slow cadence persists; it never returns to fast while
        // a frame is still waiting for release.
        assert_eq!(
            retirement_interval(FAST_RETIREMENT_SWEEPS + 1, true),
            SLOW_RETIREMENT_MILLIS
        );
        assert_eq!(retirement_interval(u8::MAX, true), SLOW_RETIREMENT_MILLIS);
        // The flag selects the slow cadence, not a reset attempt counter.
        assert_eq!(retirement_interval(1, true), SLOW_RETIREMENT_MILLIS);
    }

    #[test]
    fn a_retired_frame_waits_for_a_later_composed_scene() {
        // The scene current at detach may still be presented; two later scenes must exist.
        assert!(!scene_allows_release(0, 0));
        assert!(!scene_allows_release(1, 0));
        assert!(scene_allows_release(2, 0));
        assert!(scene_allows_release(9, 7));
        // A hidden window composes nothing, so it never releases on time alone.
        assert!(!scene_allows_release(4, 4));
    }
}
