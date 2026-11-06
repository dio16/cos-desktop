//! Two image panes over existing media/consumer ownership. Candidate selection stays in Presentation.
use crate::{
    contrast::{ContrastScheme, NORMAL_COLORS, Palette},
    image_upload::{ImageUploadService, PreparedImage, UploadRequest},
    image_viewer::{ImageViewer, ViewerEvent, ViewerMode, ViewerObservation},
    view_transform::{Point, ViewCamera},
};
use gpui::{
    AccessibleAction, App, AppContext, Context, Entity, EventEmitter, FocusHandle,
    InteractiveElement, IntoElement, KeyBinding, ParentElement, Render, Role, SharedString,
    StatefulInteractiveElement, Styled, Window, actions, div, rgb,
};
use media::{Cancellation, MediaError};
use std::sync::Arc;
use ui_model::presentation::CompareSide;

actions!(
    compare_view,
    [CompareNext, ComparePrevious, CompareActivate]
);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("tab", CompareNext, Some("CompareView")),
        KeyBinding::new("shift-tab", ComparePrevious, Some("CompareView")),
        KeyBinding::new("enter", CompareActivate, Some("CompareAction")),
        KeyBinding::new("space", CompareActivate, Some("CompareAction")),
    ]);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompareEvent {
    Swap,
    Change(CompareSide),
    Close,
}

#[derive(Clone, Copy)]
enum Control {
    Fit,
    One,
    Link,
    Swap,
    ChangeA,
    ChangeB,
    Close,
}
const CONTROLS: [Control; 7] = [
    Control::Fit,
    Control::One,
    Control::Link,
    Control::Swap,
    Control::ChangeA,
    Control::ChangeB,
    Control::Close,
];

/// No candidate or canonical state is stored here. The host owns selection and passes captions.
pub struct CompareView {
    panes: [Entity<ImageViewer>; 2],
    labels: [SharedString; 2],
    available: [bool; 2],
    held_sides: [bool; 2],
    linked: bool,
    shared_camera: ViewCamera,
    active: usize,
    controls: [FocusHandle; 7],
    cancel: Cancellation,
    side_cancel: [Cancellation; 2],
    serial: u64,
    pane_serial: [u64; 2],
    mode: ViewerMode,
    palette: Palette,
}

impl CompareView {
    pub fn new(mode: ViewerMode, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let panes = std::array::from_fn(|index| {
            cx.new(|cx| {
                let mut pane = ImageViewer::new(mode, window, cx);
                pane.set_compare_pane(index, cx);
                pane
            })
        });
        for (index, pane) in panes.iter().enumerate() {
            cx.on_focus(&pane.read(cx).focus_handle(), window, move |this, _, cx| {
                this.active = index;
                cx.notify();
            })
            .detach();
            cx.subscribe(pane, move |this, _, event: &ViewerEvent, cx| {
                match event {
                    ViewerEvent::Focused => this.active = index,
                    ViewerEvent::Camera(camera) => this.propagate_shared_camera(index, *camera, cx),
                }
                cx.notify();
            })
            .detach();
        }
        Self {
            panes,
            labels: ["A: 未選択".into(), "B: 未選択".into()],
            linked: true,
            active: 0,
            shared_camera: ViewCamera::default(),
            available: [true; 2],
            held_sides: [true; 2],
            controls: std::array::from_fn(|index| {
                cx.focus_handle()
                    .tab_index(12 + index as isize)
                    .tab_stop(true)
            }),
            cancel: Cancellation::default(),
            serial: 0,
            pane_serial: [0; 2],
            mode,
            side_cancel: std::array::from_fn(|_| Cancellation::default()),
            palette: Palette {
                scheme: ContrastScheme::Normal,
                colors: NORMAL_COLORS,
            },
        }
    }

    pub fn set_palette(&mut self, palette: Palette, cx: &mut Context<Self>) {
        if self.palette == palette {
            return;
        }
        self.palette = palette;
        for pane in &self.panes {
            pane.update(cx, |pane, cx| pane.set_palette(palette, cx));
        }
        cx.notify();
    }

    pub fn set_labels(&mut self, labels: [SharedString; 2], cx: &mut Context<Self>) {
        if self.labels != labels {
            self.labels = labels;
            cx.notify();
        }
    }

    pub fn set_selection_presence(&mut self, held_sides: [bool; 2], cx: &mut Context<Self>) {
        if self.held_sides != held_sides {
            self.held_sides = held_sides;
            cx.notify();
        }
    }

    /// A projection can withhold a side without clearing its held candidate reference.
    pub fn set_projection(
        &mut self,
        labels: [SharedString; 2],
        available: [bool; 2],
        cx: &mut Context<Self>,
    ) {
        self.set_labels(labels, cx);
        for (index, present) in available.into_iter().enumerate() {
            let side = if index == 0 {
                CompareSide::A
            } else {
                CompareSide::B
            };
            if self.available[index] && !present {
                self.unavailable(
                    side,
                    "画像を利用できません: この候補は現在閲覧できる情報に含まれていません。".into(),
                    cx,
                );
                continue;
            }
            // Reappearance restores the reference, not the picture: the withheld reason would
            // otherwise stay under a label that names an available candidate again.
            if !self.available[index] && present {
                self.unavailable(
                    side,
                    "この候補は再び利用できます。認可済み画像はまだ表示されていません。".into(),
                    cx,
                );
            }
        }
        self.available = available;
    }

    pub fn cameras(&self, cx: &App) -> [ViewCamera; 2] {
        std::array::from_fn(|index| self.panes[index].read(cx).camera())
    }

    pub fn frames(&self, cx: &App) -> [Option<Arc<PreparedImage>>; 2] {
        std::array::from_fn(|index| self.panes[index].read(cx).prepared())
    }

    pub fn observations(&self, cx: &App) -> [ViewerObservation; 2] {
        std::array::from_fn(|index| self.panes[index].read(cx).observation())
    }

    pub fn linked(&self) -> bool {
        self.linked
    }

    /// Toggle synchronization without moving either display; the next explicit interaction is
    /// mapped from its originating pane into the common ideal camera.
    pub fn set_linked(&mut self, linked: bool, cx: &mut Context<Self>) {
        self.linked = linked;
        cx.notify();
    }

    pub fn zoom_by(
        &mut self,
        side: CompareSide,
        factor: f64,
        cursor: Option<Point>,
        cx: &mut Context<Self>,
    ) {
        self.panes[side.index()].update(cx, |pane, cx| pane.zoom_by(factor, cursor, cx));
        let camera = self.panes[side.index()].read(cx).camera();
        self.propagate_camera(side.index(), camera, cx);
    }

    pub fn pan_by(&mut self, side: CompareSide, delta: Point, cx: &mut Context<Self>) {
        self.panes[side.index()].update(cx, |pane, cx| pane.pan_by(delta, cx));
        let camera = self.panes[side.index()].read(cx).camera();
        self.propagate_camera(side.index(), camera, cx);
    }

    fn propagate_camera(&mut self, index: usize, camera: ViewCamera, cx: &mut Context<Self>) {
        self.active = index;
        self.propagate_shared_camera(index, camera, cx);
    }

    /// Carries one pane's camera to its linked partner without claiming it was the user's origin.
    /// A pair-wide Fit or 1:1 sets both panes, so the last pane to report must not become the
    /// interaction origin that Swap returns focus to (Q12).
    fn propagate_shared_camera(
        &mut self,
        index: usize,
        camera: ViewCamera,
        cx: &mut Context<Self>,
    ) {
        if self.linked {
            self.shared_camera = camera;
            self.panes[1 - index].update(cx, |pane, cx| pane.set_camera(camera, cx));
        }
    }

    pub fn reset_fit(&mut self, cx: &mut Context<Self>) {
        self.shared_camera = ViewCamera::default();
        for pane in &self.panes {
            pane.update(cx, |pane, cx| pane.set_camera(self.shared_camera, cx));
        }
        cx.notify();
    }

    /// A pair the host must (re)acquire: the identity changed, so the previous picture may not
    /// stay under the new label. The host presents or loads that side next.
    pub fn await_side(&mut self, side: CompareSide, cx: &mut Context<Self>) {
        self.unavailable(
            side,
            "比較対象が変更されました。この候補の認可済み画像はまだ表示されていません。".into(),
            cx,
        );
    }

    pub fn unavailable(&mut self, side: CompareSide, reason: SharedString, cx: &mut Context<Self>) {
        self.side_cancel[side.index()].cancel();
        self.pane_serial[side.index()] = self.pane_serial[side.index()].saturating_add(1);
        self.panes[side.index()].update(cx, |pane, cx| pane.unavailable(reason, cx));
    }

    /// Swap resources, not entities/cameras/focus. Both Arc owners survive the two attachments.
    pub fn swap_resources(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_inputs();
        let frames = self.frames(cx);
        for index in 0..2 {
            match frames[1 - index].clone() {
                Some(frame) => {
                    self.panes[index]
                        .update(cx, |pane, cx| pane.present_prepared(frame, window, cx));
                }
                None => self.panes[index].update(cx, |pane, cx| {
                    pane.unavailable(
                        "画像を利用できません: この候補の認可済み画像がありません。".into(),
                        cx,
                    )
                }),
            }
        }
        // The projection's availability follows the candidate that now sits in each pane, so the
        // swapped side must not look like a reappearance to the next `set_projection` call: that
        // transition would withdraw the picture the swap just attached.
        self.available.swap(0, 1);
        self.panes[self.active]
            .read(cx)
            .focus_handle()
            .focus(window, cx);
        cx.notify();
    }

    /// Explicit local experiment input, never an automatic fallback after Runtime failure.
    /// `share` is an experiment treatment, not production adoption. A shared treatment supplies
    /// one exact input and attaches its verified frame twice; unequal keys use independent entries.
    pub fn load_generated_pair(
        &mut self,
        requests: [UploadRequest; 2],
        share: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
        source: impl Fn(usize, &Cancellation) -> Result<Arc<[u8]>, MediaError> + Send + Sync + 'static,
    ) {
        self.cancel_inputs();
        self.cancel = Cancellation::default();
        self.side_cancel = std::array::from_fn(|_| Cancellation::default());
        self.pane_serial = self.pane_serial.map(|serial| serial.saturating_add(1));
        self.reset_fit(cx);
        for pane in &self.panes {
            pane.update(cx, |pane, cx| {
                pane.unavailable(
                    "ローカル比較画像を準備中（実行基盤の成果物ではありません）".into(),
                    cx,
                )
            });
        }
        let shared = share
            && requests[0].key == requests[1].key
            && requests[0].source_revision == requests[1].source_revision
            && requests[0].device_generation == requests[1].device_generation
            && requests[0].current_device_generation == requests[1].current_device_generation;
        let source = Arc::new(source);
        for (index, request) in requests
            .into_iter()
            .enumerate()
            .take(if shared { 1 } else { 2 })
        {
            let source = Arc::clone(&source);
            let targets = if shared { 0..2 } else { index..index + 1 };
            let cancel = if shared {
                self.cancel.clone()
            } else {
                self.side_cancel[index].clone()
            };
            self.prepare_input(targets, request, cancel, window, cx, move |cancel| {
                source(index, cancel)
            });
        }
    }

    /// Replace/reacquire one exact local input without disturbing the other pane's resource.
    /// The host separately resets pair framing only for an actual candidate replacement.
    pub fn load_generated_side(
        &mut self,
        side: CompareSide,
        request: UploadRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
        source: impl FnOnce(&Cancellation) -> Result<Arc<[u8]>, MediaError> + Send + 'static,
    ) {
        self.unavailable(
            side,
            "ローカル比較画像を準備中（実行基盤の成果物ではありません）".into(),
            cx,
        );
        self.side_cancel[side.index()] = Cancellation::default();
        self.prepare_input(
            side.index()..side.index() + 1,
            request,
            self.side_cancel[side.index()].clone(),
            window,
            cx,
            source,
        );
    }

    pub fn present_prepared(
        &mut self,
        side: CompareSide,
        frame: Arc<PreparedImage>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.side_cancel[side.index()].cancel();
        self.pane_serial[side.index()] = self.pane_serial[side.index()].saturating_add(1);
        self.panes[side.index()].update(cx, |pane, cx| pane.present_prepared(frame, window, cx))
    }

    fn cancel_inputs(&mut self) {
        self.cancel.cancel();
        self.side_cancel.iter().for_each(Cancellation::cancel);
        self.serial = self.serial.saturating_add(1);
    }

    fn prepare_input(
        &mut self,
        targets: std::ops::Range<usize>,
        request: UploadRequest,
        cancel: Cancellation,
        window: &Window,
        cx: &mut Context<Self>,
        source: impl FnOnce(&Cancellation) -> Result<Arc<[u8]>, MediaError> + Send + 'static,
    ) {
        let serial = self.serial;
        let pane_serial = self.pane_serial;
        let service = ImageUploadService::global();
        let task = service.prepare_view_independent_generated(
            cx.background_executor(),
            request,
            cancel,
            self.mode == ViewerMode::PreviewOnly,
            source,
        );
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let mut unclaimed = result.as_ref().ok().cloned();
            let _ = this.update_in(cx, |this, window, cx| {
                if this.serial != serial {
                    return;
                }
                match result {
                    Ok(frame) => {
                        let mut attached = false;
                        for target in targets {
                            if this.pane_serial[target] != pane_serial[target] {
                                continue;
                            }
                            attached |= this.panes[target].update(cx, |pane, cx| {
                                pane.present_prepared(Arc::clone(&frame), window, cx)
                            });
                        }
                        if attached {
                            unclaimed = None;
                        }
                    }
                    Err(error) => {
                        for target in targets {
                            if this.pane_serial[target] != pane_serial[target] {
                                continue;
                            }
                            this.panes[target].update(cx, |pane, cx| {
                                pane.unavailable(
                                    format!("画像を利用できません: {}", error.limitation()).into(),
                                    cx,
                                )
                            });
                        }
                    }
                }
                cx.notify();
            });
            if let Some(frame) = unclaimed {
                service.discard_unpainted(&frame);
            }
        })
        .detach();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.cancel_inputs();
        for pane in &self.panes {
            pane.update(cx, |pane, cx| {
                pane.close();
                pane.set_camera(ViewCamera::default(), cx);
            });
        }
        self.linked = true;
    }

    fn execute(&mut self, control: Control, cx: &mut Context<Self>) {
        match control {
            // Q6/Q10 make Fit and physical 1:1 per-pair controls: unlink keeps each pane's current
            // display and only lets later gestures diverge, so the pair controls still move both
            // panes and the shared camera stays the value a later re-link maps from.
            Control::Fit => self.reset_fit(cx),
            Control::One => {
                if self
                    .panes
                    .iter()
                    .all(|pane| pane.read(cx).supports_one_to_one())
                {
                    for pane in &self.panes {
                        pane.update(cx, |pane, cx| pane.physical_one_to_one(cx));
                    }
                }
            }
            Control::Link => self.linked = !self.linked,
            Control::Swap => cx.emit(CompareEvent::Swap),
            Control::ChangeA => cx.emit(CompareEvent::Change(CompareSide::A)),
            Control::ChangeB => cx.emit(CompareEvent::Change(CompareSide::B)),
            Control::Close => cx.emit(CompareEvent::Close),
        }
        cx.notify();
    }
}
impl EventEmitter<CompareEvent> for CompareView {}
impl Drop for CompareView {
    fn drop(&mut self) {
        self.cancel_inputs();
    }
}

impl Render for CompareView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = self.palette.colors;
        let one_enabled = self
            .panes
            .iter()
            .all(|pane| pane.read(cx).supports_one_to_one());
        div().id("compare-images").key_context("CompareView").size_full().min_h_0().flex().flex_col().gap_2()
            .text_color(rgb(colors.text)).bg(rgb(colors.background))
            .on_action(cx.listener(|_, _: &CompareNext, window, cx| { window.focus_next(cx); cx.stop_propagation(); }))
            .on_action(cx.listener(|_, _: &ComparePrevious, window, cx| { window.focus_prev(cx); cx.stop_propagation(); }))
            .child(div().flex_1().min_h_0().flex().gap_2().children((0..2).map(|index| {
                div().id(("compare-pane", index)).role(Role::Group).aria_label(self.labels[index].clone())
                    .flex_1().min_w_0().min_h_0().border_1().border_color(rgb(colors.border))
                    .flex().flex_col().child(self.labels[index].clone()).child(self.panes[index].clone())
            })))
            .child(div().flex().flex_wrap().gap_2().children(CONTROLS.into_iter().enumerate().map(|(index, control)| {
                let label = match control {
                    Control::Fit => "全体表示 (0)", Control::One => "物理ピクセル1:1 (1)",
                    Control::Link if self.linked => "連動を解除", Control::Link => "連動する",
                    Control::Swap => "A/Bを交換", Control::ChangeA => "A側を変更",
                    Control::ChangeB => "B側を変更", Control::Close => "比較を閉じる",
                };
                let enabled = match control {
                    Control::One => one_enabled,
                    Control::ChangeA => self.held_sides[0],
                    Control::ChangeB => self.held_sides[1],
                    Control::Swap | Control::Link => self.held_sides.iter().all(|held| *held),
                    Control::Close => self.held_sides.iter().any(|held| *held),
                    Control::Fit => true,
                };
                crate::accessibility::DisabledState {
                    disabled: !enabled,
                    inner: div().id(("compare-control", index)).role(Role::Button).aria_label(label)
                        .aria_description(match control {
                            Control::One if !enabled => "原寸を表示できる画像がないため、物理1:1は利用できません。低解像度プレビューは原寸ではありません。",
                            Control::ChangeA | Control::ChangeB => "押したあとに候補一覧から選択します。閲覧中の候補と正式な採用結果は変更しません。",
                            Control::Swap => "候補だけを交換します。各ペインの表示位置・倍率とフォーカスは維持します。",
                            Control::Link => "画像の位置と倍率を連動します。画像資源の共有とは別の操作です。",
                            Control::Close => "閲覧中の候補と、その候補の表示位置・倍率に戻ります。正式な採用結果は変更しません。",
                            _ => "この画面の表示だけを変更します。正式な採用結果は変更しません。",
                        })
                        .key_context("CompareAction").track_focus(&self.controls[index]).px_3().py_1()
                        .border_1().border_color(rgb(if self.controls[index].is_focused(window) { colors.focus } else { colors.border })).cursor_pointer().child(label)
                        .on_action(cx.listener(move |this, _: &CompareActivate, _, cx| { if enabled { this.execute(control, cx); } cx.stop_propagation(); }))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if enabled { this.controls[index].focus(window, cx); this.execute(control, cx); }
                        }))
                        .on_a11y_action(AccessibleAction::Click, {
                            let entity = cx.entity().downgrade();
                            move |_, _, cx| { if enabled { let _ = entity.update(cx, |this, cx| this.execute(control, cx)); } }
                        }),
                }
            })))
    }
}
