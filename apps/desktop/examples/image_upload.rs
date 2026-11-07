//! Explicit deterministic-fixture feasibility probe for MP-IMG3-S2; this is not Runtime import.
//!
//! Run: `cargo run --locked -p cos-desktop --example image_upload -- --fixture`
//! The fixture is generated in memory on GPUI's background executor, then passes through
//! `media::decode_for_upload` and the reusable desktop-gpui whole-image tile path. No Runtime,
//! artifact success, GPU completion, Present or residency result is fabricated.
use std::sync::Arc;
use std::time::Duration;

use desktop_gpui::image_upload::{ConsumerId, ImageUploadError, ImageUploadService, PaintStatus};
#[cfg(feature = "gpu-recovery-test")]
use gpui::GpuDeviceRecoveryTest;
use gpui::prelude::*;
use gpui::{
    App, AppContext, Bounds, Context, GpuDeviceState, IntoElement, Render, Role, SharedString,
    Subscription, Task, TitlebarOptions, Window, WindowBounds, WindowOptions, canvas, div, fill,
    px, rgb, size,
};
use media::{Cancellation, MediaError};

#[path = "support/image_fixture.rs"]
mod image_fixture;
use image_fixture::{fixture_request_for, generate_fixture_png};
const DISPLAY_EDGE: f32 = 480.0;
/// Fixed manifest for the eviction loop: two revisions of one source, two further sources, and one
/// identity repeated so a single frame is attached by two consumers.
const MANIFEST: [(&str, u64); 5] = [
    ("shared", 1),
    ("alpha", 1),
    ("alpha", 2),
    ("beta", 1),
    ("gamma", 1),
];

#[derive(Clone, Copy)]
enum Mode {
    Fixture,
    /// Capacity in bytes for the process-wide quarantine ladder.
    Manifest(u64),
}
fn backend_reason(state: GpuDeviceState) -> SharedString {
    match state {
        GpuDeviceState::Unsupported => {
            "GPU表示を利用できません: このバックエンドでは画像表示が未対応です。".into()
        }
        GpuDeviceState::Ready { generation } => {
            format!("GPUデバイス世代 {generation} を利用できます。").into()
        }
        GpuDeviceState::Recovering { generation } => {
            format!("GPUデバイスを復旧中です（世代 {generation}）。").into()
        }
        GpuDeviceState::Unavailable { generation } => {
            format!("GPU表示を利用できません（世代 {generation}）。アプリを再起動してください。")
                .into()
        }
    }
}

struct ImageUploadProbe {
    service: ImageUploadService,
    status: SharedString,
    current: Option<Arc<desktop_gpui::image_upload::PreparedImage>>,
    cancel: Cancellation,
    manifest_cancel: Cancellation,
    task: Option<Task<()>>,
    request_serial: u64,
    source_revision: u64,
    logical_generation: u64,
    native_state: GpuDeviceState,
    _device_subscription: Subscription,
    base_window_title: String,
    pending: bool,
    /// The last paint result reported to the user. Paint callbacks must not mutate view state,
    /// so a differing message is deferred to the next frame instead of written during paint.
    paint_message: SharedString,
    /// This view's single consumer identity: the frame is evictable only after it detaches.
    consumer: ConsumerId,
}

impl ImageUploadProbe {
    fn start_load(&mut self, next_revision: bool, cx: &mut Context<Self>) {
        self.cancel.cancel();
        if next_revision || self.source_revision == 0 {
            if let Some(previous) = self.current.take() {
                self.service.detach(&previous, self.consumer);
                self.service.discard_unpainted(&previous);
            }
            self.source_revision = self.source_revision.saturating_add(1);
        }
        self.request_serial = self.request_serial.saturating_add(1);
        let serial = self.request_serial;
        let revision = self.source_revision;
        let state = cx.gpu_device_state();
        let GpuDeviceState::Ready { generation } = state else {
            if let Some(previous) = self.current.take() {
                self.service.detach(&previous, self.consumer);
                self.service.discard_unpainted(&previous);
            }
            self.pending = false;
            self.status = backend_reason(state);
            cx.notify();
            return;
        };
        let cancel = Cancellation::default();
        self.cancel = cancel.clone();
        self.pending = true;
        self.status = format!(
            "Fixture revision {revision}: generating PNG for native backend generation {generation}; logical-generation probe is separate"
        )
        .into();

        let request = match fixture_request_for("landmark-png", revision, generation) {
            Ok(request) => request,
            Err(error) => {
                self.pending = false;
                self.status = format!("Fixture unavailable: {error:?}").into();
                cx.notify();
                return;
            }
        };
        let service = self.service.clone();
        let cleanup_service = service.clone();
        let background = cx.background_executor();
        let task = service.prepare_generated(background, request, cancel.clone(), move |cancel| {
            generate_fixture_png(revision, cancel).map(Arc::<[u8]>::from)
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.request_serial != serial
                    || window.gpu_device_state() != (GpuDeviceState::Ready { generation })
                {
                    if let Ok(frame) = result {
                        cleanup_service.discard_unpainted(&frame);
                    }
                    return;
                }
                this.pending = false;
                match result {
                    Ok(frame) => {
                        if this.service.attach(&frame, this.consumer, Some(window.window_handle())) {
                            this.current = Some(frame);
                            this.status = format!(
                                "Fixture revision {revision}, native backend generation {generation}: verified PNG admitted; logical-generation probe={}; tiles are issued only by GPUI paint callbacks",
                                this.logical_generation
                            )
                            .into();
                        } else {
                            this.status =
                                "Fixture unavailable: the prepared frame is no longer retained".into();
                        }
                    }
                    Err(ImageUploadError::Cancelled | ImageUploadError::Decode(MediaError::Cancelled)) => {
                        this.status = "Fixture: cancelled; no complete frame published".into();
                    }
                    Err(error) => {
                        this.status = format!("Fixture unavailable: {error:?}; no complete frame published").into();
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn device_state_changed(
        &mut self,
        state: GpuDeviceState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.native_state == state {
            return;
        }
        let previous_generation = match self.native_state {
            GpuDeviceState::Unsupported => None,
            GpuDeviceState::Ready { generation }
            | GpuDeviceState::Recovering { generation }
            | GpuDeviceState::Unavailable { generation } => Some(generation),
        };
        let next_generation = match state {
            GpuDeviceState::Unsupported => None,
            GpuDeviceState::Ready { generation }
            | GpuDeviceState::Recovering { generation }
            | GpuDeviceState::Unavailable { generation } => Some(generation),
        };
        let invalidated = next_generation.is_some_and(|generation| {
            let advanced = previous_generation.is_none_or(|previous| generation > previous);
            if advanced {
                self.service.invalidate_device_generation(generation);
            }
            advanced
        });
        self.native_state = state;
        self.cancel.cancel();
        self.cancel = Cancellation::default();
        self.request_serial = self.request_serial.saturating_add(1);
        self.pending = false;
        self.task = None;
        if let Some(frame) = self.current.take() {
            self.service.detach(&frame, self.consumer);
            if !invalidated {
                self.service.discard_unpainted(&frame);
            }
        }
        match state {
            GpuDeviceState::Ready { .. } => {
                window.set_window_title(&self.base_window_title);
                self.status = format!(
                    "{} Logical-generation probe={} (not backend identity).",
                    backend_reason(state),
                    self.logical_generation
                )
                .into();
            }
            GpuDeviceState::Recovering { .. } => {
                window.set_window_title(&format!("{} — GPU復旧中", self.base_window_title));
                self.status = backend_reason(state);
            }
            GpuDeviceState::Unavailable { .. } => {
                window.set_window_title(&format!(
                    "{} — GPU表示不可: アプリを再起動してください",
                    self.base_window_title
                ));
                self.status = backend_reason(state);
            }
            GpuDeviceState::Unsupported => {
                window.set_window_title(&format!(
                    "{} — GPUバックエンド未対応",
                    self.base_window_title
                ));
                self.status = backend_reason(state);
            }
        }
        cx.notify();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        self.cancel.cancel();
        self.manifest_cancel.cancel();
        self.request_serial = self.request_serial.saturating_add(1);
        self.pending = false;
        let discarded = self.current.as_ref().is_some_and(|frame| {
            self.service.detach(frame, self.consumer);
            self.service.discard_unpainted(frame)
        });
        if discarded {
            self.current = None;
            self.status =
                "Fixture: cancelled before any GPUI image was issued; admission released".into();
        } else if self.current.take().is_some() {
            self.status = "Fixture: cancelled; an already-issued payload stays retained; GPU completion NOT_MEASURED".into();
        } else {
            self.status = "Fixture: cancelled; no complete frame published".into();
        }
        cx.notify();
    }

    fn reuse_current(&mut self, cx: &mut Context<Self>) {
        let Some(current) = self.current.clone() else {
            self.status = "Reuse unavailable: no prepared image identity".into();
            cx.notify();
            return;
        };
        let reused = self.service.reuse(current.identity());
        match reused {
            Some(reused) if Arc::ptr_eq(&current, &reused) => {
                self.current = Some(reused);
                self.status = "Reuse: exact source/revision/device/content identity shared; no duplicate tile set".into();
            }
            Some(_) => {
                self.status =
                    "Reuse unavailable: registry identity did not resolve to the same frame".into();
            }
            None => {
                self.status = "Reuse unavailable: identity is no longer cached".into();
            }
        }
        cx.notify();
    }

    fn advance_logical_generation(&mut self, cx: &mut Context<Self>) {
        self.cancel.cancel();
        self.request_serial = self.request_serial.saturating_add(1);
        self.pending = false;
        if let Some(previous) = self.current.take() {
            self.service.detach(&previous, self.consumer);
            self.service.discard_unpainted(&previous);
        }
        self.logical_generation = self.logical_generation.saturating_add(1);
        self.cancel = Cancellation::default();
        self.status = format!(
            "Logical-generation probe advanced to {} only; GPUI native backend state remains {:?}; no hardware reset or native identity change",
            self.logical_generation, self.native_state
        )
        .into();
        cx.notify();
    }
    #[cfg(feature = "gpu-recovery-test")]
    fn request_backend_recovery(
        &mut self,
        mode: GpuDeviceRecoveryTest,
        label: &'static str,
        cx: &mut Context<Self>,
    ) {
        self.status = if cx.request_gpu_device_recreation(mode) {
            label.into()
        } else {
            "Test-support backend recreation is unsupported here.".into()
        };
        cx.notify();
    }

    fn backend_controls(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(feature = "gpu-recovery-test")]
        {
            div()
                .flex()
                .gap_2()
                .child(
                    div()
                        .id("gpu-recreate")
                        .role(Role::Button)
                        .aria_label("テスト支援: GPUバックエンドを再作成（物理リセットではありません）")
                        .p_2()
                        .border_1()
                        .child("Backend: Recreate — test-support only; not a physical reset")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.request_backend_recovery(
                                GpuDeviceRecoveryTest::Recreate,
                                "Backend recreation requested; test-support only, not a physical reset.",
                                cx,
                            )
                        })),
                )
                .child(
                    div()
                        .id("gpu-fail")
                        .role(Role::Button)
                        .aria_label("テスト支援: GPUバックエンド再作成の制御失敗（物理リセットではありません）")
                        .p_2()
                        .border_1()
                        .child("Backend: Fail — controlled failure; not a physical reset")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.request_backend_recovery(
                                GpuDeviceRecoveryTest::Fail,
                                "Controlled backend factory failure requested; not a physical reset.",
                                cx,
                            )
                        })),
                )
        }
        #[cfg(not(feature = "gpu-recovery-test"))]
        {
            let _ = cx;
            div().child("Backend recovery controls require --features gpu-recovery-test")
        }
    }

    /// Drive the fixed manifest plus the process-wide quarantine ladder.
    ///
    /// Eviction keeps every in-flight payload quarantined because the pinned GPUI exposes no
    /// completion signal, so the run continues until the 64 MiB process ledger is full: the next
    /// eviction and every further prepare must then be refused with `QuarantineBudgetExhausted`.
    /// The shared identity comes first, so the retention-after-one-detach rule is exercised before
    /// the budget fills. The probe exits non-zero if any expectation is violated.
    fn start_manifest(&mut self, capacity_bytes: u64, cx: &mut Context<Self>) {
        let GpuDeviceState::Ready { generation } = cx.gpu_device_state() else {
            self.status = backend_reason(self.native_state);
            cx.notify();
            return;
        };
        // Must be set before the first render reads the capacity.
        let _ = desktop_gpui::image_upload::set_quarantine_capacity(capacity_bytes);
        let background = cx.background_executor().clone();
        let service = self.service.clone();
        self.manifest_cancel.cancel();
        self.manifest_cancel = Cancellation::default();
        let manifest_cancel = self.manifest_cancel.clone();
        self.pending = true;
        self.status = format!(
            "Manifest: fixed identities use native GPUI generation {generation}; logical probe is separate"
        )
        .into();
        self.task = Some(cx.spawn(async move |this, cx| {
            let mut failures = 0u32;
            let mut evictions = 0u32;
            let mut cap_refusals = 0u32;
            let mut shared_retentions = 0u32;
            // B-03: a frame prepared and attached but never painted is owned through the attach
            // binding, so the close sweep must collect it even though no atlas entry exists yet.
            let mut prepaint_ready = false;
            if let Ok(request) = fixture_request_for("closing", 1, generation) {
                let bytes = background
                    .spawn(async move {
                        let cancel = Cancellation::default();
                        generate_fixture_png(1, &cancel).map(Arc::<[u8]>::from)
                    })
                    .await;
                if let Ok(bytes) = bytes
                    && let Ok(frame) = service
                        .prepare_encoded(&background, request, bytes, Cancellation::default())
                        .await
                {
                    prepaint_ready = this
                        .update_in(cx, |this, window, _| {
                            let handle = window.window_handle();
                            let consumer = ConsumerId::allocate();
                            let attached = service.attach(&frame, consumer, Some(handle));
                            service.detach(&frame, consumer);
                            let _ = this;
                            attached
                        })
                        .unwrap_or(false);
                }
            }
            if !prepaint_ready {
                failures = failures.saturating_add(1);
                println!("{{\"event\":\"img4-prepaint-missing\"}}");
            }
            'outer: for (index, (source, revision)) in MANIFEST.iter().copied().enumerate() {
                // Distinct source/revision identities share the real backend generation; the
                // logical-generation control is not used as a fabricated device identity.
                let request = match fixture_request_for(source, revision, generation) {
                    Ok(request) => request,
                    Err(error) => {
                        failures = failures.saturating_add(1);
                        println!(
                            "{{\"event\":\"img4-setup-failed\",\"index\":{index},\"detail\":\"{error:?}\"}}"
                        );
                        break;
                    }
                };
                let bytes = match background
                    .spawn(async move {
                        let cancel = Cancellation::default();
                        generate_fixture_png(revision, &cancel).map(Arc::<[u8]>::from)
                    })
                    .await
                {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        failures = failures.saturating_add(1);
                        println!(
                            "{{\"event\":\"img4-setup-failed\",\"index\":{index},\"detail\":\"{error:?}\"}}"
                        );
                        break;
                    }
                };
                let frame = match service
                    .prepare_encoded(&background, request, bytes, Cancellation::default())
                    .await
                {
                    Ok(frame) => frame,
                    Err(ImageUploadError::QuarantineBudgetExhausted { retained, cap }) => {
                        cap_refusals = cap_refusals.saturating_add(1);
                        println!(
                            "{{\"event\":\"img4-prepare-refused\",\"index\":{index},\"reason\":\"process_quarantine\",\"retained\":{retained},\"cap\":{cap}}}"
                        );
                        break 'outer;
                    }
                    Err(ImageUploadError::Admission(media::UploadRefusal::BudgetExceeded {
                        ..
                    })) => {
                        // The retained, unproven payloads already occupy the pool's staging and
                        // in-flight allowance, so new work is refused rather than releasing them.
                        cap_refusals = cap_refusals.saturating_add(1);
                        println!(
                            "{{\"event\":\"img4-prepare-refused\",\"index\":{index},\"reason\":\"pool_budget\",\"quarantine_ledger\":{}}}",
                            service.counters().quarantined_payload_bytes
                        );
                        break 'outer;
                    }
                    Err(error) => {
                        failures = failures.saturating_add(1);
                        println!(
                            "{{\"event\":\"img4-setup-failed\",\"index\":{index},\"detail\":\"{error:?}\"}}"
                        );
                        break;
                    }
                };
                // The first identity is presented by two consumers; a repeated identity would be
                // refused by the pool while its payload is quarantined (unreclaimed key).
                let shared = index == 0;
                let consumers = [ConsumerId::allocate(), ConsumerId::allocate()];
                let attached = this
                    .update_in(cx, |this, window, cx| {
                        let handle = window.window_handle();
                        let mut attached = service.attach(&frame, consumers[0], Some(handle));
                        if shared {
                            attached &= service.attach(&frame, consumers[1], Some(handle));
                        }
                        if attached {
                            this.current = Some(frame.clone());
                            this.source_revision = revision;
                            cx.notify();
                        }
                        attached
                    })
                    .unwrap_or(false);
                if !attached {
                    failures = failures.saturating_add(1);
                    println!(
                        "{{\"event\":\"img4-attach-failed\",\"index\":{index}}}"
                    );
                    break;
                }
                // Wait for the frame's own scene-composed flag instead of a fixed sleep. GPU
                // Present stays unmeasured.
                let mut painted = false;
                for _ in 0..40 {
                    if frame.progress().complete_in_scene {
                        painted = true;
                        break;
                    }
                    background.timer(Duration::from_millis(25)).await;
                }
                if !painted {
                    failures = failures.saturating_add(1);
                    println!("{{\"event\":\"img4-paint-missing\",\"index\":{index}}}");
                }
                let refusal = this.update_in(cx, |_, window, _| service.evict(&frame, window));
                if !matches!(refusal, Ok(Err(ImageUploadError::ConsumersRemain))) {
                    failures = failures.saturating_add(1);
                    println!("{{\"event\":\"img4-consumer-gate-failed\",\"index\":{index}}}");
                }
                if shared {
                    let detached = this
                        .update(cx, |_, _| service.detach(&frame, consumers[1]))
                        .unwrap_or(false);
                    let refusal = this.update_in(cx, |_, window, _| service.evict(&frame, window));
                    match refusal {
                        Ok(Err(ImageUploadError::ConsumersRemain)) => {
                            shared_retentions = shared_retentions.saturating_add(1);
                            println!(
                                "{{\"event\":\"img4-shared-retained\",\"index\":{index},\"detached\":{detached},\"consumers\":{}}}",
                                service.consumers(&frame)
                            );
                        }
                        _ => {
                            failures = failures.saturating_add(1);
                            println!(
                                "{{\"event\":\"img4-shared-failed\",\"index\":{index},\"detached\":{detached}}}"
                            );
                        }
                    }
                }
                let evicted = this.update_in(cx, |this, window, cx| {
                    service.detach(&frame, consumers[0]);
                    this.current = None;
                    cx.notify();
                    service.evict(&frame, window)
                });
                match evicted {
                    Ok(Ok(outcome)) => {
                        evictions = evictions.saturating_add(1);
                        println!(
                            "{{\"event\":\"img4-evict\",\"index\":{index},\"source\":\"{source}\",\"revision\":{revision},\"shared\":{shared},\"painted\":{painted},\"drop_calls\":{},\"released_bytes\":{},\"quarantined_bytes\":{},\"retained\":{},\"quarantine_ledger\":{}}}",
                            outcome.drop_calls,
                            outcome.released_bytes,
                            outcome.quarantined_bytes,
                            service.counters().retained_frames,
                            service.counters().quarantined_payload_bytes,
                        );
                    }
                    Ok(Err(ImageUploadError::QuarantineBudgetExhausted { retained, cap })) => {
                        cap_refusals = cap_refusals.saturating_add(1);
                        println!(
                            "{{\"event\":\"img4-evict-refused-cap\",\"index\":{index},\"retained\":{retained},\"cap\":{cap},\"frames\":{}}}",
                            service.counters().retained_frames
                        );
                        break 'outer;
                    }
                    Ok(Err(error)) => {
                        failures = failures.saturating_add(1);
                        println!(
                            "{{\"event\":\"img4-evict-refused-unexpected\",\"index\":{index},\"error\":\"{error:?}\"}}"
                        );
                        break;
                    }
                    Err(_) => {
                        failures = failures.saturating_add(1);
                        break;
                    }
                }
            }
            if manifest_cancel.is_cancelled() {
                // A native device transition or explicit cancel ended this run; the diagnostic
                // expectations no longer apply and no result is claimed.
                return;
            }
            let counters = service.counters();
            // Structural expectations: at least one eviction quarantined a payload, the very next
            // prepare was refused because of the retained bytes, the shared identity survived one
            // detach, and nothing unproven was released.
            let ledger_positive = counters.quarantined_payload_bytes > 0;
            if evictions < 1
                || cap_refusals != 1
                || shared_retentions != 1
                || !ledger_positive
                || counters.quarantine_capacity_bytes != capacity_bytes
                || failures != 0
            {
                failures = failures.saturating_add(1);
            }
            println!(
                "{{\"event\":\"img4-quarantine\",\"evictions\":{evictions},\"cap_refusals\":{cap_refusals},\"shared_retentions\":{shared_retentions},\"ledger\":{},\"capacity\":{},\"retained\":{}}}",
                counters.quarantined_payload_bytes,
                counters.quarantine_capacity_bytes,
                counters.retained_frames,
            );
            let swept = this.update_in(cx, |this, window, cx| {
                let summary = this.close_with_sweep(window, cx);
                (summary, service.counters())
            });
            match swept {
                Ok((summary, counters)) => {
                    if !prepaint_ready
                        || summary.evicted_frames != 1
                        || summary.skipped_frames != 0
                        || summary.unattributed_frames != 0
                    {
                        failures = failures.saturating_add(1);
                    }
                    println!(
                        "{{\"event\":\"img4-close\",\"prepaint\":{prepaint_ready},\"evicted\":{},\"skipped\":{},\"unattributed\":{},\"retained\":{},\"consumers\":{},\"quarantine_ledger\":{},\"failures\":{failures}}}",
                        summary.evicted_frames,
                        summary.skipped_frames,
                        summary.unattributed_frames,
                        counters.retained_frames,
                        counters.consumers,
                        counters.quarantined_payload_bytes,
                    );
                }
                Err(_) => {
                    failures = failures.saturating_add(1);
                }
            }
            if failures > 0 {
                std::process::exit(1);
            }
        }));
        cx.notify();
    }

    /// Evict the frame this view presents: detach first, then release the atlas entries and the
    /// CPU payload. A refusal re-attaches the frame so nothing is lost silently.
    fn evict_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(frame) = self.current.clone() else {
            self.status = "Evict unavailable: no prepared image identity".into();
            cx.notify();
            return;
        };
        self.service.detach(&frame, self.consumer);
        match self.service.evict(&frame, window) {
            Ok(outcome) => {
                self.current = None;
                self.status = format!(
                    "Evicted: atlas images={} released_staged={} B quarantined={} B (in-flight transfer stays retained; no measured termination); GPU residency NOT_MEASURED",
                    outcome.drop_calls,
                    outcome.released_bytes,
                    outcome.quarantined_bytes,
                )
                .into();
            }
            Err(error) => {
                self.service
                    .attach(&frame, self.consumer, Some(window.window_handle()));
                self.status =
                    format!("Eviction refused: {error:?}; the frame stays retained and displayed")
                        .into();
            }
        }
        cx.notify();
    }

    /// Close the window through the ownership sweep: every frame this window owns with no
    /// consumer is evicted before the window disappears.
    fn close_with_sweep(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> desktop_gpui::image_upload::EvictionSummary {
        if let Some(frame) = self.current.take() {
            self.service.detach(&frame, self.consumer);
        }
        let summary = self.service.evict_owned_by(window);
        let counters = self.service.counters();
        eprintln!(
            "event=img4-window-close evicted={} skipped={} unattributed={} images={} released_bytes={} retained={} consumers={} resident={} quarantined={}",
            summary.evicted_frames,
            summary.skipped_frames,
            summary.unattributed_frames,
            summary.drop_calls,
            summary.released_bytes,
            counters.retained_frames,
            counters.consumers,
            counters.resident_reservation_bytes,
            counters.quarantined_staging_bytes
                + counters.quarantined_in_flight_bytes
                + counters.quarantined_resident_bytes
        );
        cx.quit();
        summary
    }
}

impl Render for ImageUploadProbe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.current.clone();
        let cancel = self.cancel.clone();
        let current_revision = self.source_revision;
        let current_generation = current
            .as_ref()
            .map_or(0, |frame| frame.identity().device_generation);
        let native_state = self.native_state;
        let native_line: SharedString = format!(
            "Native GPUI backend: {:?}; logical-generation probe counter: {} (not backend identity)",
            native_state, self.logical_generation
        )
        .into();
        let service = self.service.clone();
        let counters = service.counters();
        let frame_line: SharedString = match &current {
            Some(frame) => {
                let progress = frame.progress();
                format!(
                    "prepared revision={} generation={} tiles={}/{} scene_complete={} (scene assembly only)",
                    progress.source_revision,
                    progress.device_generation,
                    progress.issued_tiles,
                    progress.tile_count,
                    progress.complete_in_scene
                )
                .into()
            }
            None => "No current prepared frame".into(),
        };
        let counters_line: SharedString = format!(
            "retained_frames={} | reservations staging={} in_flight={} resident={} B | period={} reserved={}/{} B | first-paint-issued={} B / {} tiles | repaint-calls={} (cache hits under the generation contract) | GPU completion, residency, Present=NOT_MEASURED",
            counters.retained_frames,
            counters.staging_reservation_bytes,
            counters.in_flight_reservation_bytes,
            counters.resident_reservation_bytes,
            counters.period_index,
            counters.reserved_bytes_this_period,
            counters.period_capacity_bytes,
            counters.paint_issued_bytes,
            counters.paint_issued_tiles,
            counters.repaint_calls,
        )
        .into();
        let status = self.status.clone();
        let pending = self.pending;
        let last_paint_message = self.paint_message.clone();
        let probe = cx.entity().downgrade();
        let backend_controls = self.backend_controls(cx);
        // The canvas draws inside a sized wrapper: GPUI's canvas is a leaf whose own style is not
        // given a size here, and the wrapper owns the 1:1 display box instead.
        let frame_canvas = div()
            .flex_none()
            .w(px(DISPLAY_EDGE))
            .h(px(DISPLAY_EDGE))
            .child(
                canvas(
                    |_bounds, _, _| (),
                    move |bounds, (), window, app| {
                if let Some(frame) = &current {
                    let result = frame.paint(
                        bounds,
                        current_revision,
                        current_generation,
                        &cancel,
                        window,
                    );
                    let message = match result {
                        Ok(PaintStatus::Pending { issued_tiles, tile_count }) => {
                            Some(format!("Paint pending: {issued_tiles}/{tile_count} whole tiles issued; waiting for process allowance"))
                        }
                        Ok(PaintStatus::CompleteInScene { tile_count }) => {
                            Some(format!("Complete in GPUI scene: {tile_count} tiles; GPU completion/residency/Present NOT_MEASURED"))
                        }
                        Ok(PaintStatus::Cancelled) => Some("Paint cancelled; partial frame remains covered".to_owned()),
                        Ok(PaintStatus::StaleRevision { requested, current }) => {
                            Some(format!("Paint refused stale source revision {requested}; current={current}"))
                        }
                        Ok(PaintStatus::StaleDeviceGeneration { requested, current }) => {
                            Some(format!("Paint refused stale native device generation {requested}; current={current}"))
                        }
                        Ok(PaintStatus::DeviceUnavailable) => {
                            Some(match window.gpu_device_state() {
                                GpuDeviceState::Unsupported => "GPU表示を利用できません: このバックエンドでは画像表示が未対応です。".to_owned(),
                                GpuDeviceState::Recovering { generation } => format!("GPUデバイスを復旧中です（世代 {generation}）。"),
                                GpuDeviceState::Unavailable { generation } => format!("GPU表示を利用できません（世代 {generation}）。アプリを再起動してください。"),
                                GpuDeviceState::Ready { generation } => format!("GPU世代 {generation} の描画リソースは利用できません。"),
                            })
                        }
                        Ok(PaintStatus::EmptyBounds) => None,
                        Ok(PaintStatus::DifferentWindow) => {
                            Some("Paint refused: GPUI image atlas is owned by a different window".to_owned())
                        }
                        Ok(PaintStatus::AdmissionUnavailable) => {
                            Some("Paint deferred: process allowance or upload admission unavailable".to_owned())
                        }
                        Ok(PaintStatus::Evicted) => {
                            Some("Paint refused: the frame was evicted and is no longer retained".to_owned())
                        }
                        Err(error) => Some(format!("GPUI paint unavailable: {error}")),
                    };
                    if let Some(message) = message
                        && message.as_str() != last_paint_message.as_ref()
                    {
                        // Paint must not mutate view state: report the result on the next frame.
                        let message = SharedString::from(message);
                        window.defer(app, move |_, app| {
                            let _ = probe.update(app, |this, cx| {
                                this.paint_message = message.clone();
                                this.status = message;
                                cx.notify();
                            });
                        });
                    }
                } else {
                    window.paint_quad(fill(bounds, rgb(0x141820)));
                }
            },
                )
                .size_full(),
            );

        div()
            .id("img3-upload-probe")
            .role(Role::Application)
            .aria_label("MP-IMG3 explicit local fixture image upload feasibility")
            .size_full()
            .p_4()
            .bg(rgb(0x20242a))
            .text_color(rgb(0xf0f0f0))
            .flex()
            .flex_col()
            .gap_3()
            .child("MP-IMG3-S2 — explicit in-memory Fixture; no Runtime or product import")
            .child(native_line)
            .child(backend_controls)
            .child(div().flex().flex_row().gap_2()
                .child(button("Load current revision", !pending, cx.listener(|this, _, _, cx| this.start_load(false, cx))))
                .child(button("Load next revision", !pending, cx.listener(|this, _, _, cx| this.start_load(true, cx))))
                .child(button("Reuse exact identity", !pending, cx.listener(|this, _, _, cx| this.reuse_current(cx))))
                .child(button("Cancel", true, cx.listener(|this, _, _, cx| this.cancel(cx))))
                .child(button("Advance logical-generation probe", true, cx.listener(|this, _, _, cx| this.advance_logical_generation(cx))))
                .child(button("Evict current", !pending, cx.listener(|this, _, window, cx| this.evict_current(window, cx))))
                .child(button("Evict owned and close", true, cx.listener(|this, _, window, cx| { let _ = this.close_with_sweep(window, cx); }))))
            .child(status)
            .child(div().flex().flex_row().gap_4()
                .child(frame_canvas)
                .child(div().flex().flex_col().gap_3()
                    .child("Expected independent landmarks at source pixels:")
                    .child("(256,256) red · (1792,256) green")
                    .child("(256,1792) blue · (1792,1792) yellow")
                    .child("(1024,1024) black cross · (160,160) revision marker")
                    .child("Revision 1 marker: magenta; revision 2: cyan; then alternate.")
                    .child(frame_line)
                    .child(counters_line)
                    .child("Each 512×512 tile: 2,048-byte packed/aligned row pitch; 1 MiB. Shared 2 MiB / 8.333 ms process allowance; GPUI animation frames request continuation, no per-pane timer.")
                    .child("Complete scene means all whole tiles were issued and composed via Window::paint_image; it does NOT mean GPU completion or residency.")))
    }
}

fn button(
    label: &'static str,
    enabled: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(label)
        .role(Role::Button)
        .aria_label(label)
        .p_2()
        .border_1()
        .border_color(rgb(0xd9b76b))
        .opacity(if enabled { 1.0 } else { 0.45 })
        .child(label)
        .on_click(on_click)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = match args.next().as_deref() {
        Some("--fixture") if args.next().is_none() => Mode::Fixture,
        Some("--manifest") => Mode::Manifest(
            args.next()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(32)
                .saturating_mul(1024 * 1024),
        ),
        _ => {
            eprintln!(
                "Usage: cargo run --locked -p cos-desktop --example image_upload -- --fixture\n       cargo run --locked -p cos-desktop --example image_upload -- --manifest [capacity_mib]"
            );
            std::process::exit(2);
        }
    };
    gpui_platform::application().run(move |cx: &mut App| {
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let bounds = Bounds::centered(None, size(px(1120.0), px(790.0)), cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("MP-IMG3 Fixture upload feasibility".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                let probe = cx.new(|cx| {
                    let native_state = cx.gpu_device_state();
                    let base_window_title =
                        "MP-IMG3 Fixture upload feasibility".to_owned();
                    let window_handle = window.window_handle();
                    let weak = cx.entity().downgrade();
                    let device_subscription = cx.on_gpu_device_state_change(move |app| {
                        let state = app.gpu_device_state();
                        let _ = window_handle.update(app, |_, window, app| {
                            let _ = weak.update(app, |probe: &mut ImageUploadProbe, cx| {
                                probe.device_state_changed(state, window, cx);
                            });
                        });
                    });
                    let status = match native_state {
                        GpuDeviceState::Ready { generation } => format!(
                            "Fixture mode only. Native GPUI generation {generation}; logical-generation probe is separate."
                        )
                        .into(),
                        state => backend_reason(state),
                    };
                    ImageUploadProbe {
                        service: ImageUploadService::global(),
                        status,
                        current: None,
                        cancel: Cancellation::default(),
                        manifest_cancel: Cancellation::default(),
                        task: None,
                        request_serial: 0,
                        source_revision: 0,
                        logical_generation: 0,
                        native_state,
                        _device_subscription: device_subscription,
                        base_window_title,
                        pending: false,
                        paint_message: SharedString::default(),
                        consumer: ConsumerId::allocate(),
                    }
                });
                if let Mode::Manifest(capacity_bytes) = mode {
                    probe.update(cx, |this, cx| this.start_manifest(capacity_bytes, cx));
                }
                probe
            },
        );
        if opened.is_err() {
            eprintln!("Native feasibility window unavailable");
            cx.quit();
        }
        cx.activate(true);
    });
    eprintln!("event=img3-fixture-probe-exit Runtime-not-started");
}
