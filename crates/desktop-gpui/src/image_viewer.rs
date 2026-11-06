//! Local-input MP-IMG5 viewer. Runtime acquisition is deliberately not implemented here.
//! The experimental tile mode is not an R4 adoption/residency claim; production uses an
//! explicitly labelled bounded preview until those evidence gates and MP-IMG6 are satisfied.
use crate::{
    contrast::{ContrastScheme, NORMAL_COLORS, Palette},
    image_upload::{
        ConsumerId, FRAME_EDGE, ImageUploadError, ImageUploadService, PaintStatus, PreparedImage,
        TileDemand, UploadRequest, ViewPaint,
    },
    view_transform::{Point, Size, ViewTransform},
};
use gpui::{
    App, Bounds, Context, FocusHandle, IntoElement, KeyBinding, MouseButton, Pixels, Render, Role,
    SharedString, Window, actions, canvas, div, fill, point, prelude::*, px, rgb, size,
};
use media::{Cancellation, MediaError, Orientation};
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

/// First retirement sweeps run two frames apart, which is the shortest gap that can outlive the
/// scene that referenced the frame.
const FAST_RETIREMENT_FRAMES: u8 = 2;
/// Later sweeps are slow so a delayed last-detach is still collected without per-frame churn.
const SLOW_RETIREMENT_FRAMES: u8 = 30;
/// Number of fast sweeps before falling back to the slow cadence.
const FAST_RETIREMENT_SWEEPS: u8 = 8;
/// Bound on consecutive transient tile-preparation retries for one viewport demand.
const TILE_RETRY_LIMIT: u8 = 20;

fn retirement_delay(attempts: u8, slow: bool) -> u8 {
    if !slow && attempts <= FAST_RETIREMENT_SWEEPS {
        FAST_RETIREMENT_FRAMES
    } else {
        SLOW_RETIREMENT_FRAMES
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewerMode {
    PreviewOnly,
    ExperimentalTiles,
}

/// GPU handles never leave this GPUI-boundary entity. Input must carry explicit local provenance.
pub struct ImageViewer {
    service: ImageUploadService,
    current: Option<Arc<PreparedImage>>,
    cancel: Cancellation,
    /// Cancellation owned by the current viewport tile demand.
    tile_cancel: Cancellation,
    demand: Option<TileDemand>,
    demand_id: u64,
    tile_attempts: u8,
    /// Frames detached from this viewer but still reachable from the last presented scene.
    retiring: Vec<Arc<PreparedImage>>,
    retirement_scheduled: bool,
    retirement_attempts: u8,
    retirement_slow: bool,
    serial: u64,
    view_serial: u64,
    revision: u64,
    generation: u64,
    transform: Option<ViewTransform>,
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
    pub fn new(mode: ViewerMode, _window: &Window, cx: &mut Context<Self>) -> Self {
        Self {
            service: ImageUploadService::global(), current: None, cancel: Cancellation::default(),
            tile_cancel: Cancellation::default(), demand: None, demand_id: 0, tile_attempts: 0,
            retiring: Vec::new(),
            retirement_scheduled: false, retirement_attempts: 0, retirement_slow: false,
            serial: 0, view_serial: 0, revision: 0, generation: 0, transform: None,
            viewport: Bounds::default(), focus: cx.focus_handle().tab_stop(true), drag: None,
            mode, phase: 0, status: "画像を利用できません: 認可済みの画像入力がありません。実行基盤からの取得は未対応です。".into(),
            palette: Palette { scheme: ContrastScheme::Normal, colors: NORMAL_COLORS }, consumer: ConsumerId::allocate(),
        }
    }

    /// The shell owns the OS probe; child viewers consume its already-resolved palette.
    pub fn set_palette(&mut self, palette: Palette, cx: &mut Context<Self>) {
        if self.palette != palette {
            self.palette = palette;
            cx.notify();
        }
    }

    /// Open a caller-supplied generated/fixture input off-thread, never a Runtime artifact substitute.
    pub fn load_generated(
        &mut self,
        request: UploadRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
        source: impl FnOnce(&Cancellation) -> Result<Arc<[u8]>, MediaError> + Send + 'static,
    ) {
        self.cancel.cancel();
        self.tile_cancel.cancel();
        self.demand = None;
        self.tile_attempts = 0;
        self.serial = self.serial.saturating_add(1);
        self.release_current(window, cx);
        self.revision = request.source_revision;
        self.generation = request.current_device_generation;
        self.phase = 0;
        self.transform = None;
        self.cancel = Cancellation::default();
        if request.key.orientation != Orientation::Normal {
            self.status =
                "画像を利用できません: このアップロード経路では標準の向きだけが検証済みです。"
                    .into();
            cx.notify();
            return;
        }
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
        // Keep the completion cleanup alive after a view/revision is superseded. The cancellation
        // token stops preparation; dropping its waiter must not strand a just-prepared frame.
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let mut unclaimed = result.as_ref().ok().cloned();
            let _ = this.update_in(cx, |this, window, cx| {
                if serial != this.serial { return; }
                match result {
                    Ok(frame) if this.service.attach(&frame, this.consumer, Some(window.window_handle())) => {
                        this.current = Some(frame);
                        unclaimed = None;
                        this.status = match this.mode {
                            ViewerMode::PreviewOnly => "ローカル画像: 低解像度プレビュー。原寸の詳細表示・1:1ではありません。",
                            ViewerMode::ExperimentalTiles => "ローカル画像・実験用タイル表示。GPU完了・常駐量・本番の画質受入は未計測。",
                        }.into();
                    }
                    Ok(_) => this.status = "画像を利用できません: 保持されていない画像です。".into(),
                    Err(error) => this.status = format!("画像を利用できません: {error}").into(),
                }
                cx.notify();
            });
            if let Some(frame) = unclaimed { cleanup.discard_unpainted(&frame); }
        }).detach();
        cx.notify();
    }

    /// Call before removing the owning view/window. A painted payload is never released here: the
    /// scene that referenced it may still be presented and there is no completion signal. Frames
    /// that never reached the atlas are released through the existing unpainted path.
    pub fn close(&mut self) {
        self.cancel.cancel();
        self.tile_cancel.cancel();
        self.demand = None;
        self.serial = self.serial.saturating_add(1);
        let Some(frame) = self.current.take() else {
            return;
        };
        self.service.detach(&frame, self.consumer);
        if !self.service.discard_unpainted(&frame) {
            // Painted frames stay owned past close; the platform destroys the atlas with the
            // window, so no atlas entry is removed while a presented scene may reference it.
            self.retiring.push(frame);
        }
    }

    /// Detach at once so no later scene adds these tiles, then drop the atlas entries only after
    /// two presented frames can no longer reference them. A frame another consumer still presents
    /// stays retired and is retried by the next sweep.
    fn release_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(frame) = self.current.take() else {
            return;
        };
        self.service.detach(&frame, self.consumer);
        self.retiring.push(frame);
        self.schedule_retirement(window, cx, FAST_RETIREMENT_FRAMES);
    }

    fn schedule_retirement(&mut self, window: &Window, cx: &mut Context<Self>, frames: u8) {
        if self.retirement_scheduled || self.retiring.is_empty() {
            return;
        }
        self.retirement_scheduled = true;
        let entity = cx.entity().downgrade();
        let left = std::rc::Rc::new(std::cell::Cell::new(frames.max(1)));
        Self::arm_retirement(window, entity, left);
    }

    /// Wait `frames` presented frames, then run one retirement sweep.
    fn arm_retirement(
        window: &Window,
        entity: gpui::WeakEntity<Self>,
        left: std::rc::Rc<std::cell::Cell<u8>>,
    ) {
        window.on_next_frame(move |window, cx| {
            if left.get() > 1 {
                left.set(left.get() - 1);
                Self::arm_retirement(window, entity.clone(), std::rc::Rc::clone(&left));
                return;
            }
            let _ = entity.update_in(cx, |this, window, cx| {
                this.retirement_scheduled = false;
                this.evict_retired(window, cx);
                cx.notify();
            });
        });
    }

    fn evict_retired(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut keep = Vec::new();
        for frame in std::mem::take(&mut self.retiring) {
            match self.service.evict(&frame, window) {
                Ok(_) | Err(ImageUploadError::NotRetained) => {}
                Err(error) => {
                    // Another consumer still presents it, or release is not provable yet.
                    self.status = format!("画像の解放を保留: {error}").into();
                    keep.push(frame);
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
        let delay = retirement_delay(self.retirement_attempts, self.retirement_slow);
        self.schedule_retirement(window, cx, delay);
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
                        this.status = format!("画像の表示を制限: {error}").into();
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
        self.view_serial = self.view_serial.saturating_add(1);
        self.tile_cancel.cancel();
        self.demand = None;
        self.tile_attempts = 0;
        if let Some(transform) = self.transform.as_mut() {
            operation(transform);
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
        if self.mode == ViewerMode::ExperimentalTiles {
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
        for frame in self.current.iter().chain(&self.retiring) {
            self.service.detach(frame, self.consumer);
        }
    }
}

impl Render for ImageViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = self.palette;
        let scale = self
            .transform
            .as_ref()
            .map(ViewTransform::effective_physical_scale);
        let description: SharedString = match (self.mode, scale) {
            (ViewerMode::PreviewOnly, _) => "低解像度プレビュー（原寸・1:1ではありません）".into(),
            (_, Some(scale)) => format!(
                "物理表示倍率 {:.1}% / DIP変換 {:.2} / クリップ領域あり得ます",
                scale * 100.0,
                window.scale_factor()
            )
            .into(),
            _ => "画像は未表示です".into(),
        };
        let frame = self.current.clone();
        let cancel = self.cancel.clone();
        let revision = self.revision;
        let generation = self.generation;
        let geometry = self.transform.clone();
        let phase = self.phase;
        let preview = self.mode == ViewerMode::PreviewOnly;
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
                let needed = frame
                    .as_ref()
                    .map(|frame| frame.demand(image_bounds, bounds, mip.min(4) as u8));
                let result = frame.as_ref().map(|frame| {
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
                });
                let complete = matches!(result, Some(Ok(PaintStatus::CompleteInScene { .. })));
                let missing = matches!(result, Some(Ok(PaintStatus::Pending { .. })));
                let failure = match result {
                    Some(Err(_)) => Some("画像を利用できません: 描画に失敗しました。"),
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
                if !changed && !promote && failure.is_none() && !missing {
                    return;
                }
                let failure_present = failure.is_some();
                let entity = entity.clone();
                window.defer(app, move |_, app| {
                    let _ = entity.update(app, |this, cx| {
                        if (this.serial, this.view_serial) != demand {
                            return;
                        }
                        let resized = this.viewport != bounds;
                        let started =
                            missing && needed.is_some_and(|needed| this.request_tiles(needed, cx));
                        this.transform = Some(transform);
                        this.viewport = bounds;
                        if resized {
                            // A resize/DPI change invalidates the previous viewport's tile demand.
                            this.view_serial = this.view_serial.saturating_add(1);
                            this.tile_cancel.cancel();
                            this.demand = None;
                            this.tile_attempts = 0;
                        }
                        if promote {
                            this.phase += 1;
                        }
                        if let Some(message) = failure {
                            this.status = message.into();
                        }
                        // Repaint only on a real state change; a still-missing tile set is advanced
                        // by tile arrival or the bounded retry timer, never by repaint alone.
                        if changed || promote || failure_present || started || resized {
                            cx.notify();
                        }
                    });
                });
            },
        )
        .size_full();
        let viewport = div().id("image-viewport").role(Role::Group).aria_label("画像表示。0: 全体、1: 物理等倍、+/-: 拡大縮小、矢印: 移動、F11: 全画面、Escape: 復帰")
            .key_context("ImageViewer").track_focus(&self.focus).relative().flex_1().min_h_0().w_full().overflow_hidden()
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
            .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| { this.drag = Some(event.position); this.focus.focus(window, cx); }))
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
            .child(
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
                        inner: button("viewer-one", "物理ピクセル1:1 (1)").on_click(cx.listener(
                            |this, _, window, cx| {
                                this.one(cx);
                                this.focus.focus(window, cx);
                            },
                        )),
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
                    .child(
                        button("viewer-fullscreen", "全画面 / 復帰 (F11)").on_click(
                            cx.listener(|this, _, window, cx| this.fullscreen(window, cx)),
                        ),
                    ),
            )
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
    fn retirement_sweeps_switch_from_fast_to_slow_without_stopping() {
        for attempt in 1..=FAST_RETIREMENT_SWEEPS {
            assert_eq!(retirement_delay(attempt, false), FAST_RETIREMENT_FRAMES);
        }
        // Once the fast budget is spent the slow cadence persists; it never returns to fast while
        // a frame is still waiting for release.
        assert_eq!(
            retirement_delay(FAST_RETIREMENT_SWEEPS + 1, true),
            SLOW_RETIREMENT_FRAMES
        );
        assert_eq!(retirement_delay(u8::MAX, true), SLOW_RETIREMENT_FRAMES);
        // The flag selects the slow cadence, not a reset attempt counter.
        assert_eq!(retirement_delay(1, true), SLOW_RETIREMENT_FRAMES);
    }
}
