//! Explicit local Compare fixture experiment; never a Runtime/product result.
#[path = "support/image_fixture.rs"]
mod image_fixture;

use desktop_gpui::{
    compare_view::{CompareEvent, CompareView},
    image_upload::PreparedImage,
    image_viewer::{ImageViewer, ViewerMode},
    view_transform::ViewCamera,
};
use gpui::{
    AccessibleAction, App, AppContext, Bounds, Context, Entity, FocusHandle, InteractiveElement,
    IntoElement, KeyBinding, ParentElement, Render, Role, SharedString, StatefulInteractiveElement,
    Styled, TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div, point, px, rgb,
    size,
};
use media::MediaError;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use ui_model::presentation::{CompareSide, Presentation, SelectionEffect, ViewedCandidate};

actions!(
    native_compare,
    [
        CandidateNext,
        CandidatePrevious,
        CandidateActivate,
        DemoNext,
        DemoPrevious
    ]
);

#[derive(Clone, Copy)]
struct Candidate {
    name: &'static str,
    source: &'static str,
    revision: u64,
}
const CANDIDATES: [Candidate; 3] = [
    Candidate {
        name: "Candidate A",
        source: "compare-a",
        revision: 1,
    },
    Candidate {
        name: "Candidate B",
        source: "compare-a",
        revision: 1,
    },
    Candidate {
        name: "Candidate C (cyan marker)",
        source: "compare-c",
        revision: 2,
    },
];

fn reference(index: usize) -> desktop_protocol::CandidateRef {
    desktop_protocol::CandidateRef {
        task_id: desktop_protocol::TaskId::fixture("compare-task"),
        run_id: desktop_protocol::RunId::fixture("compare-run"),
        iteration: 1,
        candidate_id: desktop_protocol::CandidateId::fixture(CANDIDATES[index].name),
        artifact_ref: desktop_protocol::ArtifactId::fixture(CANDIDATES[index].name),
    }
}

struct TraceSample {
    requested_us: u64,
    achieved_us: u64,
    command: u8,
    target: u8,
    input: [f64; 2],
    camera: [f64; 6],
    scene_serial: [u64; 2],
    view_serial: [u64; 2],
    phase: [u8; 2],
    dpi: [f64; 2],
    viewport: [f64; 4],
    image_bounds: [f64; 8],
    bounds_available: [u8; 2],
    issued: [usize; 2],
    tiles: [usize; 2],
    decode_calls: [u64; 2],
    queue_wait_ns: [u64; 2],
    complete: [u8; 2],
    cancelled: [u8; 2],
}
struct CompareDemo {
    presentation: Presentation,
    compare: Entity<CompareView>,
    viewer: Entity<ImageViewer>,
    selected: usize,
    candidate_focus: FocusHandle,
    fixture_focus: [FocusHandle; 2],
    missing: Option<usize>,
    missing_frame: Option<Arc<desktop_gpui::image_upload::PreparedImage>>,
    closed: bool,
    share_treatment: bool,
    status: SharedString,
}
fn scale_value(camera: desktop_gpui::view_transform::ViewCamera) -> f64 {
    match camera.scale {
        desktop_gpui::view_transform::CameraScale::RelativeFit(value)
        | desktop_gpui::view_transform::CameraScale::Physical(value) => value,
    }
}

struct TraceEvidence {
    peak: [u64; 3],
    digests: [Option<[u8; 32]>; 2],
    cancel_probe_stage: u8,
    settled: bool,
}

fn export_trace(
    samples: &[TraceSample],
    ready: bool,
    warmup_ms: u128,
    refusals: u64,
    variant: &'static str,
    counters: desktop_gpui::image_upload::ImageUploadCounters,
    evidence: TraceEvidence,
) {
    use std::fmt::Write;
    let mut output = String::with_capacity(samples.len() * 1024 + 4096);
    let _ = write!(
        output,
        "{{\"scenario\":\"cmp1_native_fixed_trace\",\"variant\":\"{variant}\",\"ready\":{ready},\"warmup_ms\":{warmup_ms},\"samples\":{},\"refusals\":{refusals},\"phase\":\"initial_detail_then_warm_cache\",\"timing_scope\":\"CPU_scene_request_and_readiness_only\",\"frame_metric_scope\":\"per_entry_repeats_if_shared_across_panes_do_not_sum\",\"command_legend\":\"1=link_toggle,2=fit,3=warm_cache_zoom,4=pan,5=initial_detail_zoom_8x\",\"present\":\"NOT_MEASURED\",\"gpu\":\"NOT_MEASURED\",\"8k\":\"NOT_MEASURED\",\"mixed_dpi\":\"NOT_MEASURED\",\"one_to_one\":\"NOT_MEASURED\",\"cursor_anchored_zoom\":\"NOT_MEASURED\",\"decode_calls\":{},\"reuse_calls\":{},\"retained_frames\":{},\"consumers\":{},\"packed_retained_bytes\":{},\"cpu_retained_bytes\":{},\"resident_reservation_bytes\":{},\"queue_wait_ns\":{},\"queue_rejections\":{},\"queue_cancellations\":{},\"ledger\":[",
        samples.len(),
        counters.decode_calls,
        counters.reuse_calls,
        counters.retained_frames,
        counters.consumers,
        counters.packed_retained_bytes,
        counters.cpu_retained_bytes,
        counters.resident_reservation_bytes,
        counters.preparation_queue_wait_nanos,
        counters.preparation_queue_rejections,
        counters.preparation_queue_cancellations
    );
    for (index, sample) in samples.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        let _ = write!(
            output,
            "{{\"requested_us\":{},\"achieved_us\":{},\"command\":{},\"target\":{},\"input\":[{:.6},{:.6}],\"camera\":[{:.6},{:.6},{:.6},{:.6},{:.6},{:.6}],\"scene_serial\":[{},{}],\"view_serial\":[{},{}],\"phase\":[{},{}],\"dpi\":[{:.6},{:.6}],\"viewport\":[{:.6},{:.6},{:.6},{:.6}],\"image_bounds\":[{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6}],\"bounds_available\":[{},{}],\"issued\":[{},{}],\"tiles\":[{},{}],\"decode_calls\":[{},{}],\"queue_wait_ns\":[{},{}],\"complete\":[{},{}],\"cancelled\":[{},{}]}}",
            sample.requested_us,
            sample.achieved_us,
            sample.command,
            sample.target,
            sample.input[0],
            sample.input[1],
            sample.camera[0],
            sample.camera[1],
            sample.camera[2],
            sample.camera[3],
            sample.camera[4],
            sample.camera[5],
            sample.scene_serial[0],
            sample.scene_serial[1],
            sample.view_serial[0],
            sample.view_serial[1],
            sample.phase[0],
            sample.phase[1],
            sample.dpi[0],
            sample.dpi[1],
            sample.viewport[0],
            sample.viewport[1],
            sample.viewport[2],
            sample.viewport[3],
            sample.image_bounds[0],
            sample.image_bounds[1],
            sample.image_bounds[2],
            sample.image_bounds[3],
            sample.image_bounds[4],
            sample.image_bounds[5],
            sample.image_bounds[6],
            sample.image_bounds[7],
            sample.bounds_available[0],
            sample.bounds_available[1],
            sample.issued[0],
            sample.issued[1],
            sample.tiles[0],
            sample.tiles[1],
            sample.decode_calls[0],
            sample.decode_calls[1],
            sample.queue_wait_ns[0],
            sample.queue_wait_ns[1],
            sample.complete[0],
            sample.complete[1],
            sample.cancelled[0],
            sample.cancelled[1]
        );
    }
    output.push(']');
    let _ = write!(
        output,
        ",\"sampled_peak_cpu_retained_bytes\":{},\"sampled_peak_packed_bytes\":{},\"sampled_peak_reservation_bytes\":{},\"retention_scope\":\"CPU_payload_and_reservation_samples_not_GPU_or_process_peak\",\"settled\":{},\"cancel_probe_stage\":{},\"probe_stage_legend\":\"0=not_run,1=submitted,2=restored,3=verified_no_late_overwrite\",\"additional_commands\":\"6=delayed_source_probe,7=restore,8=verify\",\"input_decoded_rgba_sha256\":[",
        evidence.peak[0],
        evidence.peak[1],
        evidence.peak[2],
        evidence.settled,
        evidence.cancel_probe_stage
    );
    for (index, digest) in evidence.digests.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        match digest {
            Some(digest) => {
                output.push('\"');
                for byte in digest {
                    let _ = write!(output, "{byte:02x}");
                }
                output.push('\"');
            }
            None => output.push_str("null"),
        }
    }
    output.push_str(
        "],\"scene_sample_order\":\"before_input;camera_and_entry_progress_after_input\"}\n",
    );
    print!("{output}");
}

fn same_entries(
    current: &[Option<Arc<PreparedImage>>; 2],
    expected: &[Option<Arc<PreparedImage>>; 2],
) -> bool {
    current.iter().zip(expected).all(|(current, expected)| {
        current
            .as_ref()
            .zip(expected.as_ref())
            .is_some_and(|(current, expected)| Arc::ptr_eq(current, expected))
    })
}

fn start_trace(cx: &mut Context<CompareDemo>, variant: &'static str) {
    let background = cx.background_executor().clone();
    cx.spawn(async move |this, cx| {
        let warm_started = Instant::now();
        let mut ready = false;
        while warm_started.elapsed() < Duration::from_secs(10) {
            ready = this
                .update_in(cx, |this, _, cx| {
                    this.compare.update(cx, |view, cx| {
                        view.observations(cx).iter().all(|observation| {
                            observation.complete_in_scene && observation.phase >= 2
                        })
                    })
                })
                .unwrap_or(false);
            if ready {
                break;
            }
            background.timer(Duration::from_millis(16)).await;
        }
        let warmup_ms = warm_started.elapsed().as_millis();
        let mut samples = Vec::with_capacity(4096);
        let mut refusals = 0u64;
        let digests = this
            .update_in(cx, |this, _, cx| {
                this.compare
                    .read(cx)
                    .frames(cx)
                    .map(|frame| frame.map(|frame| frame.identity().content_digest))
            })
            .unwrap_or([None; 2]);
        let baseline = desktop_gpui::image_upload::ImageUploadService::global().counters();
        let mut peak = [
            baseline.cpu_retained_bytes,
            baseline.packed_retained_bytes,
            baseline.resident_reservation_bytes,
        ];
        let mut probe_frames: [Option<Arc<desktop_gpui::image_upload::PreparedImage>>; 2] =
            [None, None];
        let mut cancel_probe_stage = 0;
        let run_started = Instant::now();
        if ready {
            while run_started.elapsed() < Duration::from_secs(60) && samples.len() < 4096 {
                let requested_us = (samples.len() as u64).saturating_mul(16_000);
                background.timer(Duration::from_millis(16)).await;
                let step = samples.len();
                let command = match step {
                    0 => 5,
                    1..=63 => 4,
                    64 => 1,
                    65 => 2,
                    66 => 6,
                    67 => 7,
                    96 => 8,
                    _ if step % 240 == 0 => 1,
                    _ if step % 480 == 1 => 2,
                    _ if step % 4 == 0 => 3,
                    _ => 4,
                };
                let sample = this
                    .update_in(cx, |this, window, cx| {
                        let mut result = None;
                        this.compare.update(cx, |view, cx| {
                            let observations = view.observations(cx);
                            let mut target = 2;
                            let mut input = [0.0; 2];
                            match step {
                                0 => {
                                    target = 0;
                                    input[0] = 8.0;
                                    view.zoom_by(CompareSide::A, input[0], None, cx);
                                }
                                1..=63 => {
                                    let side = if step % 2 == 0 {
                                        CompareSide::A
                                    } else {
                                        CompareSide::B
                                    };
                                    target = side.index() as u8;
                                    input = [
                                        if step % 4 < 2 { 2200.0 } else { -2200.0 },
                                        if step % 8 < 4 { 1700.0 } else { -1700.0 },
                                    ];
                                    view.pan_by(
                                        side,
                                        desktop_gpui::view_transform::Point {
                                            x: input[0],
                                            y: input[1],
                                        },
                                        cx,
                                    );
                                }
                                64 => {
                                    let linked = !view.linked();
                                    input[0] = f64::from(linked);
                                    view.set_linked(linked, cx);
                                }
                                65 => view.reset_fit(cx),
                                66 => {
                                    target = 0;
                                    probe_frames = view.frames(cx);
                                    let request = image_fixture::fixture_request_for(
                                        CANDIDATES[2].source,
                                        2,
                                        1,
                                    )
                                    .expect("declared delayed fixture");
                                    view.load_generated_side(
                                        CompareSide::A,
                                        request,
                                        window,
                                        cx,
                                        |cancel| {
                                            for _ in 0..40 {
                                                if cancel.is_cancelled() {
                                                    return Err(MediaError::Cancelled);
                                                }
                                                std::thread::sleep(Duration::from_millis(5));
                                            }
                                            image_fixture::generate_fixture_png(2, cancel)
                                                .map(Arc::<[u8]>::from)
                                        },
                                    );
                                    cancel_probe_stage = 1;
                                }
                                67 => {
                                    target = 0;
                                    if let Some(frame) = probe_frames[0].clone()
                                        && view.present_prepared(CompareSide::A, frame, window, cx)
                                    {
                                        cancel_probe_stage = 2;
                                    }
                                }
                                96 => {
                                    let current = view.frames(cx);
                                    if cancel_probe_stage == 2
                                        && same_entries(&current, &probe_frames)
                                    {
                                        cancel_probe_stage = 3;
                                    } else {
                                        refusals = refusals.saturating_add(1);
                                    }
                                }
                                _ if step % 240 == 0 => {
                                    input[0] = f64::from(!view.linked());
                                    view.set_linked(!view.linked(), cx);
                                }
                                _ if step % 480 == 1 => view.reset_fit(cx),
                                _ => {
                                    let side = if step % 2 == 0 {
                                        CompareSide::A
                                    } else {
                                        CompareSide::B
                                    };
                                    target = side.index() as u8;
                                    if step % 4 == 0 {
                                        input[0] = if step % 8 == 0 { 1.02 } else { 1.0 / 1.02 };
                                        view.zoom_by(side, input[0], None, cx);
                                    } else {
                                        input = [if step % 2 == 0 { 3.0 } else { -3.0 }, 2.0];
                                        view.pan_by(
                                            side,
                                            desktop_gpui::view_transform::Point {
                                                x: input[0],
                                                y: input[1],
                                            },
                                            cx,
                                        );
                                    }
                                }
                            }
                            let cameras = view.cameras(cx);
                            let frames = view.frames(cx);
                            let progress = frames
                                .each_ref()
                                .map(|frame| frame.as_ref().map(|frame| frame.progress()));
                            let achieved_us =
                                run_started.elapsed().as_micros().min(u64::MAX as u128) as u64;
                            result = Some(TraceSample {
                                command,
                                target,
                                input,
                                requested_us,
                                achieved_us,
                                camera: [
                                    cameras[0].anchor.x,
                                    cameras[0].anchor.y,
                                    scale_value(cameras[0]),
                                    cameras[1].anchor.x,
                                    cameras[1].anchor.y,
                                    scale_value(cameras[1]),
                                ],
                                scene_serial: [
                                    observations[0].scene_serial,
                                    observations[1].scene_serial,
                                ],
                                view_serial: [
                                    observations[0].view_serial,
                                    observations[1].view_serial,
                                ],
                                phase: [observations[0].phase, observations[1].phase],
                                dpi: [observations[0].dpi, observations[1].dpi],
                                viewport: [
                                    observations[0].viewport.width,
                                    observations[0].viewport.height,
                                    observations[1].viewport.width,
                                    observations[1].viewport.height,
                                ],
                                image_bounds: [
                                    observations[0].image_bounds.map_or(0.0, |bounds| bounds.x),
                                    observations[0].image_bounds.map_or(0.0, |bounds| bounds.y),
                                    observations[0]
                                        .image_bounds
                                        .map_or(0.0, |bounds| bounds.width),
                                    observations[0]
                                        .image_bounds
                                        .map_or(0.0, |bounds| bounds.height),
                                    observations[1].image_bounds.map_or(0.0, |bounds| bounds.x),
                                    observations[1].image_bounds.map_or(0.0, |bounds| bounds.y),
                                    observations[1]
                                        .image_bounds
                                        .map_or(0.0, |bounds| bounds.width),
                                    observations[1]
                                        .image_bounds
                                        .map_or(0.0, |bounds| bounds.height),
                                ],
                                bounds_available: [
                                    u8::from(observations[0].image_bounds.is_some()),
                                    u8::from(observations[1].image_bounds.is_some()),
                                ],
                                issued: progress.map(|progress| {
                                    progress.map_or(0, |progress| progress.issued_tiles)
                                }),
                                tiles: progress.map(|progress| {
                                    progress.map_or(0, |progress| progress.tile_count)
                                }),
                                decode_calls: progress.map(|progress| {
                                    progress.map_or(0, |progress| progress.decode_calls)
                                }),
                                queue_wait_ns: progress.map(|progress| {
                                    progress
                                        .map_or(0, |progress| progress.preparation_queue_wait_nanos)
                                }),
                                complete: [
                                    u8::from(observations[0].complete_in_scene),
                                    u8::from(observations[1].complete_in_scene),
                                ],
                                cancelled: progress.map(|progress| {
                                    u8::from(
                                        progress.is_some_and(|progress| {
                                            progress.cancelled_before_upload
                                        }),
                                    )
                                }),
                            });
                        });
                        result
                    })
                    .ok()
                    .flatten();
                if let Some(sample) = sample {
                    samples.push(sample);
                } else {
                    refusals = refusals.saturating_add(1);
                }
                let usage = desktop_gpui::image_upload::ImageUploadService::global().counters();
                peak = [
                    peak[0].max(usage.cpu_retained_bytes),
                    peak[1].max(usage.packed_retained_bytes),
                    peak[2].max(usage.resident_reservation_bytes),
                ];
            }
        }
        let settle_started = Instant::now();
        let mut settled = false;
        while ready && settle_started.elapsed() < Duration::from_secs(2) {
            background.timer(Duration::from_millis(16)).await;
            settled = this
                .update_in(cx, |this, _, cx| {
                    this.compare
                        .read(cx)
                        .observations(cx)
                        .iter()
                        .all(|observation| observation.complete_in_scene && observation.phase >= 2)
                })
                .unwrap_or(false);
            if settled {
                break;
            }
        }
        if cancel_probe_stage >= 2 {
            let unchanged = this
                .update_in(cx, |this, _, cx| {
                    same_entries(&this.compare.read(cx).frames(cx), &probe_frames)
                })
                .unwrap_or(false);
            if !unchanged {
                cancel_probe_stage = 2;
                refusals = refusals.saturating_add(1);
            }
        }
        let counters = desktop_gpui::image_upload::ImageUploadService::global().counters();
        background
            .spawn(async move {
                export_trace(
                    &samples,
                    ready,
                    warmup_ms,
                    refusals,
                    variant,
                    counters,
                    TraceEvidence {
                        peak,
                        digests,
                        cancel_probe_stage,
                        settled,
                    },
                );
            })
            .await;
        let _ = this.update_in(cx, |this, window, cx| {
            this.compare.update(cx, |view, cx| view.close(cx));
            this.viewer.update(cx, |viewer, _| viewer.close());
            let _ = desktop_gpui::image_upload::ImageUploadService::global().evict_owned_by(window);
            window.remove_window();
        });
    })
    .detach();
}
impl CompareDemo {
    fn load_pair(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pair) = self.presentation.compare_pair() else {
            return;
        };
        self.closed = false;
        let indexes = [candidate_index(&pair[0]), candidate_index(&pair[1])];
        let requests = indexes.map(|i| {
            image_fixture::fixture_request_for(CANDIDATES[i].source, CANDIDATES[i].revision, 1)
                .expect("declared fixture request")
        });
        let shared = self.share_treatment && requests[0].key == requests[1].key;
        let retained = self.viewer.read(cx).prepared();
        if retained.is_none() {
            self.viewer.update(cx, |viewer, _| viewer.close());
        }
        self.compare.update(cx, |view, cx| {
            view.set_labels(indexes.map(|i| SharedString::from(CANDIDATES[i].name)), cx);
            if self.share_treatment
                && retained.as_ref().is_some_and(|frame| {
                    requests
                        .iter()
                        .any(|request| frame.identity().key == request.key)
                })
            {
                view.reset_fit(cx);
                for (side, request) in [CompareSide::A, CompareSide::B].into_iter().zip(requests) {
                    if let Some(frame) = retained
                        .as_ref()
                        .filter(|frame| frame.identity().key == request.key)
                    {
                        view.present_prepared(side, Arc::clone(frame), window, cx);
                    } else {
                        let revision = CANDIDATES[indexes[side.index()]].revision;
                        view.load_generated_side(side, request, window, cx, move |cancel| {
                            image_fixture::generate_fixture_png(revision, cancel)
                                .map(Arc::<[u8]>::from)
                        });
                    }
                }
            } else {
                view.load_generated_pair(requests, shared, window, cx, move |side, cancel| {
                    image_fixture::generate_fixture_png(CANDIDATES[indexes[side]].revision, cancel)
                        .map(Arc::<[u8]>::from)
                });
            }
        });
        self.status =
            "ローカル画像の比較実験。本番経路の採用・実行基盤の画像取得ではありません。".into();
        cx.notify();
    }

    fn load_side(&mut self, side: CompareSide, window: &mut Window, cx: &mut Context<Self>) {
        let Some(candidate) = self.presentation.compare_side(side) else {
            return;
        };
        let index = candidate_index(candidate);
        let request = match image_fixture::fixture_request_for(
            CANDIDATES[index].source,
            CANDIDATES[index].revision,
            1,
        ) {
            Ok(request) => request,
            Err(error) => {
                self.status = format!("Fixture request refused: {error}").into();
                return;
            }
        };
        self.compare.update(cx, |view, cx| {
            // This fixed local manifest maps these candidates to the same immutable generator
            // input. Reuse the already verified exact frame, not an unverified external key.
            if self.share_treatment
                && let Some(frame) = view.frames(cx)[1 - side.index()].clone()
                && frame.identity().key == request.key
                && view.present_prepared(side, frame, window, cx)
            {
                return;
            }
            view.load_generated_side(side, request, window, cx, move |cancel| {
                image_fixture::generate_fixture_png(CANDIDATES[index].revision, cancel)
                    .map(Arc::<[u8]>::from)
            });
        });
    }

    fn select(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.missing == Some(index) {
            return;
        }
        let previous_viewed = self.presentation.viewed().map(candidate_index);
        let effect = self.presentation.select(ViewedCandidate(reference(index)));
        self.selected = index;
        self.candidate_focus.focus(window, cx);
        match effect {
            SelectionEffect::CompareSideA => {
                self.closed = true;
                let request = image_fixture::fixture_request_for(
                    CANDIDATES[index].source,
                    CANDIDATES[index].revision,
                    1,
                )
                .expect("declared fixture request");
                self.viewer.update(cx, |viewer, cx| {
                    viewer.set_camera(ViewCamera::default(), cx);
                    if !viewer
                        .prepared()
                        .is_some_and(|frame| frame.identity().key == request.key)
                    {
                        viewer.load_generated(request, window, cx, move |cancel| {
                            image_fixture::generate_fixture_png(CANDIDATES[index].revision, cancel)
                                .map(Arc::<[u8]>::from)
                        });
                    }
                });
                self.status = "A側を選択しました。別の候補を選ぶと比較します。".into();
            }
            SelectionEffect::CompareSideB => self.load_pair(window, cx),
            SelectionEffect::ViewedOnly => {
                let pane = self.presentation.compare_pair().and_then(|pair| {
                    pair.iter()
                        .position(|candidate| candidate_index(candidate) == index)
                });
                let frame = pane.and_then(|pane| self.compare.read(cx).frames(cx)[pane].clone());
                self.viewer.update(cx, |viewer, cx| {
                    if previous_viewed != Some(index) {
                        viewer.set_camera(ViewCamera::default(), cx);
                    }
                    if let Some(frame) = frame {
                        viewer.present_prepared(frame, window, cx);
                    } else {
                        viewer.unavailable(
                            "画像を利用できません: 閲覧中の候補の保持済み画像がありません。".into(),
                            cx,
                        );
                    }
                });
                self.status =
                    "閲覧中の候補だけを変更しました。比較するA/Bと正式な採用結果は変わりません。"
                        .into();
            }
            SelectionEffect::Changed(side) => {
                if let Some(pair) = self.presentation.compare_pair() {
                    let labels = pair.each_ref().map(|candidate| {
                        SharedString::from(CANDIDATES[candidate_index(candidate)].name)
                    });
                    self.compare.update(cx, |view, cx| {
                        view.set_labels(labels, cx);
                        view.reset_fit(cx);
                    });
                    self.load_side(side, window, cx);
                }
                self.status =
                    "指定した側の候補を変更し、連動状態を保って両側を全体表示に戻しました。".into();
            }
            SelectionEffect::DuplicateSelection | SelectionEffect::DuplicateSide => {
                self.status = "比較には異なる候補が2件必要です。".into()
            }
        }
        cx.notify();
    }

    fn disappear(&mut self, cx: &mut Context<Self>) {
        let Some(pair) = self.presentation.compare_pair() else {
            return;
        };
        let index = candidate_index(&pair[0]);
        self.missing = Some(index);
        self.missing_frame = self.compare.read(cx).frames(cx)[0].clone();
        self.compare.update(cx, |view, cx| view.unavailable(CompareSide::A,
            "画像を利用できません: この候補は現在閲覧できる情報に含まれていません。参照を保持しています。".into(), cx));
        if self.selected == index {
            self.selected = (index + 1) % CANDIDATES.len();
        }
        self.status = "Fixture: A側の候補が消失。比較参照と閲覧中の候補は変更しません。".into();
        cx.notify();
    }

    fn reappear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.missing.take() else {
            return;
        };
        let side = self
            .presentation
            .compare_pair()
            .and_then(|pair| {
                pair.iter()
                    .position(|candidate| candidate_index(candidate) == index)
            })
            .map(|index| {
                if index == 0 {
                    CompareSide::A
                } else {
                    CompareSide::B
                }
            });
        if let Some(side) = side {
            let restored = self.missing_frame.take().is_some_and(|frame| {
                self.compare.update(cx, |view, cx| {
                    view.present_prepared(side, frame, window, cx)
                })
            });
            if !restored {
                self.load_side(side, window, cx);
            }
        }
        self.status =
            "Fixture: 同じ候補が再出現。参照とペインの変換を維持して画像を再取得します。".into();
        cx.notify();
    }

    fn move_candidate(&mut self, delta: isize, cx: &mut Context<Self>) {
        let next = (self.selected as isize + delta).rem_euclid(CANDIDATES.len() as isize) as usize;
        self.selected = if self.missing == Some(next) {
            (next as isize + delta).rem_euclid(CANDIDATES.len() as isize) as usize
        } else {
            next
        };
        cx.notify();
    }
}

fn candidate_index(candidate: &ViewedCandidate) -> usize {
    CANDIDATES
        .iter()
        .position(|c| candidate.0.candidate_id == desktop_protocol::CandidateId::fixture(c.name))
        .expect("the fixture host creates only declared candidate references")
}

impl Render for CompareDemo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().p_3().bg(rgb(0x20242a)).text_color(rgb(0xf0f0f0)).flex().flex_col().gap_2()
            .on_action(cx.listener(|_, _: &DemoNext, window, cx| window.focus_next(cx)))
            .on_action(cx.listener(|_, _: &DemoPrevious, window, cx| window.focus_prev(cx)))
            .on_action(cx.listener(|this, _: &CandidateNext, _, cx| this.move_candidate(1, cx)))
            .on_action(cx.listener(|this, _: &CandidatePrevious, _, cx| this.move_candidate(-1, cx)))
            .on_action(cx.listener(|this, _: &CandidateActivate, window, cx| this.select(this.selected, window, cx)))
            .child("Compare — 明示的なローカル画像の比較実験。実行基盤・GPU完了・Presentの証拠ではありません。")
            .child("正式な採用結果: このローカル実験にはありません（表示専用）")
            .child(self.status.clone())
            .child(div().flex_1().min_h_0().child(if self.closed { self.viewer.clone().into_any_element() } else { self.compare.clone().into_any_element() }))
            .child(div().id("fixture-candidates").role(Role::Group).aria_label("候補一覧。左右キーで移動、Enterで選択")
                .key_context("FixtureCandidates").track_focus(&self.candidate_focus).flex().gap_2()
                .children(CANDIDATES.iter().enumerate().map(|(index, candidate)| {
                    div().id(("fixture-candidate", index)).role(Role::Button).aria_label(candidate.name)
                        .aria_description(if self.missing == Some(index) { "この候補は現在利用できません。参照は保持されています。" } else { "通常の選択は閲覧中の候補だけを変更します。A/Bの置換には変更操作を使います。" })
                        .border_1().border_color(rgb(if self.candidate_focus.is_focused(window) && self.selected == index { 0xffd75e } else { 0xd9b76b }))
                        .p_2().child(candidate.name)
                        .on_click(cx.listener(move |this, _, window, cx| this.select(index, window, cx)))
                        .on_a11y_action(AccessibleAction::Click, {
                            let this = cx.entity().downgrade();
                            move |_, window, cx| { let _ = this.update(cx, |this, cx| this.select(index, window, cx)); }
                        })
                })))
            .child(div().flex().gap_2().children((0..2).map(|index| {
                let label = if index == 0 { "Fixture: A側候補の消失" } else { "Fixture: 同じ候補の再出現" };
                div().id(("fixture-availability", index)).role(Role::Button).aria_label(label)
                    .key_context("FixtureAvailability").track_focus(&self.fixture_focus[index]).p_2().border_1().child(label)
                    .on_action(cx.listener(move |this, _: &CandidateActivate, window, cx| {
                        if index == 0 { this.disappear(cx); } else { this.reappear(window, cx); }
                        cx.stop_propagation();
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if index == 0 { this.disappear(cx); } else { this.reappear(window, cx); }
                    }))
                    .on_a11y_action(AccessibleAction::Click, {
                        let this = cx.entity().downgrade();
                        move |_, window, cx| { let _ = this.update(cx, |this, cx| {
                            if index == 0 { this.disappear(cx); } else { this.reappear(window, cx); }
                        }); }
                    })
            })))
    }
}

fn open_compare_window(
    cx: &mut App,
    treatment_shared: bool,
    trace_enabled: bool,
    distinct: bool,
    window_number: usize,
    window_count: usize,
) {
    // Fixed reference-profile arrangement: two 1280×900 windows fit side by side on 3440×1440.
    let bounds = Bounds::new(
        point(px(40.0 + (window_number - 1) as f32 * 1320.0), px(60.0)),
        size(px(1280.0), px(900.0)),
    );
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(format!("MP-CMP1 Compare local fixture {}", window_number).into()),
                ..Default::default()
            }),
            ..Default::default()
        },
        move |window, cx| {
            let compare = cx.new(|cx| CompareView::new(ViewerMode::ExperimentalTiles, window, cx));
            let viewer = cx.new(|cx| {
                desktop_gpui::image_viewer::ImageViewer::new(
                    ViewerMode::ExperimentalTiles,
                    window,
                    cx,
                )
            });
            let demo = cx.new(|cx| CompareDemo {
                presentation: Presentation::default(),
                compare: compare.clone(),
                viewer: viewer.clone(),
                selected: if distinct { 2 } else { 1 },
                closed: false,
                share_treatment: treatment_shared,
                candidate_focus: cx.focus_handle().tab_index(30).tab_stop(true),
                fixture_focus: std::array::from_fn(|index| {
                    cx.focus_handle()
                        .tab_index(31 + index as isize)
                        .tab_stop(true)
                }),
                missing: None,
                missing_frame: None,
                status: "候補を2件選ぶと比較します。".into(),
            });
            demo.update(cx, |_, cx| {
                cx.subscribe_in(
                    &compare,
                    window,
                    |this, view_entity, event: &CompareEvent, window, cx| match *event {
                        CompareEvent::Swap => {
                            if this.presentation.swap_compare() {
                                let labels = this.presentation.compare_pair().map(|pair| {
                                    pair.each_ref().map(|candidate| {
                                        SharedString::from(
                                            CANDIDATES[candidate_index(candidate)].name,
                                        )
                                    })
                                });
                                view_entity.update(cx, |view, cx| {
                                    view.swap_resources(window, cx);
                                    if let Some(labels) = labels {
                                        view.set_labels(labels, cx);
                                    }
                                });
                                cx.notify();
                            }
                        }
                        CompareEvent::Change(side) => {
                            if !this.presentation.arm_change(side) {
                                this.status = "Cannot change a side before it exists".into();
                            } else {
                                this.status =
                                    format!("Select a candidate to replace side {}", side.label())
                                        .into();
                            }
                            this.candidate_focus.focus(window, cx);
                            cx.notify();
                        }
                        CompareEvent::Close => {
                            let viewed_index = this.presentation.viewed().map(candidate_index);
                            let pair_indexes = this
                                .presentation
                                .compare_pair()
                                .map(|pair| [candidate_index(&pair[0]), candidate_index(&pair[1])]);
                            this.presentation.close_compare();
                            view_entity.update(cx, |view, cx| {
                                let frames = view.frames(cx);
                                view.close(cx);
                                let viewed_frame = viewed_index.and_then(|index| {
                                    let pane = pair_indexes?
                                        .iter()
                                        .position(|candidate| *candidate == index)?;
                                    frames[pane].clone()
                                });
                                if let Some(frame) = viewed_frame {
                                    this.viewer.update(cx, |viewer, cx| {
                                        viewer.present_prepared(frame, window, cx);
                                    });
                                } else {
                                    this.viewer.update(cx, |viewer, cx| {
                                        viewer.unavailable(
                                            "Viewed candidate has no retained prepared frame"
                                                .into(),
                                            cx,
                                        )
                                    });
                                }
                            });
                            this.closed = true;
                            this.status = format!(
                                "比較を閉じました。閲覧中: {}",
                                viewed_index.map_or("未選択", |index| CANDIDATES[index].name)
                            )
                            .into();
                            cx.notify();
                        }
                    },
                )
                .detach();
            });
            demo.update(cx, |demo, cx| {
                demo.presentation.select(ViewedCandidate(reference(0)));
                demo.presentation
                    .select(ViewedCandidate(reference(if distinct { 2 } else { 1 })));
                demo.load_pair(window, cx);
                if trace_enabled {
                    start_trace(
                        cx,
                        if window_count > 1 {
                            if window_number == 1 {
                                "windows2-a"
                            } else {
                                "windows2-b"
                            }
                        } else if distinct {
                            "distinct"
                        } else if treatment_shared {
                            "shared"
                        } else {
                            "independent"
                        },
                    );
                }
            });
            let weak_compare = compare.downgrade();
            let weak_viewer = viewer.downgrade();
            window.on_window_should_close(cx, move |window, cx| {
                let _ = weak_compare.update(cx, |view, cx| view.close(cx));
                let _ = weak_viewer.update(cx, |viewer, _| viewer.close());
                let _ =
                    desktop_gpui::image_upload::ImageUploadService::global().evict_owned_by(window);
                true
            });
            demo
        },
    );
    if opened.is_ok() {
        println!("COMPARE_WINDOW_OPENED window={window_number}");
    } else {
        eprintln!("COMPARE_WINDOW_REFUSED window={window_number}");
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.iter().any(|arg| arg == "--fixture")
        || args.iter().any(|arg| {
            ![
                "--fixture",
                "--shared",
                "--independent",
                "--distinct",
                "--windows2",
                "--trace",
            ]
            .contains(&arg.as_str())
        })
    {
        eprintln!(
            "Usage: compare --fixture [--shared|--independent|--distinct] [--windows2] [--trace]"
        );
        std::process::exit(2);
    }
    let treatment_shared = !args
        .iter()
        .any(|arg| arg == "--independent" || arg == "--distinct");
    let trace_enabled = args.iter().any(|arg| arg == "--trace");
    let distinct = args.iter().any(|arg| arg == "--distinct");
    let windows2 = args.iter().any(|arg| arg == "--windows2");
    let independent = args.iter().any(|arg| arg == "--independent");
    if args.iter().any(|arg| arg == "--shared") && (independent || distinct) {
        eprintln!("Choose one shared/independent/distinct treatment");
        std::process::exit(2);
    }
    if windows2 && (independent || distinct) {
        eprintln!(
            "--windows2 requires shared-per-window entries; --independent/--distinct would need four retained entries"
        );
        std::process::exit(2);
    }
    gpui_platform::application().run(move |cx: &mut App| {
        desktop_gpui::compare_view::bind_keys(cx);
        desktop_gpui::image_viewer::bind_keys(cx);
        cx.bind_keys([
            KeyBinding::new("tab", DemoNext, None),
            KeyBinding::new("shift-tab", DemoPrevious, None),
            KeyBinding::new("right", CandidateNext, Some("FixtureCandidates")),
            KeyBinding::new("left", CandidatePrevious, Some("FixtureCandidates")),
            KeyBinding::new(
                "enter",
                CandidateActivate,
                Some("FixtureCandidates || FixtureAvailability"),
            ),
            KeyBinding::new(
                "space",
                CandidateActivate,
                Some("FixtureCandidates || FixtureAvailability"),
            ),
        ]);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        if windows2 {
            open_compare_window(cx, true, trace_enabled, false, 1, 2);
            open_compare_window(cx, true, trace_enabled, false, 2, 2);
        } else {
            open_compare_window(cx, treatment_shared, trace_enabled, distinct, 1, 1);
        }
        if cx.windows().is_empty() {
            cx.quit();
        }
    });
}
