//! Local-input MP-IMG5 viewer. Runtime acquisition is deliberately not implemented here.
//! The experimental tile mode is not an R4 adoption/residency claim; production uses an
//! explicitly labelled bounded preview until those evidence gates and MP-IMG6 are satisfied.
use crate::{
    contrast::{ContrastScheme, NORMAL_COLORS, Palette},
    image_upload::{
        ConsumerId, FRAME_EDGE, ImageUploadError, ImageUploadService, PaintStatus, PreparedImage,
        TileDemand, UploadRequest, ViewPaint,
    },
    media_presentation::{
        MediaRefetchState, admitted_bytes, media_decode_failure_text, media_preparing_text,
        media_success_text, media_unavailable_text, media_upload_request,
    },
    view_transform::{Point, Rect, Size, ViewCamera, ViewTransform},
};
use app_core::{
    MediaAvailability, MediaFetchCancellation, MediaPresentation, MediaRepository, MediaSelection,
    MediaSource, MediaUnavailable, MediaUnavailableCause,
};
use gpui::{
    AccessibleAction, App, Bounds, Context, EventEmitter, FocusHandle, GpuDeviceState, IntoElement,
    KeyBinding, MouseButton, Pixels, Render, Role, SharedString, Subscription, Window, actions,
    canvas, div, fill, point, prelude::*, px, rgb, size,
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
        ViewerEscape,
        ViewerRefetch
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
        KeyBinding::new("r", ViewerRefetch, Some("ImageViewer")),
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

/// Local-input failure sentence; the media route uses its own explicit state wording.
fn local_failure_text(error: &ImageUploadError) -> SharedString {
    format!("画像を利用できません: {}", error.limitation()).into()
}

/// A media result may be applied only when it belongs to the newest request.
///
/// Comparing against the *latest* request, not the last applied one, also rejects a
/// result whose selection was withdrawn (candidate change, unavailable state, close)
/// between the request and its completion.
fn media_result_is_current(request_serial: u64, latest_serial: u64) -> bool {
    request_serial != 0 && request_serial == latest_serial
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
    /// The user activated the one manual media refetch this pane offered.
    ///
    /// The viewer starts no fetch: the host owns the canonical identity, the
    /// live session credential and the cancellation, and re-resolves the same
    /// media selection through them.
    RefetchMedia,
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
    /// Opaque origin of the frame currently attached for painting.
    pub current_origin: Option<u64>,
    /// Opaque origin held as this pane's recovery backup.
    pub retained_origin: Option<u64>,
    /// Detached frames held until a later composed scene permits retirement.
    pub retiring_frames: usize,
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
    /// Monotonic media request sequence; only the newest result may be applied.
    media_request_serial: u64,
    /// Explicit media outcome plus the one manual refetch this pane may offer.
    media_refetch: MediaRefetchState,
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
            media_request_serial: 0,
            media_refetch: MediaRefetchState::default(),
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
        self.drop_backup();
        self.requested = None;
        self.recovery_pending = false;
        self.recovery_attempted_generation = None;
        cx.notify();
    }

    /// Adopt a frame as this pane's reusable backup, claiming its CPU owner in the service.
    fn adopt_backup(&mut self, frame: Arc<PreparedImage>) {
        if self
            .retained
            .as_ref()
            .is_some_and(|retained| retained.origin() == frame.origin())
        {
            self.retained = Some(frame);
            return;
        }
        let previous = self.retained.replace(Arc::clone(&frame));
        self.service.claim_backup(frame.origin());
        if let Some(previous) = previous {
            self.release_backup(previous.origin());
        }
    }

    /// Drop this pane's backup and withdraw its claim.
    fn drop_backup(&mut self) {
        let Some(previous) = self.retained.take() else {
            return;
        };
        self.release_backup(previous.origin());
    }

    /// Withdraw this pane's claim and release the owner once nothing else references it.
    fn release_backup(&mut self, origin: u64) {
        self.service.release_backup_claim(origin);
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
            current_origin: self.current.as_ref().map(|frame| frame.origin()),
            retained_origin: self.retained.as_ref().map(|frame| frame.origin()),
            retiring_frames: self.retiring.len(),
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
        self.adopt_backup(Arc::clone(&frame));
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
        self.adopt_backup(Arc::clone(&frame));
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
        // The pane no longer addresses the selection its in-flight resolution was
        // requested for, so that result must not be applied when it completes.
        self.media_request_serial = self.media_request_serial.saturating_add(1);
        self.recovery_pending = false;
        self.recovery_attempted_generation = None;
        self.release_current(cx);
        self.drop_backup();
        // The host withdrew this pane's picture: no recovery backup may resurrect it later.
        self.requested = None;
        self.demand = None;
        // A non-media reason replaced the pane's status: no media outcome (and no
        // refetch offer) may survive it.
        self.media_refetch.clear();
        self.status = reason;
        cx.notify();
    }

    /// Whether this pane currently offers the one manual media refetch.
    ///
    /// Compare panes carry no host-owned refetch path, so only the standalone
    /// artifact pane offers it.
    pub fn media_refetch_offered(&self) -> bool {
        self.compare_pane.is_none() && self.media_refetch.offers()
    }

    /// Take the refetch offer and ask the host to re-resolve the same media.
    ///
    /// The pane starts no fetch itself. It records that one bounded attempt is in
    /// flight (a repeated click or key press cannot start a second) and emits
    /// [`ViewerEvent::RefetchMedia`]; the host owns the canonical identity, the
    /// live session credential, the cancellation and the resolution.
    pub fn request_media_refetch(&mut self, cx: &mut Context<Self>) -> bool {
        if self.compare_pane.is_some() || !self.media_refetch.request() {
            return false;
        }
        cx.emit(ViewerEvent::RefetchMedia);
        cx.notify();
        true
    }

    /// The host could not start the requested refetch.
    ///
    /// The explicit media state is kept (never replaced by a fabricated one) and
    /// the control is re-enabled for a later manual attempt instead of leaving
    /// it silently disabled.
    pub fn media_refetch_unstarted(&mut self, cx: &mut Context<Self>) {
        self.media_refetch.settle();
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
        let preview_only = self.mode == ViewerMode::PreviewOnly;
        let preparing: SharedString =
            "ローカル画像を準備中（実行基盤の成果物ではありません）".into();
        let success: SharedString = match self.mode {
            ViewerMode::PreviewOnly => {
                "ローカル画像: 低解像度プレビュー。原寸の詳細表示・1:1ではありません。"
            }
            ViewerMode::ExperimentalTiles => {
                "ローカル画像・実験用タイル表示。GPU完了・常駐量・本番の画質受入は未計測。"
            }
        }
        .into();
        self.begin_view_preparation(
            request,
            generation,
            window,
            cx,
            preparing,
            success,
            local_failure_text,
            move |service, background, request, cancel| {
                service.prepare_view_generated(&background, request, cancel, preview_only, source)
            },
        );
    }

    /// Present one media resolution result.
    ///
    /// An unavailable state is rendered as its own explicit sentence; admitted bytes go through
    /// the canonical media upload identity and the existing background preparation path. The
    /// fetch, admission and cache decisions stay in `app-core`/`media`; this viewer owns only
    /// the GPUI attachment, texture lifetime and status wording. A caller that already holds a
    /// resolved outcome applies it unconditionally; off-thread resolution goes through
    /// [`Self::resolve_media`], which orders concurrent results.
    pub fn load_media(
        &mut self,
        presentation: MediaPresentation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = window.gpu_device_state();
        if state != self.device_state {
            self.device_state_changed(state, window, cx);
        }
        let GpuDeviceState::Ready { generation } = state else {
            self.unavailable(device_status(state), cx);
            return;
        };
        match presentation {
            // Cancellation is not an availability state: this pane keeps its current status.
            MediaPresentation::Cancelled => self.media_refetch.settle(),
            MediaPresentation::Unavailable(unavailable) => {
                self.unavailable(media_unavailable_text(&unavailable), cx);
                // Only an explicit media outcome may offer a refetch; `unavailable`
                // cleared the previous one before this state was recorded.
                self.media_refetch
                    .record_unavailable(unavailable.availability);
            }
            MediaPresentation::Admitted(admitted) => {
                // A presented picture replaces the empty/unavailable state (and any
                // refetch offer) before the background preparation completes.
                self.media_refetch.record_available();
                let request = match media_upload_request(&admitted, generation) {
                    Ok(request) => request,
                    Err(refusal) => {
                        self.unavailable(refusal.text().into(), cx);
                        return;
                    }
                };
                let verified = admitted.identity.verified();
                let preview_only = self.mode == ViewerMode::PreviewOnly;
                let bytes = admitted_bytes(&admitted);
                self.begin_view_preparation(
                    request,
                    generation,
                    window,
                    cx,
                    media_preparing_text(verified),
                    media_success_text(verified, preview_only),
                    media_decode_failure_text,
                    move |service, background, request, cancel| {
                        service.prepare_media(&background, request, cancel, preview_only, bytes)
                    },
                );
            }
        }
    }

    /// Resolve one media selection on GPUI's background executor, then apply the outcome here.
    ///
    /// The descriptor/byte exchange, cache lookup and remote admission are blocking work and
    /// never run on the UI thread; only [`Self::load_media`]'s status/attachment work is applied
    /// on it. Results are ordered by their request sequence: an older resolution that completes
    /// after a newer one was applied is dropped instead of clearing the newer picture. A poisoned
    /// repository is reported as an explicit failure, never as a picture.
    pub fn resolve_media(
        &mut self,
        repository: std::sync::Arc<std::sync::Mutex<MediaRepository>>,
        mut source: Box<dyn MediaSource + Send>,
        selection: MediaSelection,
        cancellation: MediaFetchCancellation,
        cx: &mut Context<Self>,
    ) {
        self.media_request_serial = self.media_request_serial.saturating_add(1);
        let request_serial = self.media_request_serial;
        let background = cx.background_executor().clone();
        let task = background.spawn(async move {
            match repository.lock() {
                Ok(mut repository) => repository.load(source.as_mut(), &selection, &cancellation),
                Err(_) => MediaPresentation::Unavailable(MediaUnavailable {
                    availability: MediaAvailability::Failed,
                    cause: MediaUnavailableCause::Identity,
                }),
            }
        });
        cx.spawn(async move |this, cx| {
            let presentation = task.await;
            let _ = this.update_in(cx, |viewer, window, cx| {
                viewer.apply_media_if_current(presentation, request_serial, window, cx);
            });
        })
        .detach();
    }

    /// Apply a media resolution only when no later requested resolution was already applied.
    fn apply_media_if_current(
        &mut self,
        presentation: MediaPresentation,
        request_serial: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !media_result_is_current(request_serial, self.media_request_serial) {
            return;
        }
        self.load_media(presentation, window, cx);
    }

    /// Reset this pane for one explicit host request, prepare it off-thread and attach the
    /// resulting frame. `start` creates the preparation task only; attachment, retention and
    /// publication ownership stay in this viewer.
    #[allow(clippy::too_many_arguments)]
    fn begin_view_preparation(
        &mut self,
        request: UploadRequest,
        generation: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
        preparing: SharedString,
        success: SharedString,
        failure: fn(&ImageUploadError) -> SharedString,
        start: impl FnOnce(
            &ImageUploadService,
            gpui::BackgroundExecutor,
            UploadRequest,
            Cancellation,
        ) -> gpui::Task<Result<Arc<PreparedImage>, ImageUploadError>>
        + 'static,
    ) {
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
        let replaced_origin = self.retained.as_ref().map(|frame| frame.origin());
        self.drop_backup();
        if let Some(origin) = replaced_origin {
            // A load for another identity must not keep the previous candidate's CPU owner: the
            // release is decided after the old frame left this pane's references.
            self.release_origin_after_registry(origin);
        }
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
        self.status = preparing;
        let serial = self.serial;
        let service = self.service.clone();
        let cleanup = service.clone();
        let task = start(
            &service,
            cx.background_executor().clone(),
            request,
            self.cancel.clone(),
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
                        this.adopt_backup(Arc::clone(&frame));
                        this.current = Some(frame);
                        unclaimed = None;
                        this.status = success;
                    }
                    Ok(_) => {
                        this.status = "画像を利用できません: 保持されていない画像です。".into()
                    }
                    Err(error) => this.status = failure(&error),
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
                self.drop_backup();
                self.recovery_pending = false;
                self.recovery_attempted_generation = None;
                self.release_origin_after_registry(previous.origin());
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
                        this.adopt_backup(Arc::clone(&frame));
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
        self.media_request_serial = self.media_request_serial.saturating_add(1);
        self.recovery_pending = false;
        self.recovery_attempted_generation = None;
        self.requested = None;
        let retained_origin = self.retained.as_ref().map(|frame| frame.origin());
        self.drop_backup();
        let window_scenes = self.service.composed_frames(self.window_id());
        for retired in &mut self.retiring {
            if let ReleaseClock::Pane(_) = retired.clock {
                retired.clock = ReleaseClock::Window(window_scenes);
            }
        }
        let Some(frame) = self.current.take() else {
            if let Some(origin) = retained_origin {
                self.release_origin_after_registry(origin);
            }
            self.schedule_retirement(cx);
            return;
        };
        self.service.detach(&frame, self.consumer);
        let discarded = self.service.discard_unpainted(&frame);
        if !discarded
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
        // The service dropped its registry entry (or the frame is now only retired), so the
        // release decision is re-evaluated here rather than before the removal, where the
        // registry entry itself would have refused it.
        if let Some(origin) = retained_origin {
            self.release_origin_after_registry(origin);
        }
    }

    /// Re-evaluate a release once the registry no longer holds the frame.
    fn release_origin_after_registry(&mut self, origin: u64) {
        if self
            .retiring
            .iter()
            .any(|retired| retired.frame.origin() == origin)
        {
            // Retirement still owns the frame; the sweep retries after eviction.
            return;
        }
        self.service.release_origin(origin);
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
                Ok(_) => {
                    // The registry entry is gone and the pane no longer references this origin, so
                    // an unpainted CPU owner can be reclaimed now; a still-painted or shared one is
                    // refused by the service and stays charged.
                    self.service.release_origin(retired.frame.origin());
                }
                Err(ImageUploadError::NotRetained) => {
                    self.service.release_origin(retired.frame.origin());
                }
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
                // The transform maps the frame's own square edge, not a process-wide constant: a
                // 1024 or 320 admitted artifact must fill the viewport like the 2048 class does.
                let source_edge = frame.as_ref().map_or(FRAME_EDGE, |frame| frame.edge());
                let mut transform = match geometry.clone() {
                    Some(transform) => transform,
                    None => match ViewTransform::new(
                        Size {
                            width: source_edge as f64,
                            height: source_edge as f64,
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
        let refetch_hint = if self.media_refetch_offered() {
            "、R: メディア再取得"
        } else {
            ""
        };
        let viewport = div().id("image-viewport").role(Role::Group).aria_label(SharedString::from(format!("{pane_label}。0: 全体、1: 物理等倍、+/-: 拡大縮小、矢印: 移動、F11: 全画面、Escape: 復帰{refetch_hint}")))
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
            .on_action(cx.listener(|this, _: &ViewerRefetch, _, cx| { this.request_media_refetch(cx); }))
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
                        ))
                        // One manual refetch, offered only from an explicit expired or
                        // refused-principal state. Activation means the same thing to a
                        // keyboard, a screen reader (UIA Invoke) and a mouse click: ask
                        // the host to re-resolve the same canonical identity through the
                        // current session. The pane takes the offer before emitting, so a
                        // double click or key repeat cannot start a second resolution.
                        .when(self.media_refetch_offered(), |element| {
                            element.child(
                                button("viewer-refetch", "メディアを再取得 (R)")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        if this.request_media_refetch(cx) {
                                            this.focus.focus(window, cx);
                                        }
                                    }))
                                    .on_a11y_action(AccessibleAction::Click, {
                                        let entity = cx.entity().downgrade();
                                        move |_, _, cx| {
                                            let _ = entity.update(cx, |this, cx| {
                                                this.request_media_refetch(cx);
                                            });
                                        }
                                    }),
                            )
                        }),
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

    #[test]
    fn only_the_newest_media_request_may_be_applied() {
        assert!(media_result_is_current(1, 1));
        assert!(media_result_is_current(2, 2));
        // A late completion of an older request keeps the newer selection's state.
        assert!(!media_result_is_current(1, 2));
        // A withdrawn selection (candidate change, unavailable, close) advanced the
        // sequence, so an already completed result is not re-applied.
        assert!(!media_result_is_current(2, 3));
        // No request has been issued yet.
        assert!(!media_result_is_current(0, 0));
    }
}
