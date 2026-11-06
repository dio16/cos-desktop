//! Explicit deterministic-fixture feasibility probe for MP-IMG3-S2; this is not Runtime import.
//!
//! Run: `cargo run --locked -p cos-desktop --example image_upload -- --fixture`
//! The fixture is generated in memory on GPUI's background executor, then passes through
//! `media::decode_for_upload` and the reusable desktop-gpui whole-image tile path. No Runtime,
//! artifact success, GPU completion, Present or residency result is fabricated.
use std::sync::Arc;
use std::time::Duration;

use desktop_gpui::image_upload::{
    ConsumerId, FRAME_EDGE, ImageUploadError, ImageUploadService, PaintStatus, UploadRequest,
};
use gpui::prelude::*;
use gpui::{
    App, AppContext, Bounds, Context, IntoElement, Render, Role, SharedString, Task,
    TitlebarOptions, Window, WindowBounds, WindowOptions, canvas, div, fill, px, rgb, size,
};
use media::{
    Cancellation, DecodeBudget, DecodePolicy, LocalId, LocalProvenance, MediaError, Orientation,
    PixelSize, RepresentationKey,
};

const MAX_FIXTURE_ENCODED_BYTES: usize = 16 * 1024 * 1024;
const DISPLAY_EDGE: f32 = 480.0;
/// Fixed manifest for the eviction loop: two revisions of one source, two further sources, and one
/// identity repeated so a single frame is attached by two consumers.
const MANIFEST: [(&str, u64); 6] = [
    ("alpha", 1),
    ("alpha", 2),
    ("beta", 1),
    ("gamma", 1),
    ("shared", 1),
    ("shared", 1),
];

#[derive(Clone, Copy)]
enum Mode {
    Fixture,
    Manifest(u32),
}

struct ImageUploadProbe {
    service: ImageUploadService,
    status: SharedString,
    current: Option<Arc<desktop_gpui::image_upload::PreparedImage>>,
    cancel: Cancellation,
    task: Option<Task<()>>,
    request_serial: u64,
    source_revision: u64,
    device_generation: u64,
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
        let generation = self.device_generation;
        let cancel = Cancellation::default();
        self.cancel = cancel.clone();
        self.pending = true;
        self.status = format!(
            "Fixture revision {revision}: generating PNG, decoding, and converting on background worker"
        )
        .into();

        let request = match fixture_request(revision, generation) {
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
            let _ = this.update(cx, |this, cx| {
                if this.request_serial != serial {
                    if let Ok(frame) = result {
                        cleanup_service.discard_unpainted(&frame);
                    }
                    return;
                }
                this.pending = false;
                match result {
                    Ok(frame) => {
                        // The view presents this frame: eviction must wait for this consumer.
                        if this.service.attach(&frame, this.consumer) {
                            this.current = Some(frame);
                            this.status = format!(
                                "Fixture revision {revision}, device generation {generation}: verified PNG admitted; tiles are issued only by GPUI paint callbacks"
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

    fn cancel(&mut self, cx: &mut Context<Self>) {
        self.cancel.cancel();
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
        // A frame whose tiles were already issued cannot be released, so it stays in the view and
        // its next paint reports the stale-generation refusal instead of silently disappearing.
        if let Some(previous) = self.current.clone() {
            self.service.detach(&previous, self.consumer);
            if self.service.discard_unpainted(&previous) {
                self.current = None;
            }
        }
        self.device_generation = self.device_generation.saturating_add(1);
        // Cancel any in-flight preparation, but give the retained frame a live token so its next
        // paint reports the stale-generation refusal rather than a cancellation.
        self.cancel = Cancellation::default();
        self.status = format!(
            "Logical device generation advanced to {} (no hardware device reset performed); previous frame is now stale",
            self.device_generation
        )
        .into();
        cx.notify();
    }

    /// Drive the fixed manifest: load → present → switch → evict → load another, repeated.
    ///
    /// Every identity comes from `MANIFEST`; the last two entries share one (source, revision) so a
    /// shared frame is attached by two consumers and must survive the first detach. Counters and
    /// outcomes are printed as JSON lines so a harness can bind them to an execution.
    fn start_manifest(&mut self, cycles: u32, cx: &mut Context<Self>) {
        let background = cx.background_executor().clone();
        let service = self.service.clone();
        self.pending = true;
        self.status = format!(
            "Manifest: {cycles} cycles over {} fixed identities (one shared by two consumers)",
            MANIFEST.len()
        )
        .into();
        self.task = Some(cx.spawn(async move |this, cx| {
            for cycle in 0..cycles {
                for (index, (source, revision)) in MANIFEST.iter().copied().enumerate() {
                    let generation = this
                        .update(cx, |this, _| this.device_generation)
                        .unwrap_or(1);
                    let Ok(request) = fixture_request_for(source, revision, generation) else {
                        continue;
                    };
                    let bytes = background
                        .spawn(async move {
                            let cancel = Cancellation::default();
                            generate_fixture_png(revision, &cancel).map(Arc::<[u8]>::from)
                        })
                        .await;
                    let Ok(bytes) = bytes else { continue };
                    let Ok(frame) = service
                        .prepare_encoded(&background, request, bytes, Cancellation::default())
                        .await
                    else {
                        continue;
                    };
                    let shared = MANIFEST
                        .get(index + 1)
                        .is_some_and(|next| *next == (source, revision));
                    let consumers = [ConsumerId::new(1), ConsumerId::new(2)];
                    let attached = this
                        .update(cx, |this, cx| {
                            let mut attached = service.attach(&frame, consumers[0]);
                            if shared {
                                attached &= service.attach(&frame, consumers[1]);
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
                        continue;
                    }
                    // Failure injection 1: eviction while a consumer is still attached must refuse.
                    let attached_consumers = service.consumers(&frame);
                    let refusal = this.update_in(cx, |this, window, _| {
                        let _ = this;
                        service.evict(&frame, window)
                    });
                    match refusal {
                        Ok(Ok(_)) => println!(
                            "{{\"event\":\"img4-evict-unexpected\",\"cycle\":{cycle},\"index\":{index}}}"
                        ),
                        Ok(Err(error)) => println!(
                            "{{\"event\":\"img4-evict-refused\",\"cycle\":{cycle},\"index\":{index},\"error\":\"{error:?}\",\"attached_consumers\":{attached_consumers},\"retained\":{}}}",
                            service.counters().retained_frames,
                        ),
                        Err(_) => println!(
                            "{{\"event\":\"img4-evict-refused\",\"cycle\":{cycle},\"index\":{index},\"error\":\"view_gone\"}}"
                        ),
                    }
                    // Let the canvas paint the frame at least once before it is switched away.
                    background.timer(Duration::from_millis(120)).await;
                    let evicted = this.update_in(cx, |this, window, cx| {
                        service.detach(&frame, consumers[0]);
                        if shared {
                            service.detach(&frame, consumers[1]);
                        }
                        this.current = None;
                        cx.notify();
                        let outcome = service.evict(&frame, window);
                        (outcome, service.counters(), this.device_generation)
                    });
                    match evicted {
                        Ok((Ok(outcome), counters, generation)) => println!(
                            "{{\"event\":\"img4-evict\",\"cycle\":{cycle},\"index\":{index},\"source\":\"{source}\",\"revision\":{revision},\"generation\":{generation},\"shared\":{shared},\"images\":{},\"released_bytes\":{},\"retained\":{},\"consumers\":{},\"staging\":{},\"in_flight\":{},\"resident\":{},\"quarantined\":{},\"evicted_frames\":{},\"dropped_images\":{},\"released_total\":{}}}",
                            outcome.dropped_images,
                            outcome.released_bytes,
                            counters.retained_frames,
                            counters.consumers,
                            counters.staging_reservation_bytes,
                            counters.in_flight_reservation_bytes,
                            counters.resident_reservation_bytes,
                            counters.quarantined_staging_bytes
                                + counters.quarantined_in_flight_bytes
                                + counters.quarantined_resident_bytes,
                            counters.evicted_frames,
                            counters.dropped_atlas_images,
                            counters.released_payload_bytes,
                        ),
                        Ok((Err(error), counters, generation)) => println!(
                            "{{\"event\":\"img4-evict-refused\",\"cycle\":{cycle},\"index\":{index},\"source\":\"{source}\",\"revision\":{revision},\"generation\":{generation},\"error\":\"{error:?}\",\"retained\":{},\"consumers\":{}}}",
                            counters.retained_frames, counters.consumers,
                        ),
                        Err(_) => break,
                    }
                }
                // One logical generation change per cycle: the previous pool must not survive and
                // the next pass must recreate resources lazily.
                let _ = this.update(cx, |this, cx| this.advance_logical_generation(cx));
                let counters = service.counters();
                println!(
                    "{{\"event\":\"img4-cycle\",\"cycle\":{cycle},\"retained\":{},\"consumers\":{},\"staging\":{},\"in_flight\":{},\"resident\":{},\"quarantined\":{},\"evicted_frames\":{},\"dropped_images\":{},\"released_bytes\":{}}}",
                    counters.retained_frames,
                    counters.consumers,
                    counters.staging_reservation_bytes,
                    counters.in_flight_reservation_bytes,
                    counters.resident_reservation_bytes,
                    counters.quarantined_staging_bytes
                        + counters.quarantined_in_flight_bytes
                        + counters.quarantined_resident_bytes,
                    counters.evicted_frames,
                    counters.dropped_atlas_images,
                    counters.released_payload_bytes,
                );
            }
            // Failure injection 2: two retained frames fill the bound, so a third is refused.
            let generation = this
                .update(cx, |this, _| this.device_generation)
                .unwrap_or(1);
            let mut held = Vec::new();
            for source in ["cap-a", "cap-b"] {
                let Ok(request) = fixture_request_for(source, 1, generation) else {
                    continue;
                };
                let bytes = background
                    .spawn(async move {
                        let cancel = Cancellation::default();
                        generate_fixture_png(1, &cancel).map(Arc::<[u8]>::from)
                    })
                    .await;
                let Ok(bytes) = bytes else { continue };
                if let Ok(frame) = service
                    .prepare_encoded(&background, request, bytes, Cancellation::default())
                    .await
                {
                    held.push(frame);
                }
            }
            let third = fixture_request_for("cap-c", 1, generation).ok();
            let third_result = match third {
                Some(request) => {
                    let bytes = background
                        .spawn(async move {
                            let cancel = Cancellation::default();
                            generate_fixture_png(1, &cancel).map(Arc::<[u8]>::from)
                        })
                        .await;
                    match bytes {
                        Ok(bytes) => service
                            .prepare_encoded(&background, request, bytes, Cancellation::default())
                            .await
                            .map(|_| ()),
                        Err(error) => Err(ImageUploadError::Decode(error)),
                    }
                }
                None => Err(ImageUploadError::InvalidEncodedInputLimit),
            };
            println!(
                "{{\"event\":\"img4-capacity\",\"held\":{},\"third\":\"{}\",\"retained\":{}}}",
                held.len(),
                match &third_result {
                    Ok(()) => "admitted".to_owned(),
                    Err(error) => format!("{error:?}"),
                },
                service.counters().retained_frames,
            );
            let _ = this.update_in(cx, |_, window, _| {
                for frame in &held {
                    let _ = service.evict(frame, window);
                }
            });
            // One frame is deliberately left retained so the close sweep has something to release.
            let generation = this
                .update(cx, |this, _| this.device_generation)
                .unwrap_or(1);
            if let (Ok(request), Ok(bytes)) = (
                fixture_request_for("closing", 1, generation),
                background
                    .spawn(async move {
                        let cancel = Cancellation::default();
                        generate_fixture_png(1, &cancel).map(Arc::<[u8]>::from)
                    })
                    .await,
            ) && let Ok(frame) = service
                .prepare_encoded(&background, request, bytes, Cancellation::default())
                .await
            {
                let attached = this
                    .update(cx, |this, cx| {
                        let attached = service.attach(&frame, ConsumerId::new(1));
                        if attached {
                            this.current = Some(frame);
                            cx.notify();
                        }
                        attached
                    })
                    .unwrap_or(false);
                if attached {
                    background.timer(Duration::from_millis(120)).await;
                }
            }
            let summary = this
                .update_in(cx, |this, window, cx| {
                    this.close_with_sweep(window, cx);
                    let counters = service.counters();
                    println!(
                        "{{\"event\":\"img4-summary\",\"retained\":{},\"consumers\":{},\"resident\":{},\"quarantined\":{},\"generation\":{}}}",
                        counters.retained_frames,
                        counters.consumers,
                        counters.resident_reservation_bytes,
                        counters.quarantined_staging_bytes
                            + counters.quarantined_in_flight_bytes
                            + counters.quarantined_resident_bytes,
                        this.device_generation,
                    );
                });
            let _ = summary;
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
                    "Evicted: atlas images={} released_payload={} B out of {} B; GPU residency NOT_MEASURED",
                    outcome.dropped_images,
                    outcome.released_bytes,
                    FRAME_EDGE as u64 * FRAME_EDGE as u64 * 4
                )
                .into();
            }
            Err(error) => {
                self.service.attach(&frame, self.consumer);
                self.status =
                    format!("Eviction refused: {error:?}; the frame stays retained and displayed")
                        .into();
            }
        }
        cx.notify();
    }

    /// Close the window through the ownership sweep: every frame this window owns with no
    /// consumer is evicted before the window disappears.
    fn close_with_sweep(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(frame) = self.current.take() {
            self.service.detach(&frame, self.consumer);
        }
        let summary = self.service.evict_owned_by(window);
        let counters = self.service.counters();
        eprintln!(
            "event=img4-window-close evicted={} skipped={} images={} released_bytes={} retained={} consumers={} resident={} quarantined={}",
            summary.evicted_frames,
            summary.skipped_frames,
            summary.dropped_images,
            summary.released_bytes,
            counters.retained_frames,
            counters.consumers,
            counters.resident_reservation_bytes,
            counters.quarantined_staging_bytes
                + counters.quarantined_in_flight_bytes
                + counters.quarantined_resident_bytes
        );
        cx.quit();
    }
}

impl Render for ImageUploadProbe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.current.clone();
        let cancel = self.cancel.clone();
        let current_revision = self.source_revision;
        let current_generation = self.device_generation;
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
                            Some(format!("Paint refused stale device generation {requested}; current={current}"))
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
            .child(div().flex().flex_row().gap_2()
                .child(button("Load current revision", !pending, cx.listener(|this, _, _, cx| this.start_load(false, cx))))
                .child(button("Load next revision", !pending, cx.listener(|this, _, _, cx| this.start_load(true, cx))))
                .child(button("Reuse exact identity", !pending, cx.listener(|this, _, _, cx| this.reuse_current(cx))))
                .child(button("Cancel", true, cx.listener(|this, _, _, cx| this.cancel(cx))))
                .child(button("Advance logical generation", true, cx.listener(|this, _, _, cx| this.advance_logical_generation(cx))))
                .child(button("Evict current", !pending, cx.listener(|this, _, window, cx| this.evict_current(window, cx))))
                .child(button("Evict owned and close", true, cx.listener(|this, _, window, cx| this.close_with_sweep(window, cx)))))
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

fn fixture_request(revision: u64, generation: u64) -> Result<UploadRequest, MediaError> {
    fixture_request_for("landmark-png", revision, generation)
}

/// One request for a named fixture source: the manifest uses distinct source ids so each entry is
/// a distinct identity, and a repeated (source, revision) pair is a genuinely shared frame.
fn fixture_request_for(
    source: &str,
    revision: u64,
    generation: u64,
) -> Result<UploadRequest, MediaError> {
    let key = RepresentationKey {
        source: media::RepresentationSource::Local {
            provenance: LocalProvenance::Fixture {
                fixture: LocalId::new("img3-s2")?,
                manifest: LocalId::new("generated-v1")?,
                source: LocalId::new(source)?,
            },
            revision,
        },
        kind: desktop_protocol::RepresentationKind::Preview,
        size: PixelSize::new(FRAME_EDGE, FRAME_EDGE)?,
        orientation: Orientation::Normal,
        policy: DecodePolicy {
            decoder: LocalId::new("media-image-png-rgb8")?,
            colour: LocalId::new("srgb-bytes-no-transform")?,
            generation: 1,
        },
    };
    Ok(UploadRequest {
        key,
        source_revision: revision,
        device_generation: generation,
        current_device_generation: generation,
        decode_limits: DecodeBudget::ratified_default().limits()?,
        max_encoded_bytes: MAX_FIXTURE_ENCODED_BYTES,
    })
}

fn generate_fixture_png(revision: u64, cancel: &Cancellation) -> Result<Vec<u8>, MediaError> {
    let row_len = (FRAME_EDGE as usize)
        .checked_mul(3)
        .and_then(|bytes| bytes.checked_add(1))
        .ok_or(MediaError::TooLarge)?;
    let raw_len = row_len
        .checked_mul(FRAME_EDGE as usize)
        .ok_or(MediaError::TooLarge)?;
    let mut raw = Vec::new();
    raw.try_reserve_exact(raw_len)
        .map_err(|_| MediaError::Allocation)?;
    let revision_color = if revision % 2 == 1 {
        [255, 0, 255]
    } else {
        [0, 255, 255]
    };
    for y in 0..FRAME_EDGE {
        if cancel.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        raw.push(0); // PNG filter None.
        for x in 0..FRAME_EDGE {
            let rgb = if (128..256).contains(&x) && (128..256).contains(&y) {
                revision_color
            } else if (1016..1032).contains(&x) || (1016..1032).contains(&y) {
                [0, 0, 0]
            } else if x < FRAME_EDGE / 2 && y < FRAME_EDGE / 2 {
                [255, 0, 0]
            } else if x >= FRAME_EDGE / 2 && y < FRAME_EDGE / 2 {
                [0, 255, 0]
            } else if x < FRAME_EDGE / 2 {
                [0, 0, 255]
            } else {
                [255, 255, 0]
            };
            raw.extend_from_slice(&rgb);
        }
    }
    if cancel.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    let blocks = raw.len().div_ceil(u16::MAX as usize);
    let deflate_len = 2usize
        .checked_add(raw.len())
        .and_then(|len| len.checked_add(blocks.checked_mul(5)?))
        .and_then(|len| len.checked_add(4))
        .ok_or(MediaError::TooLarge)?;
    let mut idat = Vec::new();
    idat.try_reserve_exact(deflate_len)
        .map_err(|_| MediaError::Allocation)?;
    idat.extend_from_slice(&[0x78, 0x01]); // zlib, fastest/no-compression DEFLATE blocks.
    let block_count = raw.len().div_ceil(u16::MAX as usize);
    for (index, block) in raw.chunks(u16::MAX as usize).enumerate() {
        if cancel.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        idat.push(u8::from(index + 1 == block_count)); // BFINAL, BTYPE=00, byte aligned.
        let len = block.len() as u16;
        idat.extend_from_slice(&len.to_le_bytes());
        idat.extend_from_slice(&(!len).to_le_bytes());
        idat.extend_from_slice(block);
    }
    idat.extend_from_slice(&adler32(&raw, cancel)?.to_be_bytes());
    drop(raw);

    let mut png = Vec::new();
    png.try_reserve_exact(idat.len() + 64)
        .map_err(|_| MediaError::Allocation)?;
    png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = [0u8; 13];
    ihdr[0..4].copy_from_slice(&FRAME_EDGE.to_be_bytes());
    ihdr[4..8].copy_from_slice(&FRAME_EDGE.to_be_bytes());
    ihdr[8] = 8; // bit depth
    ihdr[9] = 2; // truecolour RGB, no alpha
    append_png_chunk(&mut png, *b"IHDR", &ihdr, cancel)?;
    append_png_chunk(&mut png, *b"IDAT", &idat, cancel)?;
    append_png_chunk(&mut png, *b"IEND", &[], cancel)?;
    if png.len() > MAX_FIXTURE_ENCODED_BYTES {
        return Err(MediaError::TooLarge);
    }
    Ok(png)
}

fn adler32(bytes: &[u8], cancel: &Cancellation) -> Result<u32, MediaError> {
    const MODULUS: u32 = 65_521;
    let (mut a, mut b) = (1u32, 0u32);
    for block in bytes.chunks(5_552) {
        if cancel.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        for byte in block {
            a += u32::from(*byte);
            b += a;
        }
        a %= MODULUS;
        b %= MODULUS;
    }
    Ok((b << 16) | a)
}

fn append_png_chunk(
    png: &mut Vec<u8>,
    chunk_type: [u8; 4],
    data: &[u8],
    cancel: &Cancellation,
) -> Result<(), MediaError> {
    let len = u32::try_from(data.len()).map_err(|_| MediaError::TooLarge)?;
    png.extend_from_slice(&len.to_be_bytes());
    png.extend_from_slice(&chunk_type);
    png.extend_from_slice(data);
    let mut crc = !0u32;
    for byte in chunk_type.iter().chain(data.iter()) {
        if cancel.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        crc = (crc >> 8) ^ crc32_table()[((crc as u8) ^ *byte) as usize];
    }
    png.extend_from_slice(&(!crc).to_be_bytes());
    Ok(())
}

fn crc32_table() -> &'static [u32; 256] {
    static TABLE: std::sync::LazyLock<[u32; 256]> = std::sync::LazyLock::new(|| {
        std::array::from_fn(|index| {
            let mut value = index as u32;
            for _ in 0..8 {
                value = if value & 1 == 1 {
                    (value >> 1) ^ 0xedb8_8320
                } else {
                    value >> 1
                };
            }
            value
        })
    });
    &TABLE
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = match args.next().as_deref() {
        Some("--fixture") if args.next().is_none() => Mode::Fixture,
        Some("--manifest") => Mode::Manifest(
            args.next()
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(2),
        ),
        _ => {
            eprintln!(
                "Usage: cargo run --locked -p cos-desktop --example image_upload -- --fixture\n       cargo run --locked -p cos-desktop --example image_upload -- --manifest [cycles]"
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
            move |_, cx| {
                let probe = cx.new(|_| ImageUploadProbe {
                    service: ImageUploadService::global(),
                    status:
                        "Fixture mode only. Load to generate/decode the deterministic RGB8 PNG."
                            .into(),
                    current: None,
                    cancel: Cancellation::default(),
                    task: None,
                    request_serial: 0,
                    source_revision: 0,
                    device_generation: 1,
                    pending: false,
                    paint_message: SharedString::default(),
                    consumer: ConsumerId::new(1),
                });
                if let Mode::Manifest(cycles) = mode {
                    probe.update(cx, |this, cx| this.start_manifest(cycles, cx));
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
