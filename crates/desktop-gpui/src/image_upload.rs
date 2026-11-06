//! Bounded MP-IMG3 presentation of the admitted 2048×2048 RGB8 PNG class.
//!
//! Decode, generated-source work and RGBA→BGRA tile conversion run on GPUI's background
//! executor. The pinned GPUI accepts whole `RenderImage`s, so this path uses sixteen 512×512
//! images: each packed row is 2,048 bytes (256-byte aligned) and each whole image is 1 MiB.
//! A single process-wide 2 MiB / 8.333 ms allowance admits at most two such images per period.
//!
//! `Window::paint_image` adds the whole image to that window's atlas and the scene. This module
//! counts only successful API calls as `paint_issued`; GPUI exposes no fence, Present completion,
//! or GPU-residency measurement here. Accordingly it never calls `complete_upload` and retains
//! the admitted CPU raster and `RenderImage` payloads. A frame is composed into the visible scene
//! only after every tile has been inserted for the owning window. Same-key users in that window
//! share the same frame and atlas IDs. GPUI's sprite atlas is per-window, so a prepared frame is
//! deliberately bound to its first painting window rather than duplicating uploads in another.
//!
//! Retained identities are bounded to two complete frames per process. Each retains the decoded
//! 16 MiB raster and 16 MiB of packed tile payloads; this 64 MiB ceiling is CPU-side retention,
//! not a ratified GPU residency budget. Reaching it refuses new identities rather than dropping
//! payloads or implying GPU completion. Native transfer duration, completion, residency, and
//! presentation remain NOT_MEASURED.

use std::{
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};

use gpui::{
    AnyWindowHandle, BackgroundExecutor, Bounds, Corners, RenderImage, Task, Window, fill, point,
    px, rgb, size,
};
use image::{Frame, RgbaImage};
use media::{
    Cancellation, DecodeLimits, MediaError, RepresentationKey, RepresentationSource, UploadBudget,
    UploadIdentity, UploadPool, UploadRefusal, UploadStage, VerifiedRaster, admit_upload,
    decode_for_upload,
};

use crate::upload_budget::{MAX_SCHEDULE_TILES, TileSpan, schedule_tiles};

/// The only frame geometry currently admitted by the media proof.
pub const FRAME_EDGE: u32 = 2048;
/// Kept below the largest 120 Hz tile so each row is naturally aligned for the pinned upload APIs.
pub const TILE_EDGE: u32 = 512;
/// Conservative process-wide allowance, selected to fit the smaller active canon cap.
pub const PERIOD_UPLOAD_CAP_BYTES: u64 = 2 * 1024 * 1024;
/// The process-wide allowance period is 1/120 second, rounded down to the nanosecond.
pub const PERIOD_DURATION: Duration = Duration::from_nanos(8_333_333);
/// Tightly packed GPUI images are additionally required to have this aligned row pitch.
const ROW_PITCH_ALIGNMENT: u64 = 256;
/// Two frames retain 64 MiB total: decoded RGBA plus packed tile bytes for each.
const MAX_RETAINED_FRAMES: usize = 2;
const FRAME_RGBA8_BYTES: u64 = FRAME_EDGE as u64 * FRAME_EDGE as u64 * 4;
const STAGING_LIMIT_BYTES: u64 = FRAME_RGBA8_BYTES * MAX_RETAINED_FRAMES as u64;

static PROCESS_EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);
static PROCESS_ALLOWANCE: LazyLock<Mutex<PeriodAllowance>> =
    LazyLock::new(|| Mutex::new(PeriodAllowance::default()));
static PROCESS_PREPARATION: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
static PROCESS_SERVICE: LazyLock<ImageUploadService> = LazyLock::new(|| ImageUploadService {
    inner: Arc::new(ServiceInner {
        state: Mutex::new(ServiceState::default()),
    }),
});

/// Caller-owned source identity and current device generation for one admitted upload request.
#[derive(Clone, Debug)]
pub struct UploadRequest {
    pub key: RepresentationKey,
    pub source_revision: u64,
    pub device_generation: u64,
    /// The device generation observed by the caller when the request is made.
    pub current_device_generation: u64,
    pub decode_limits: DecodeLimits,
    /// Encoded input is bounded before `decode_for_upload` inspects it.
    pub max_encoded_bytes: usize,
}

/// Safe refusal categories for the reusable desktop image presentation path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageUploadError {
    Cancelled,
    PreparationBusy,
    /// A consumer still presents this frame; eviction would drop a live resource.
    ConsumersRemain,
    /// A paint callback holds the frame; eviction never frees a resource in use.
    PaintInProgress,
    /// The frame is not in this process registry (already evicted or never admitted).
    NotRetained,
    /// The frame's atlas entries belong to another window.
    NotOwnedByWindow,
    /// GPUI refused to remove an atlas entry.
    AtlasRelease,
    StaleDeviceGeneration {
        requested: u64,
        current: u64,
    },
    SourceRevisionMismatch {
        key_revision: u64,
        requested: u64,
    },
    UnverifiableSourceRevision,
    EncodedInputTooLarge {
        actual: usize,
        maximum: usize,
    },
    Decode(MediaError),
    Admission(UploadRefusal),
    RetainedFrameLimit,
    InvalidRasterLayout,
    Allocation,
    InvalidEncodedInputLimit,
}

impl std::fmt::Display for ImageUploadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for ImageUploadError {}

/// Progress returned by one paint callback. `CompleteInScene` means every tile was added to the
/// current GPUI scene; it is not GPU completion, residency or Present evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaintStatus {
    Pending {
        issued_tiles: usize,
        tile_count: usize,
    },
    CompleteInScene {
        tile_count: usize,
    },
    Cancelled,
    StaleRevision {
        requested: u64,
        current: u64,
    },
    StaleDeviceGeneration {
        requested: u64,
        current: u64,
    },
    EmptyBounds,
    DifferentWindow,
    AdmissionUnavailable,
    /// The frame was evicted and must not be painted again.
    Evicted,
}

/// What one eviction actually released.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EvictionOutcome {
    /// Atlas images removed from the owning window (one per tile).
    pub dropped_images: usize,
    /// CPU payload bytes released (a staged release, or a quarantined payload after the
    /// transfer-terminated assertion).
    pub released_bytes: u64,
}

/// What a window-close sweep evicted and what it had to leave retained.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EvictionSummary {
    pub evicted_frames: usize,
    pub skipped_frames: usize,
    pub dropped_images: usize,
    pub released_bytes: u64,
}

/// Read-only progress for one prepared identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameProgress {
    pub source_revision: u64,
    pub device_generation: u64,
    pub issued_tiles: usize,
    pub tile_count: usize,
    /// Whether a complete tile composition was added to the most recent valid GPUI scene.
    pub complete_in_scene: bool,
    pub cancelled_before_upload: bool,
}

/// Process-wide upload observations. These are admission/API-call counters, not GPU measurements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageUploadCounters {
    pub retained_frames: usize,
    pub staging_reservation_bytes: u64,
    pub in_flight_reservation_bytes: u64,
    pub resident_reservation_bytes: u64,
    pub quarantined_staging_bytes: u64,
    pub quarantined_in_flight_bytes: u64,
    pub quarantined_resident_bytes: u64,
    pub period_index: u128,
    pub reserved_bytes_this_period: u64,
    pub period_capacity_bytes: u64,
    /// Bytes in first successful `paint_image` calls for newly issued whole tiles.
    pub paint_issued_bytes: u64,
    pub paint_issued_tiles: u64,
    /// Repaint calls for tiles already issued in an earlier frame. Under the device-generation
    /// contract these are atlas cache hits; a real device loss advances the generation and makes
    /// the frame stale before it can repaint.
    pub repaint_calls: u64,
    /// Frames whose consumers all detached and whose atlas entries were released by eviction.
    pub evicted_frames: u64,
    /// Atlas images removed by eviction (one per tile of every evicted frame).
    pub dropped_atlas_images: u64,
    /// CPU payload bytes released after an eviction asserted transfer termination.
    pub released_payload_bytes: u64,
    /// Consumers currently attached across every retained frame.
    pub consumers: usize,
}

#[derive(Default)]
struct PeriodAllowance {
    period_index: u128,
    reserved_bytes: u64,
    paint_issued_bytes: u64,
    paint_issued_tiles: u64,
    /// Repaint calls for tiles already issued; these are cache hits under the device-generation
    /// contract and are reported separately rather than charged to a period.
    repaint_calls: u64,
}

impl PeriodAllowance {
    fn reserve(&mut self, period_index: u128, bytes: u64) -> bool {
        if bytes == 0 || bytes > PERIOD_UPLOAD_CAP_BYTES {
            return false;
        }
        if self.period_index != period_index {
            self.period_index = period_index;
            self.reserved_bytes = 0;
        }
        let Some(total) = self.reserved_bytes.checked_add(bytes) else {
            return false;
        };
        if total > PERIOD_UPLOAD_CAP_BYTES {
            return false;
        }
        self.reserved_bytes = total;
        true
    }

    fn note_paint_issued(&mut self, bytes: u64) {
        self.paint_issued_bytes = self.paint_issued_bytes.saturating_add(bytes);
        self.paint_issued_tiles = self.paint_issued_tiles.saturating_add(1);
    }

    fn note_repaint(&mut self, tiles: u64) {
        self.repaint_calls = self.repaint_calls.saturating_add(tiles);
    }
}

fn current_period_index() -> u128 {
    PROCESS_EPOCH.elapsed().as_nanos() / PERIOD_DURATION.as_nanos()
}

fn reserve_process_period(period_index: u128, bytes: u64) -> bool {
    PROCESS_ALLOWANCE
        .lock()
        .expect("process upload allowance")
        .reserve(period_index, bytes)
}

/// Whether this paint callback may still charge its own period.
///
/// One scene is one frame: if the process period rolled over while the callback was issuing
/// tiles, the remainder must wait for the next requested frame instead of spending the new
/// period's allowance inside the same scene.
fn period_still_current(paint_period: u128) -> bool {
    current_period_index() == paint_period
}

fn note_process_paint_issued(bytes: u64) {
    PROCESS_ALLOWANCE
        .lock()
        .expect("process upload allowance")
        .note_paint_issued(bytes);
}

fn note_process_repaint(tiles: u64) {
    PROCESS_ALLOWANCE
        .lock()
        .expect("process upload allowance")
        .note_repaint(tiles);
}

/// The one process-wide reusable admission/cache. Clones share its registry and period allowance.
#[derive(Clone)]
pub struct ImageUploadService {
    inner: Arc<ServiceInner>,
}

struct ServiceInner {
    state: Mutex<ServiceState>,
}

#[derive(Default)]
struct ServiceState {
    pools: Vec<(u64, Arc<UploadPool>)>,
    frames: Vec<Arc<PreparedImage>>,
    evicted_frames: u64,
    dropped_atlas_images: u64,
    released_payload_bytes: u64,
}

impl ImageUploadService {
    /// Obtain the process-wide service. Separate panes/windows must share this value.
    pub fn global() -> Self {
        (*PROCESS_SERVICE).clone()
    }

    /// Generate/obtain encoded bytes, decode the approved class and build BGRA whole-image tiles
    /// on GPUI's background executor. The source callback also runs there and must observe cancel.
    pub fn prepare_generated(
        &self,
        background: &BackgroundExecutor,
        request: UploadRequest,
        cancel: Cancellation,
        source: impl FnOnce(&Cancellation) -> Result<Arc<[u8]>, MediaError> + Send + 'static,
    ) -> Task<Result<Arc<PreparedImage>, ImageUploadError>> {
        let service = self.clone();
        background.spawn(async move {
            validate_request(&request)?;
            if cancel.is_cancelled() {
                return Err(ImageUploadError::Cancelled);
            }
            // One full-frame decoder/converter at a time bounds transient encoded, decoded and
            // tiled buffers process-wide while retained frames remain separately capped.
            let _preparation = match PROCESS_PREPARATION.try_lock() {
                Ok(guard) => guard,
                Err(std::sync::TryLockError::WouldBlock) => {
                    return Err(ImageUploadError::PreparationBusy);
                }
                Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
            };
            if cancel.is_cancelled() {
                return Err(ImageUploadError::Cancelled);
            }
            let encoded = match source(&cancel) {
                Ok(encoded) => encoded,
                Err(MediaError::Cancelled) => return Err(ImageUploadError::Cancelled),
                Err(error) => return Err(ImageUploadError::Decode(error)),
            };
            if cancel.is_cancelled() {
                return Err(ImageUploadError::Cancelled);
            }
            if encoded.len() > request.max_encoded_bytes {
                return Err(ImageUploadError::EncodedInputTooLarge {
                    actual: encoded.len(),
                    maximum: request.max_encoded_bytes,
                });
            }
            let verified = decode_for_upload(&encoded, request.decode_limits, &cancel)
                .map_err(map_media_error)?;
            if cancel.is_cancelled() {
                return Err(ImageUploadError::Cancelled);
            }
            service.prepare_verified(verified, request, &cancel)
        })
    }

    /// The ordinary encoded-input convenience path; decoding and conversion remain background work.
    pub fn prepare_encoded(
        &self,
        background: &BackgroundExecutor,
        request: UploadRequest,
        encoded: Arc<[u8]>,
        cancel: Cancellation,
    ) -> Task<Result<Arc<PreparedImage>, ImageUploadError>> {
        self.prepare_generated(background, request, cancel, move |_| Ok(encoded))
    }

    fn prepare_verified(
        &self,
        verified: VerifiedRaster,
        request: UploadRequest,
        cancel: &Cancellation,
    ) -> Result<Arc<PreparedImage>, ImageUploadError> {
        if cancel.is_cancelled() {
            return Err(ImageUploadError::Cancelled);
        }
        validate_request(&request)?;
        if verified.size().width() != FRAME_EDGE
            || verified.size().height() != FRAME_EDGE
            || verified.raster().rgba.len() as u64 != FRAME_RGBA8_BYTES
        {
            return Err(ImageUploadError::InvalidRasterLayout);
        }
        let identity = UploadIdentity {
            key: request.key.clone(),
            source_revision: request.source_revision,
            device_generation: request.device_generation,
            content_digest: verified.content_digest(),
        };
        let pool = {
            let mut state = self.inner.state.lock().expect("image upload service state");
            if let Some(existing) = state.frames.iter().find(|frame| frame.identity == identity) {
                return Ok(Arc::clone(existing));
            }
            if state.frames.len() >= MAX_RETAINED_FRAMES {
                return Err(ImageUploadError::RetainedFrameLimit);
            }
            prune_idle_pools(&mut state);
            state
                .pools
                .iter()
                .find(|(generation, _)| *generation == request.device_generation)
                .map(|(_, pool)| Arc::clone(pool))
                .unwrap_or_else(|| {
                    let pool = UploadPool::new(UploadBudget {
                        staging_bytes: STAGING_LIMIT_BYTES,
                        in_flight_bytes: STAGING_LIMIT_BYTES,
                        resident_bytes: STAGING_LIMIT_BYTES,
                    });
                    state
                        .pools
                        .push((request.device_generation, Arc::clone(&pool)));
                    pool
                })
        };
        let admission = admit_upload(
            &verified,
            identity.clone(),
            request.current_device_generation,
            &pool,
            cancel,
        )
        .map_err(ImageUploadError::Admission)?;
        let tiles = build_tiles(&verified, cancel)?;
        if cancel.is_cancelled() {
            return Err(ImageUploadError::Cancelled);
        }
        let issued = vec![false; tiles.len()];
        let prepared = Arc::new(PreparedImage {
            identity,
            tiles,
            admission: Mutex::new(Some(admission)),
            paint_state: Mutex::new(PaintState {
                issued,
                ..PaintState::default()
            }),
        });
        let mut state = self.inner.state.lock().expect("image upload service state");
        if let Some(existing) = state
            .frames
            .iter()
            .find(|frame| frame.identity == prepared.identity)
        {
            return Ok(Arc::clone(existing));
        }
        if state.frames.len() >= MAX_RETAINED_FRAMES {
            return Err(ImageUploadError::RetainedFrameLimit);
        }
        state.frames.push(Arc::clone(&prepared));
        Ok(prepared)
    }

    /// Share an already prepared exact identity without decoding or creating new atlas images.
    pub fn reuse(&self, identity: &UploadIdentity) -> Option<Arc<PreparedImage>> {
        self.inner
            .state
            .lock()
            .expect("image upload service state")
            .frames
            .iter()
            .find(|frame| frame.identity == *identity)
            .cloned()
    }

    /// Attach one live consumer to a retained frame.
    ///
    /// Returns `false` when the frame is no longer retained (already evicted) — a consumer never
    /// resurrects a released resource.
    pub fn attach(&self, frame: &Arc<PreparedImage>, consumer: ConsumerId) -> bool {
        let retained = self
            .inner
            .state
            .lock()
            .expect("image upload service state")
            .frames
            .iter()
            .any(|candidate| Arc::ptr_eq(candidate, frame));
        if !retained {
            return false;
        }
        frame
            .paint_state
            .lock()
            .expect("image frame paint state")
            .consumers
            .insert(consumer);
        true
    }

    /// Detach one consumer. Returns whether this consumer was attached.
    pub fn detach(&self, frame: &Arc<PreparedImage>, consumer: ConsumerId) -> bool {
        frame
            .paint_state
            .lock()
            .expect("image frame paint state")
            .consumers
            .remove(&consumer)
    }

    /// How many consumers currently present this frame.
    pub fn consumers(&self, frame: &Arc<PreparedImage>) -> usize {
        frame
            .paint_state
            .lock()
            .expect("image frame paint state")
            .consumers
            .len()
    }

    /// Release a frame whose consumers have all detached.
    ///
    /// The caller must have stopped painting it: this removes the window's atlas entries, drops
    /// the admission (a `Staged` payload is released; an already-begun transfer is quarantined),
    /// then asserts that the transfer is terminated so that quarantined payload can be freed.
    /// That assertion is the body's only option without a GPU completion signal, and the module
    /// says so instead of reporting a measured release.
    pub fn evict(
        &self,
        frame: &Arc<PreparedImage>,
        window: &mut Window,
    ) -> Result<EvictionOutcome, ImageUploadError> {
        let mut service = self.inner.state.lock().expect("image upload service state");
        if !service
            .frames
            .iter()
            .any(|candidate| Arc::ptr_eq(candidate, frame))
        {
            return Err(ImageUploadError::NotRetained);
        }
        let mut paint_state = match frame.paint_state.try_lock() {
            Ok(state) => state,
            Err(std::sync::TryLockError::WouldBlock) => {
                return Err(ImageUploadError::PaintInProgress);
            }
            Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
        };
        if !paint_state.consumers.is_empty() {
            return Err(ImageUploadError::ConsumersRemain);
        }
        let mut outcome = EvictionOutcome::default();
        if let Some(owner) = paint_state.owner_window {
            if owner != window.window_handle() {
                return Err(ImageUploadError::NotOwnedByWindow);
            }
            for tile in &frame.tiles {
                window
                    .drop_image(Arc::clone(&tile.image))
                    .map_err(|_| ImageUploadError::AtlasRelease)?;
                outcome.dropped_images = outcome.dropped_images.saturating_add(1);
            }
        }
        paint_state.complete_in_scene = false;
        let mut admission = frame.admission.lock().expect("image frame admission");
        if let Some(mut admitted) = admission.take() {
            let identity = admitted.identity().clone();
            let bytes = admitted.bytes();
            if admitted.stage() == UploadStage::Staged {
                if admitted.release().is_ok() {
                    outcome.released_bytes = bytes;
                }
            } else {
                // Dropping an in-flight admission quarantines its payload; the assertion below
                // is what frees it. Nothing here observes GPU completion.
                drop(admitted);
                let released = service
                    .pools
                    .iter()
                    .find(|(generation, _)| *generation == identity.device_generation)
                    .is_some_and(|(_, pool)| pool.confirm_transfer_terminated(&identity).is_ok());
                if released {
                    outcome.released_bytes = bytes;
                }
            }
        }
        drop(admission);
        service
            .frames
            .retain(|candidate| !Arc::ptr_eq(candidate, frame));
        service.evicted_frames = service.evicted_frames.saturating_add(1);
        service.dropped_atlas_images = service
            .dropped_atlas_images
            .saturating_add(outcome.dropped_images as u64);
        service.released_payload_bytes = service
            .released_payload_bytes
            .saturating_add(outcome.released_bytes);
        drop(paint_state);
        prune_idle_pools(&mut service);
        Ok(outcome)
    }

    /// Evict every frame this window owns that has no remaining consumer.
    ///
    /// Window close calls this before the platform destroys the atlas: frames another window owns
    /// or that a consumer still presents are left retained and reported as skipped.
    pub fn evict_owned_by(&self, window: &mut Window) -> EvictionSummary {
        let handle = window.window_handle();
        let candidates: Vec<Arc<PreparedImage>> = self
            .inner
            .state
            .lock()
            .expect("image upload service state")
            .frames
            .iter()
            .filter(|frame| {
                frame
                    .paint_state
                    .try_lock()
                    .map(|state| state.owner_window == Some(handle) && state.consumers.is_empty())
                    .unwrap_or(false)
            })
            .cloned()
            .collect();
        let mut summary = EvictionSummary::default();
        for frame in candidates {
            match self.evict(&frame, window) {
                Ok(outcome) => {
                    summary.evicted_frames = summary.evicted_frames.saturating_add(1);
                    summary.dropped_images = summary
                        .dropped_images
                        .saturating_add(outcome.dropped_images);
                    summary.released_bytes = summary
                        .released_bytes
                        .saturating_add(outcome.released_bytes);
                }
                Err(_) => summary.skipped_frames = summary.skipped_frames.saturating_add(1),
            }
        }
        summary
    }

    /// Release a prepared frame only if no whole image was ever passed to GPUI. Once `paint_image`
    /// was entered, transfer completion is not observable and the frame must remain retained
    /// until it is evicted after its last consumer detaches.
    pub fn discard_unpainted(&self, frame: &Arc<PreparedImage>) -> bool {
        let mut service = self.inner.state.lock().expect("image upload service state");
        let mut paint_state = frame.paint_state.lock().expect("image frame paint state");
        if paint_state.issued.iter().any(|issued| *issued) || paint_state.cancelled_before_upload {
            return false;
        }
        let mut admission = frame.admission.lock().expect("image frame admission");
        let Some(admission) = admission.as_mut() else {
            return false;
        };
        if admission.stage() != UploadStage::Staged || admission.release().is_err() {
            return false;
        }
        paint_state.cancelled_before_upload = true;
        service
            .frames
            .retain(|candidate| !Arc::ptr_eq(candidate, frame));
        prune_idle_pools(&mut service);
        true
    }

    /// Return bounded admission and paint-call counters. Resident fields are reservations only;
    /// none of these fields measures GPU memory or completion.
    pub fn counters(&self) -> ImageUploadCounters {
        let state = self.inner.state.lock().expect("image upload service state");
        let mut staging = 0u64;
        let mut in_flight = 0u64;
        let mut resident = 0u64;
        let mut staging_stuck = 0u64;
        let mut in_flight_stuck = 0u64;
        let mut resident_stuck = 0u64;
        for (_, pool) in &state.pools {
            let usage = pool.usage();
            staging = staging.saturating_add(usage.staging);
            in_flight = in_flight.saturating_add(usage.in_flight);
            resident = resident.saturating_add(usage.resident);
            staging_stuck = staging_stuck.saturating_add(usage.staging_stuck);
            in_flight_stuck = in_flight_stuck.saturating_add(usage.in_flight_stuck);
            resident_stuck = resident_stuck.saturating_add(usage.resident_stuck);
        }
        let period_index = current_period_index();
        let allowance = PROCESS_ALLOWANCE.lock().expect("process upload allowance");
        let reserved_bytes_this_period = if allowance.period_index == period_index {
            allowance.reserved_bytes
        } else {
            0
        };
        ImageUploadCounters {
            retained_frames: state.frames.len(),
            staging_reservation_bytes: staging,
            in_flight_reservation_bytes: in_flight,
            resident_reservation_bytes: resident,
            quarantined_staging_bytes: staging_stuck,
            quarantined_in_flight_bytes: in_flight_stuck,
            quarantined_resident_bytes: resident_stuck,
            period_index,
            reserved_bytes_this_period,
            period_capacity_bytes: PERIOD_UPLOAD_CAP_BYTES,
            paint_issued_bytes: allowance.paint_issued_bytes,
            paint_issued_tiles: allowance.paint_issued_tiles,
            repaint_calls: allowance.repaint_calls,
            evicted_frames: state.evicted_frames,
            dropped_atlas_images: state.dropped_atlas_images,
            released_payload_bytes: state.released_payload_bytes,
            consumers: state
                .frames
                .iter()
                .map(|frame| {
                    frame
                        .paint_state
                        .try_lock()
                        .map(|state| state.consumers.len())
                        .unwrap_or(0)
                })
                .sum(),
        }
    }
}

/// Drop pools that no retained frame owns and that hold no charged or quarantined bytes.
///
/// A logical generation advance creates a pool on first use; without this, every advance would
/// leave an empty entry behind and `counters()` would sum an ever-growing list. A pool that still
/// owns a retained frame or any charged/quarantined bytes is never removed.
fn prune_idle_pools(state: &mut ServiceState) {
    let live: Vec<u64> = state
        .frames
        .iter()
        .map(|frame| frame.identity.device_generation)
        .collect();
    state
        .pools
        .retain(|(generation, pool)| !pool_is_idle(*generation, &pool.usage(), &live));
}

/// Whether a generation's pool can be dropped: no retained frame owns it and it holds no charged
/// or quarantined bytes.
fn pool_is_idle(generation: u64, usage: &media::UploadUsage, live: &[u64]) -> bool {
    if live.contains(&generation) {
        return false;
    }
    usage
        .staging
        .saturating_add(usage.in_flight)
        .saturating_add(usage.resident)
        .saturating_add(usage.staging_stuck)
        .saturating_add(usage.in_flight_stuck)
        .saturating_add(usage.resident_stuck)
        == 0
}

fn validate_request(request: &UploadRequest) -> Result<(), ImageUploadError> {
    if request.device_generation != request.current_device_generation {
        return Err(ImageUploadError::StaleDeviceGeneration {
            requested: request.device_generation,
            current: request.current_device_generation,
        });
    }
    let key_revision = match &request.key.source {
        RepresentationSource::Local { revision, .. } => *revision,
        RepresentationSource::Artifact { .. } => {
            return Err(ImageUploadError::UnverifiableSourceRevision);
        }
    };
    if key_revision != request.source_revision {
        return Err(ImageUploadError::SourceRevisionMismatch {
            key_revision,
            requested: request.source_revision,
        });
    }
    if request.max_encoded_bytes == 0 {
        return Err(ImageUploadError::InvalidEncodedInputLimit);
    }
    Ok(())
}
fn map_media_error(error: MediaError) -> ImageUploadError {
    match error {
        MediaError::Cancelled => ImageUploadError::Cancelled,
        error => ImageUploadError::Decode(error),
    }
}

struct PreparedTile {
    span: TileSpan,
    image: Arc<RenderImage>,
    transfer_bytes: u64,
}

/// One live consumer of a prepared frame: a pane, a surface or a probe view.
///
/// Ownership is explicit so a shared frame is released only after its **last** consumer detaches;
/// a pane closing must not drop a frame another pane still displays.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConsumerId(u64);

impl ConsumerId {
    pub fn new(value: u64) -> Self {
        Self(value)
    }
}

#[derive(Default)]
struct PaintState {
    owner_window: Option<AnyWindowHandle>,
    issued: Vec<bool>,
    complete_in_scene: bool,
    cancelled_before_upload: bool,
    /// Consumers currently presenting this frame. Eviction requires this to be empty.
    consumers: std::collections::BTreeSet<ConsumerId>,
}

/// An immutable, complete set of tile images tied to one verified source/revision/device identity.
pub struct PreparedImage {
    identity: UploadIdentity,
    tiles: Vec<PreparedTile>,
    admission: Mutex<Option<media::AdmittedUpload>>,
    paint_state: Mutex<PaintState>,
}

impl PreparedImage {
    pub fn identity(&self) -> &UploadIdentity {
        &self.identity
    }

    pub fn progress(&self) -> FrameProgress {
        let state = self.paint_state.lock().expect("image frame paint state");
        FrameProgress {
            source_revision: self.identity.source_revision,
            device_generation: self.identity.device_generation,
            issued_tiles: state.issued.iter().filter(|issued| **issued).count(),
            tile_count: self.tiles.len(),
            complete_in_scene: state.complete_in_scene,
            cancelled_before_upload: state.cancelled_before_upload,
        }
    }

    /// Add this image to the current GPUI paint scene. New whole-image calls are admitted through
    /// the process-wide allowance. Until every tile has a successful `paint_image` return, an
    /// opaque background quad covers any early tile primitives; then all tiles are drawn together.
    pub fn paint(
        &self,
        bounds: Bounds<gpui::Pixels>,
        current_source_revision: u64,
        current_device_generation: u64,
        cancel: &Cancellation,
        window: &mut Window,
    ) -> gpui::Result<PaintStatus> {
        window.paint_quad(fill(bounds, rgb(0x141820)));
        // One paint callback charges exactly one process period, whatever the wall clock does
        // while the tiles are issued: a rollover defers the remainder to the next frame.
        let paint_period = current_period_index();
        let mut state = self.paint_state.lock().expect("image frame paint state");
        state.complete_in_scene = false;
        if cancel.is_cancelled() {
            return Ok(PaintStatus::Cancelled);
        }
        if self.identity.source_revision != current_source_revision {
            return Ok(PaintStatus::StaleRevision {
                requested: self.identity.source_revision,
                current: current_source_revision,
            });
        }
        if self.identity.device_generation != current_device_generation {
            return Ok(PaintStatus::StaleDeviceGeneration {
                requested: self.identity.device_generation,
                current: current_device_generation,
            });
        }
        if bounds.size.width <= gpui::Pixels::ZERO || bounds.size.height <= gpui::Pixels::ZERO {
            return Ok(PaintStatus::EmptyBounds);
        }
        let window_id = window.window_handle();
        if state.cancelled_before_upload {
            return Ok(PaintStatus::Cancelled);
        }
        if state.owner_window.is_some_and(|owner| owner != window_id) {
            return Ok(PaintStatus::DifferentWindow);
        }
        let mut admission = self.admission.lock().expect("image frame admission");
        // An evicted frame has no admission left: it must not be painted again.
        let Some(admission) = admission.as_mut() else {
            return Ok(PaintStatus::Evicted);
        };
        let previously_issued = state.issued.iter().filter(|issued| **issued).count();
        for (index, tile) in self.tiles.iter().enumerate() {
            if state.issued[index] {
                continue;
            }
            if cancel.is_cancelled() {
                state.complete_in_scene = false;
                window.paint_quad(fill(bounds, rgb(0x141820)));
                return Ok(PaintStatus::Cancelled);
            }
            if !period_still_current(paint_period)
                || !reserve_process_period(paint_period, tile.transfer_bytes)
            {
                state.complete_in_scene = false;
                window.paint_quad(fill(bounds, rgb(0x141820)));
                window.request_animation_frame();
                return Ok(PaintStatus::AdmissionUnavailable);
            }
            if admission.stage() == UploadStage::Staged && admission.begin_upload().is_err() {
                state.complete_in_scene = false;
                window.paint_quad(fill(bounds, rgb(0x141820)));
                window.request_animation_frame();
                return Ok(PaintStatus::AdmissionUnavailable);
            }
            let tile_bounds = tile_bounds(bounds, tile.span);
            if let Err(error) = window.paint_image(
                tile_bounds,
                tile_bounds,
                Corners::default(),
                Arc::clone(&tile.image),
                0,
                false,
            ) {
                window.paint_quad(fill(bounds, rgb(0x141820)));
                window.request_animation_frame();
                return Err(error);
            }
            state.owner_window = Some(window_id);
            state.issued[index] = true;
            note_process_paint_issued(tile.transfer_bytes);
        }
        let issued_tiles = state.issued.iter().filter(|issued| **issued).count();
        if issued_tiles != self.tiles.len() {
            state.complete_in_scene = false;
            window.paint_quad(fill(bounds, rgb(0x141820)));
            window.request_animation_frame();
            return Ok(PaintStatus::Pending {
                issued_tiles,
                tile_count: self.tiles.len(),
            });
        }
        if previously_issued > 0 {
            // Already-issued tiles are drawn again every frame: GPUI rebuilds the scene and the
            // sprite atlas is expected to hold them, so these calls are normally cache hits and
            // must not consume the period allowance (that would starve a completed frame on the
            // next frame). A cleared atlas accompanies a device-generation change, which makes
            // this frame stale above before any repaint can re-upload it.
            note_process_repaint(previously_issued as u64);
        }
        for tile in self.tiles.iter().take(previously_issued) {
            let tile_bounds = tile_bounds(bounds, tile.span);
            if let Err(error) = window.paint_image(
                tile_bounds,
                tile_bounds,
                Corners::default(),
                Arc::clone(&tile.image),
                0,
                false,
            ) {
                state.complete_in_scene = false;
                window.paint_quad(fill(bounds, rgb(0x141820)));
                return Err(error);
            }
        }
        state.complete_in_scene = true;
        Ok(PaintStatus::CompleteInScene {
            tile_count: self.tiles.len(),
        })
    }
}

fn tile_bounds(bounds: Bounds<gpui::Pixels>, span: TileSpan) -> Bounds<gpui::Pixels> {
    let width: f32 = bounds.size.width.into();
    let height: f32 = bounds.size.height.into();
    let scale_x = width / FRAME_EDGE as f32;
    let scale_y = height / FRAME_EDGE as f32;
    let x0 = span.x as f32 * scale_x;
    let y0 = span.y as f32 * scale_y;
    let x1 = (span.x + span.width) as f32 * scale_x;
    let y1 = (span.y + span.height) as f32 * scale_y;
    Bounds::new(
        point(bounds.origin.x + px(x0), bounds.origin.y + px(y0)),
        size(px(x1 - x0), px(y1 - y0)),
    )
}

fn build_tiles(
    verified: &VerifiedRaster,
    cancel: &Cancellation,
) -> Result<Vec<PreparedTile>, ImageUploadError> {
    let schedule = schedule_tiles(FRAME_EDGE, TILE_EDGE, PERIOD_UPLOAD_CAP_BYTES)
        .ok_or(ImageUploadError::InvalidRasterLayout)?;
    if schedule.periods.len() > MAX_SCHEDULE_TILES as usize
        || !schedule.every_period_fits()
        || schedule.periods.is_empty()
    {
        return Err(ImageUploadError::InvalidRasterLayout);
    }
    let raster = verified.raster();
    let mut tiles = Vec::with_capacity(16);
    for period in &schedule.periods {
        let mut period_transfer_bytes = 0u64;
        for span in &period.tiles {
            if cancel.is_cancelled() {
                return Err(ImageUploadError::Cancelled);
            }
            let (row_pitch, transfer_bytes) = tile_transfer_layout(*span)?;
            if row_pitch % ROW_PITCH_ALIGNMENT != 0 {
                return Err(ImageUploadError::InvalidRasterLayout);
            }
            period_transfer_bytes = period_transfer_bytes
                .checked_add(transfer_bytes)
                .ok_or(ImageUploadError::InvalidRasterLayout)?;
            if period_transfer_bytes > PERIOD_UPLOAD_CAP_BYTES {
                return Err(ImageUploadError::InvalidRasterLayout);
            }
            let bgra = rgba_tile_to_bgra(
                &raster.rgba,
                raster.size.width(),
                raster.size.height(),
                *span,
                cancel,
            )?;
            let expected = usize::try_from(transfer_bytes)
                .map_err(|_| ImageUploadError::InvalidRasterLayout)?;
            if bgra.len() != expected || row_pitch != u64::from(span.width) * 4 {
                return Err(ImageUploadError::InvalidRasterLayout);
            }
            let buffer = RgbaImage::from_raw(span.width, span.height, bgra)
                .ok_or(ImageUploadError::InvalidRasterLayout)?;
            let image = Arc::new(RenderImage::new(vec![Frame::new(buffer)]));
            let image_bytes = image
                .as_bytes(0)
                .ok_or(ImageUploadError::InvalidRasterLayout)?;
            if image_bytes.len() != expected
                || u32::from(image.size(0).width) != span.width
                || u32::from(image.size(0).height) != span.height
            {
                return Err(ImageUploadError::InvalidRasterLayout);
            }
            tiles.push(PreparedTile {
                span: *span,
                image,
                transfer_bytes,
            });
        }
    }
    if tiles.len() != 16 {
        return Err(ImageUploadError::InvalidRasterLayout);
    }
    Ok(tiles)
}

fn tile_transfer_layout(span: TileSpan) -> Result<(u64, u64), ImageUploadError> {
    if span.width == 0 || span.height == 0 {
        return Err(ImageUploadError::InvalidRasterLayout);
    }
    let row_bytes = u64::from(span.width)
        .checked_mul(4)
        .ok_or(ImageUploadError::InvalidRasterLayout)?;
    let aligned_row = row_bytes
        .checked_add(ROW_PITCH_ALIGNMENT - 1)
        .map(|bytes| bytes / ROW_PITCH_ALIGNMENT * ROW_PITCH_ALIGNMENT)
        .ok_or(ImageUploadError::InvalidRasterLayout)?;
    let transfer_bytes = aligned_row
        .checked_mul(u64::from(span.height))
        .ok_or(ImageUploadError::InvalidRasterLayout)?;
    Ok((aligned_row, transfer_bytes))
}

fn rgba_tile_to_bgra(
    rgba: &[u8],
    frame_width: u32,
    frame_height: u32,
    span: TileSpan,
    cancel: &Cancellation,
) -> Result<Vec<u8>, ImageUploadError> {
    let right = span
        .x
        .checked_add(span.width)
        .ok_or(ImageUploadError::InvalidRasterLayout)?;
    let bottom = span
        .y
        .checked_add(span.height)
        .ok_or(ImageUploadError::InvalidRasterLayout)?;
    let source_stride = frame_width
        .checked_mul(4)
        .ok_or(ImageUploadError::InvalidRasterLayout)? as usize;
    let source_len = source_stride
        .checked_mul(frame_height as usize)
        .ok_or(ImageUploadError::InvalidRasterLayout)?;
    if right > frame_width || bottom > frame_height || rgba.len() != source_len {
        return Err(ImageUploadError::InvalidRasterLayout);
    }
    let row_bytes = (span.width as usize)
        .checked_mul(4)
        .ok_or(ImageUploadError::InvalidRasterLayout)?;
    let output_len = row_bytes
        .checked_mul(span.height as usize)
        .ok_or(ImageUploadError::InvalidRasterLayout)?;
    let mut bgra = Vec::new();
    bgra.try_reserve_exact(output_len)
        .map_err(|_| ImageUploadError::Allocation)?;
    bgra.resize(output_len, 0);
    for y in 0..span.height {
        if cancel.is_cancelled() {
            return Err(ImageUploadError::Cancelled);
        }
        let source_row = (span.y as usize + y as usize) * source_stride + span.x as usize * 4;
        let target_row = y as usize * row_bytes;
        let source = &rgba[source_row..source_row + row_bytes];
        let target = &mut bgra[target_row..target_row + row_bytes];
        let (source_pixels, source_remainder) = source.as_chunks::<4>();
        let (target_pixels, target_remainder) = target.as_chunks_mut::<4>();
        if !source_remainder.is_empty() || !target_remainder.is_empty() {
            return Err(ImageUploadError::InvalidRasterLayout);
        }
        for (src, dst) in source_pixels.iter().zip(target_pixels.iter_mut()) {
            // Pinned GPUI RenderImage payloads are BGRA; media::DecodedRaster is RGBA.
            *dst = [src[2], src[1], src[0], src[3]];
        }
    }
    Ok(bgra)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_tile_with_padded_rows_is_refused_by_the_smallest_period_cap() {
        let span = TileSpan {
            x: 0,
            y: 0,
            width: 724,
            height: 724,
        };
        let (row_pitch, transfer_bytes) = tile_transfer_layout(span).expect("aligned layout");
        assert_eq!(row_pitch, 3072);
        assert_eq!(transfer_bytes, 2_224_128);
        assert!(transfer_bytes > PERIOD_UPLOAD_CAP_BYTES);
    }

    #[test]
    fn admitted_tile_pitch_and_transfer_bytes_fit_with_alignment() {
        let span = TileSpan {
            x: 0,
            y: 0,
            width: TILE_EDGE,
            height: TILE_EDGE,
        };
        let (row_pitch, transfer_bytes) = tile_transfer_layout(span).expect("tile layout");
        assert_eq!(row_pitch, 2048);
        assert_eq!(row_pitch % ROW_PITCH_ALIGNMENT, 0);
        assert_eq!(transfer_bytes, 1024 * 1024);
        assert_eq!(transfer_bytes * 2, PERIOD_UPLOAD_CAP_BYTES);
        assert!(transfer_bytes * 3 > PERIOD_UPLOAD_CAP_BYTES);
    }

    #[test]
    fn actual_tiles_pack_in_process_periods_at_the_smallest_cap() {
        let schedule = schedule_tiles(FRAME_EDGE, TILE_EDGE, PERIOD_UPLOAD_CAP_BYTES)
            .expect("the aligned whole-image tiling exists");
        assert_eq!(schedule.period_count(), 8);
        assert!(schedule.every_period_fits());
        for period in &schedule.periods {
            let actual = period
                .tiles
                .iter()
                .map(|span| tile_transfer_layout(*span).expect("aligned span").1)
                .sum::<u64>();
            assert_eq!(actual, period.bytes);
            assert!(actual <= PERIOD_UPLOAD_CAP_BYTES);
        }
    }

    #[test]
    fn rgba_conversion_swaps_red_and_blue_and_preserves_alpha() {
        let rgba = [255, 0, 0, 17, 0, 0, 255, 231];
        let tile = TileSpan {
            x: 0,
            y: 0,
            width: 2,
            height: 1,
        };
        let bgra = rgba_tile_to_bgra(&rgba, 2, 1, tile, &Cancellation::default())
            .expect("the source rectangle is valid");
        assert_eq!(bgra, [0, 0, 255, 17, 255, 0, 0, 231]);
    }

    #[test]
    fn cancelled_tile_conversion_does_not_publish_partial_bytes() {
        let cancel = Cancellation::default();
        cancel.cancel();
        let rgba = [0; 8];
        let tile = TileSpan {
            x: 0,
            y: 0,
            width: 2,
            height: 1,
        };
        assert_eq!(
            rgba_tile_to_bgra(&rgba, 2, 1, tile, &cancel),
            Err(ImageUploadError::Cancelled)
        );
    }

    #[test]
    fn period_allowance_is_shared_and_never_exceeds_the_process_cap() {
        let mut process = PeriodAllowance::default();
        assert!(process.reserve(7, 1024 * 1024));
        // A second pane in the same period spends the same process allowance.
        assert!(process.reserve(7, 1024 * 1024));
        assert!(!process.reserve(7, 1));
        assert_eq!(process.reserved_bytes, PERIOD_UPLOAD_CAP_BYTES);
        assert!(process.reserve(8, 1024 * 1024));
        assert_eq!(process.reserved_bytes, 1024 * 1024);
    }

    #[test]
    fn request_identity_rejects_stale_generation_and_revision_before_decode() {
        let key = RepresentationKey {
            source: RepresentationSource::Local {
                provenance: media::LocalProvenance::Fixture {
                    fixture: media::LocalId::new("img3-test").expect("fixture id"),
                    manifest: media::LocalId::new("img3-manifest").expect("manifest id"),
                    source: media::LocalId::new("img3-source").expect("source id"),
                },
                revision: 4,
            },
            kind: desktop_protocol::RepresentationKind::Preview,
            size: media::PixelSize::new(FRAME_EDGE, FRAME_EDGE).expect("frame size"),
            orientation: media::Orientation::Normal,
            policy: media::DecodePolicy {
                decoder: media::LocalId::new("png-rgb8").expect("decoder id"),
                colour: media::LocalId::new("identity-rgb").expect("colour id"),
                generation: 1,
            },
        };
        let request = UploadRequest {
            key,
            source_revision: 3,
            device_generation: 1,
            current_device_generation: 2,
            decode_limits: media::DecodeBudget::ratified_default()
                .limits()
                .expect("declared decode limits"),
            max_encoded_bytes: 16 * 1024 * 1024,
        };
        assert!(matches!(
            validate_request(&request),
            Err(ImageUploadError::StaleDeviceGeneration {
                requested: 1,
                current: 2
            })
        ));
        let mut current = request;
        current.current_device_generation = 1;
        assert!(matches!(
            validate_request(&current),
            Err(ImageUploadError::SourceRevisionMismatch {
                key_revision: 4,
                requested: 3
            })
        ));
    }

    #[test]
    fn only_idle_untracked_generation_pools_are_dropped() {
        let idle = media::UploadUsage {
            staging: 0,
            in_flight: 0,
            resident: 0,
            staging_stuck: 0,
            in_flight_stuck: 0,
            resident_stuck: 0,
            unreclaimed: 0,
        };
        // An empty pool for a generation no retained frame owns can go.
        assert!(pool_is_idle(4, &idle, &[7]));
        // A generation a retained frame still owns stays, even when nothing is charged.
        assert!(!pool_is_idle(7, &idle, &[7]));
        // Charged or quarantined bytes keep the pool alive even without a retained frame.
        for usage in [
            media::UploadUsage { staging: 1, ..idle },
            media::UploadUsage {
                in_flight: 1,
                ..idle
            },
            media::UploadUsage {
                resident: 1,
                ..idle
            },
            media::UploadUsage {
                staging_stuck: 1,
                ..idle
            },
            media::UploadUsage {
                in_flight_stuck: 1,
                ..idle
            },
            media::UploadUsage {
                resident_stuck: 1,
                ..idle
            },
        ] {
            assert!(!pool_is_idle(4, &usage, &[]));
        }
    }

    #[test]
    fn consumers_gate_release_until_the_last_detach() {
        let service = ImageUploadService::global();
        let identity = media::UploadIdentity {
            key: RepresentationKey {
                source: RepresentationSource::Local {
                    provenance: media::LocalProvenance::Fixture {
                        fixture: media::LocalId::new("img4-test").expect("fixture id"),
                        manifest: media::LocalId::new("img4-manifest").expect("manifest id"),
                        source: media::LocalId::new("img4-source").expect("source id"),
                    },
                    revision: 1,
                },
                kind: desktop_protocol::RepresentationKind::Preview,
                size: media::PixelSize::new(FRAME_EDGE, FRAME_EDGE).expect("frame size"),
                orientation: media::Orientation::Normal,
                policy: media::DecodePolicy {
                    decoder: media::LocalId::new("png-rgb8").expect("decoder id"),
                    colour: media::LocalId::new("identity-rgb").expect("colour id"),
                    generation: 1,
                },
            },
            source_revision: 1,
            device_generation: 7,
            content_digest: [0u8; 32],
        };
        let frame = Arc::new(PreparedImage {
            identity,
            tiles: Vec::new(),
            admission: Mutex::new(None),
            paint_state: Mutex::new(PaintState::default()),
        });
        // A frame nobody retained can never gain a consumer.
        assert!(!service.attach(&frame, ConsumerId::new(1)));
        service
            .inner
            .state
            .lock()
            .expect("image upload service state")
            .frames
            .push(Arc::clone(&frame));
        let first = ConsumerId::new(1);
        let second = ConsumerId::new(2);
        assert!(service.attach(&frame, first));
        assert!(service.attach(&frame, second));
        assert_eq!(service.consumers(&frame), 2);
        assert!(service.detach(&frame, first));
        // The shared frame survives one pane closing: the second consumer still holds it.
        assert_eq!(service.consumers(&frame), 1);
        assert!(service.detach(&frame, second));
        assert_eq!(service.consumers(&frame), 0);
        // Detaching twice is reported, not silently accepted.
        assert!(!service.detach(&frame, second));
        // A detached frame is still retained until an eviction runs on the owning window.
        assert!(service.counters().retained_frames >= 1);
        service
            .inner
            .state
            .lock()
            .expect("image upload service state")
            .frames
            .retain(|candidate| !Arc::ptr_eq(candidate, &frame));
    }

    #[test]
    fn admitted_class_never_exceeds_the_schedule_tile_ceiling() {
        let schedule = schedule_tiles(FRAME_EDGE, TILE_EDGE, PERIOD_UPLOAD_CAP_BYTES)
            .expect("bounded schedule");
        assert!(schedule.periods.len() <= MAX_SCHEDULE_TILES as usize);
        assert_eq!(
            schedule
                .periods
                .iter()
                .map(|period| period.tiles.len())
                .sum::<usize>(),
            16
        );
    }
}
