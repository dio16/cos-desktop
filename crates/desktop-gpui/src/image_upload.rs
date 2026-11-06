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
/// Process-wide cap for payloads quarantined after an unproven transfer, across every device
/// generation. It matches the ratified 64 MiB staging/in-flight budget: once it is reached, new
/// work is refused rather than any unproven payload being released.
pub const PROCESS_QUARANTINE_CAP_BYTES: u64 = 64 * 1024 * 1024;
static QUARANTINE_CAPACITY: std::sync::OnceLock<u64> = std::sync::OnceLock::new();

/// Set the process-wide quarantine capacity before the service is first used.
///
/// The ratified default is PROCESS_QUARANTINE_CAP_BYTES; an operator or a probe may select a
/// smaller value so the boundary can be exercised. Returns false once it has been set.
pub fn set_quarantine_capacity(bytes: u64) -> bool {
    QUARANTINE_CAPACITY.set(bytes).is_ok()
}

/// The effective process-wide quarantine capacity.
pub fn quarantine_capacity() -> u64 {
    *QUARANTINE_CAPACITY.get_or_init(|| PROCESS_QUARANTINE_CAP_BYTES)
}
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
    /// Quarantined payloads already occupy the process-wide quarantine allowance, so neither a
    /// new admission nor another in-flight release can be accepted.
    QuarantineBudgetExhausted {
        retained: u64,
        cap: u64,
    },
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
    /// Window::drop_image calls made (one per tile). GPUI accepts a drop for an entry that does
    /// not exist, so this is a call count, not a proven removal count.
    pub drop_calls: usize,
    /// CPU payload bytes released because the admission had never begun a transfer.
    pub released_bytes: u64,
    /// CPU payload bytes left charged to the quarantine because a transfer had already begun and
    /// termination is not measurable. These stay reserved for the life of the process (or until a
    /// real device teardown can be proven, which this module does not have), and the process
    /// budget refuses new work instead of releasing them.
    pub quarantined_bytes: u64,
}

/// What a window-close sweep evicted and what it had to leave retained.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EvictionSummary {
    pub evicted_frames: usize,
    /// Frames this window owns that could not be released in this sweep: a consumer still
    /// presents them, or a paint callback held them.
    pub skipped_frames: usize,
    pub drop_calls: usize,
    pub released_bytes: u64,
    pub quarantined_bytes: u64,
    /// Frames whose paint state was locked by a callback during the sweep: their owning window
    /// cannot be attributed safely, so they are reported separately instead of being charged to
    /// this window's skipped count.
    pub unattributed_frames: usize,
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
    /// Window::drop_image calls made by eviction (one per tile of every evicted frame); a call
    /// count, not a proven removal count.
    pub atlas_drop_calls: u64,
    /// CPU payload bytes released because the admission had never begun a transfer.
    pub released_payload_bytes: u64,
    /// Payload bytes currently quarantined across every device generation.
    pub quarantined_payload_bytes: u64,
    pub quarantine_capacity_bytes: u64,
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
    atlas_drop_calls: u64,
    released_payload_bytes: u64,
    quarantined_payload_bytes: u64,
    /// Viewer mip payloads with no transfer-completion proof stay owned until process exit.
    quarantined_views: Vec<Arc<PreparedImage>>,
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
        self.prepare_with_mips(background, request, cancel, source, None)
    }

    /// Decode the admitted source off-thread. Viewer tiles are prepared only for later viewport
    /// demand; preview-only restricts that demand to levels 2–4 without preparing detail.
    /// This does not widen source admission or establish large-image residency capability.
    pub fn prepare_view_generated(
        &self,
        background: &BackgroundExecutor,
        request: UploadRequest,
        cancel: Cancellation,
        preview_only: bool,
        source: impl FnOnce(&Cancellation) -> Result<Arc<[u8]>, MediaError> + Send + 'static,
    ) -> Task<Result<Arc<PreparedImage>, ImageUploadError>> {
        self.prepare_with_mips(
            background,
            request,
            cancel,
            source,
            Some(if preview_only { 2 } else { 0 }),
        )
    }

    fn prepare_with_mips(
        &self,
        background: &BackgroundExecutor,
        request: UploadRequest,
        cancel: Cancellation,
        source: impl FnOnce(&Cancellation) -> Result<Arc<[u8]>, MediaError> + Send + 'static,
        minimum_mip: Option<u8>,
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
            service.prepare_verified(verified, request, &cancel, minimum_mip)
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
        minimum_mip: Option<u8>,
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
        let mips = minimum_mip.is_some();
        let identity = UploadIdentity {
            key: request.key.clone(),
            source_revision: request.source_revision,
            device_generation: request.device_generation,
            content_digest: verified.content_digest(),
        };
        // One process-wide admission decision: live staging/in-flight reservations of **every**
        // generation plus the quarantine ledger are summed under the service lock, and the
        // admission itself happens inside that lock, so two generations cannot each pass a
        // per-pool check and together exceed the process budget.
        let admission = {
            let mut state = self.inner.state.lock().expect("image upload service state");
            if let Some(existing) = state.frames.iter().find(|frame| frame.identity == identity) {
                if (mips && !existing.has_mips) || existing.first_mip > minimum_mip.unwrap_or(0) {
                    return Err(ImageUploadError::InvalidRasterLayout);
                }
                return Ok(Arc::clone(existing));
            }
            if state.frames.len() >= MAX_RETAINED_FRAMES {
                return Err(ImageUploadError::RetainedFrameLimit);
            }
            // Count actual raster and packed-tile owners before extending the viewer pyramid. The
            // whole-frame route keeps its pool-level refusal so its bounded-refusal evidence is
            // unchanged; only a viewer frame (or an existing mip owner) uses this CPU ceiling.
            if mips
                || !state.quarantined_views.is_empty()
                || state.frames.iter().any(|frame| frame.has_mips)
            {
                let incoming = FRAME_RGBA8_BYTES + if mips { 0 } else { FRAME_RGBA8_BYTES };
                if retained_cpu_bytes(&state).saturating_add(incoming) > STAGING_LIMIT_BYTES * 2 {
                    return Err(ImageUploadError::RetainedFrameLimit);
                }
            }
            prune_idle_pools(&mut state);
            // Per-category totals across **every** generation. A pool's own limit is per pool, so
            // without this sum two generations could each hold 32 MiB under a 64 MiB budget. The
            // quarantine ledger is tracked separately; its payloads are already the stuck bytes of
            // their pools, so adding it here would double count them.
            let mut staging_total = 0u64;
            let mut in_flight_total = 0u64;
            for (_, pool) in &state.pools {
                let usage = pool.usage();
                staging_total = staging_total
                    .saturating_add(usage.staging)
                    .saturating_add(usage.staging_stuck);
                in_flight_total = in_flight_total
                    .saturating_add(usage.in_flight)
                    .saturating_add(usage.in_flight_stuck);
            }
            let capacity = quarantine_capacity();
            if let Some(retained) = budget_breach(
                staging_total,
                in_flight_total,
                state.quarantined_payload_bytes,
                capacity,
                FRAME_RGBA8_BYTES,
            ) {
                return Err(ImageUploadError::QuarantineBudgetExhausted {
                    retained,
                    cap: capacity,
                });
            }
            let pool = state
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
                });
            admit_upload(
                &verified,
                identity.clone(),
                request.current_device_generation,
                &pool,
                cancel,
            )
            .map_err(ImageUploadError::Admission)?
        };
        let tiles = if mips {
            Vec::new()
        } else {
            build_tiles(&verified, cancel, 0, None)?
        };
        if cancel.is_cancelled() {
            return Err(ImageUploadError::Cancelled);
        }
        let issued = vec![false; tiles.len()];
        let prepared = Arc::new(PreparedImage {
            identity,
            source: mips.then_some(verified),
            has_mips: mips,
            first_mip: minimum_mip.unwrap_or(0),
            admission: Mutex::new(Some(admission)),
            paint_state: Mutex::new(PaintState {
                issued,
                tiles,
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
    /// Prepare only this viewport's missing complete tiles, never on the input/paint thread.
    pub(crate) fn prepare_visible(
        &self,
        background: &BackgroundExecutor,
        frame: Arc<PreparedImage>,
        demand: TileDemand,
        cancel: Cancellation,
    ) -> Task<Result<(), ImageUploadError>> {
        let service = self.clone();
        background.spawn(async move { service.prepare_visible_now(&frame, &demand, &cancel) })
    }

    fn prepare_visible_now(
        &self,
        frame: &Arc<PreparedImage>,
        demand: &TileDemand,
        cancel: &Cancellation,
    ) -> Result<(), ImageUploadError> {
        if cancel.is_cancelled() {
            return Err(ImageUploadError::Cancelled);
        }
        let _preparation = match PROCESS_PREPARATION.try_lock() {
            Ok(guard) => guard,
            Err(std::sync::TryLockError::WouldBlock) => {
                return Err(ImageUploadError::PreparationBusy);
            }
            Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
        };
        let source = frame
            .source
            .as_ref()
            .ok_or(ImageUploadError::InvalidRasterLayout)?;
        let missing = {
            let state = self.inner.state.lock().expect("image upload service state");
            if !state
                .frames
                .iter()
                .any(|retained| Arc::ptr_eq(retained, frame))
            {
                return Err(ImageUploadError::NotRetained);
            }
            let missing = {
                let paint = frame.paint_state.lock().expect("image frame paint state");
                demand
                    .spans
                    .iter()
                    .copied()
                    .filter(|span| {
                        !paint
                            .tiles
                            .iter()
                            .any(|tile| tile.level == demand.level && tile.span == *span)
                    })
                    .collect::<Vec<_>>()
            };
            let incoming = missing.iter().try_fold(0u64, |sum, span| {
                tile_transfer_layout(*span).map(|(_, bytes)| sum.saturating_add(bytes))
            })?;
            if retained_cpu_bytes(&state).saturating_add(incoming) > STAGING_LIMIT_BYTES * 2 {
                return Err(ImageUploadError::RetainedFrameLimit);
            }
            missing
        };
        if missing.is_empty() {
            return Ok(());
        }
        let mip = source.mip(demand.level, cancel).map_err(map_media_error)?;
        let tiles = build_tiles(&mip, cancel, demand.level, Some(&missing))?;
        let service = self.inner.state.lock().expect("image upload service state");
        if cancel.is_cancelled() {
            return Err(ImageUploadError::Cancelled);
        }
        if !service
            .frames
            .iter()
            .any(|retained| Arc::ptr_eq(retained, frame))
        {
            return Err(ImageUploadError::NotRetained);
        }
        let mut paint = frame.paint_state.lock().expect("image frame paint state");
        paint.issued.extend(std::iter::repeat_n(false, tiles.len()));
        paint.tiles.extend(tiles);
        Ok(())
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
    /// The registry check and the consumer insertion happen under the same service lock that
    /// `evict` takes, so a concurrent eviction can never remove the frame between the check and
    /// the registration. Returns `false` when the frame is no longer retained (already evicted) —
    /// a consumer never resurrects a released resource.
    pub fn attach(
        &self,
        frame: &Arc<PreparedImage>,
        consumer: ConsumerId,
        window: Option<AnyWindowHandle>,
    ) -> bool {
        let service = self.inner.state.lock().expect("image upload service state");
        if !service
            .frames
            .iter()
            .any(|candidate| Arc::ptr_eq(candidate, frame))
        {
            return false;
        }
        let mut paint_state = frame.paint_state.lock().expect("image frame paint state");
        if let Some(window) = window {
            match paint_state.owner_window {
                Some(owner) if owner != window => return false,
                _ => paint_state.owner_window = Some(window),
            }
        }
        // Re-attaching the same consumer is idempotent: the frame is retained and that consumer
        // already owns it, so the call reports success rather than a spurious failure.
        paint_state.consumers.insert(consumer);
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
    /// The caller must have stopped presenting it: this removes the window's atlas entries and
    /// drops the admission. A payload that never began a transfer is released; one whose transfer
    /// already began is quarantined and **stays charged** because no completion signal exists, and
    /// eviction never asserts termination. The process budget refuses new work at the cap.
    ///
    /// The previously painted scene may still reference a removed sprite until the next frame is
    /// composed; that is why eviction is a caller decision taken *after* the frame stopped being
    /// presented, not a replacement for switching the surface first.
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
        // A payload whose transfer already began cannot be released safely, so eviction is refused
        // before any atlas entry is dropped once the process-wide quarantine is full.
        if let Some(admission) = frame
            .admission
            .lock()
            .expect("image frame admission")
            .as_ref()
            && admission.stage() != UploadStage::Staged
        {
            let retained = service.quarantined_payload_bytes;
            let bytes = admission.bytes();
            if retained.saturating_add(bytes) > quarantine_capacity() {
                return Err(ImageUploadError::QuarantineBudgetExhausted {
                    retained,
                    cap: quarantine_capacity(),
                });
            }
        }
        if let Some(owner) = paint_state.owner_window {
            if owner != window.window_handle() {
                return Err(ImageUploadError::NotOwnedByWindow);
            }
            for tile in &paint_state.tiles {
                window
                    .drop_image(Arc::clone(&tile.image))
                    .map_err(|_| ImageUploadError::AtlasRelease)?;
                outcome.drop_calls = outcome.drop_calls.saturating_add(1);
            }
        }
        paint_state.complete_in_scene = false;
        let mut admission = frame.admission.lock().expect("image frame admission");
        if let Some(mut admitted) = admission.take() {
            let bytes = admitted.bytes();
            if admitted.stage() == UploadStage::Staged {
                if admitted.release().is_ok() {
                    outcome.released_bytes = bytes;
                }
            } else {
                // Dropping an in-flight admission quarantines its payload and keeps it charged to
                // the pool. `drop_image` only removes an atlas entry; it does not prove the GPU
                // stopped reading the bytes, so eviction never frees them. The process-wide ledger
                // bounds how much may stay retained and refuses new work at the cap.
                drop(admitted);
                outcome.quarantined_bytes = bytes;
                service.quarantined_payload_bytes =
                    service.quarantined_payload_bytes.saturating_add(bytes);
                if frame.has_mips {
                    service.quarantined_views.push(Arc::clone(frame));
                }
            }
        }
        drop(admission);
        service
            .frames
            .retain(|candidate| !Arc::ptr_eq(candidate, frame));
        service.evicted_frames = service.evicted_frames.saturating_add(1);
        service.atlas_drop_calls = service
            .atlas_drop_calls
            .saturating_add(outcome.drop_calls as u64);
        service.released_payload_bytes = service
            .released_payload_bytes
            .saturating_add(outcome.released_bytes);
        drop(paint_state);
        prune_idle_pools(&mut service);
        Ok(outcome)
    }

    /// Evict every frame this window owns that has no remaining consumer.
    ///
    /// Window close calls this before the platform destroys the atlas. A frame another window owns
    /// is not this sweep's resource. A frame this window owns while a consumer still presents it is
    /// **counted as skipped** rather than silently ignored, so a close can never strand a resource
    /// without a number that says so; the caller must detach its own consumers before closing.
    pub fn evict_owned_by(&self, window: &mut Window) -> EvictionSummary {
        let handle = window.window_handle();
        let (candidates, blocked, unattributed): (Vec<Arc<PreparedImage>>, usize, usize) = {
            let service = self.inner.state.lock().expect("image upload service state");
            let mut candidates = Vec::new();
            let mut blocked = 0usize;
            let mut unattributed = 0usize;
            for frame in &service.frames {
                let Ok(state) = frame.paint_state.try_lock() else {
                    unattributed = unattributed.saturating_add(1);
                    continue;
                };
                if state.owner_window != Some(handle) {
                    continue;
                }
                if state.consumers.is_empty() {
                    candidates.push(Arc::clone(frame));
                } else {
                    blocked = blocked.saturating_add(1);
                }
            }
            (candidates, blocked, unattributed)
        };
        let mut summary = EvictionSummary {
            skipped_frames: blocked,
            unattributed_frames: unattributed,
            ..EvictionSummary::default()
        };
        for frame in candidates {
            match self.evict(&frame, window) {
                Ok(outcome) => {
                    summary.evicted_frames = summary.evicted_frames.saturating_add(1);
                    summary.drop_calls = summary.drop_calls.saturating_add(outcome.drop_calls);
                    summary.released_bytes = summary
                        .released_bytes
                        .saturating_add(outcome.released_bytes);
                    summary.quarantined_bytes = summary
                        .quarantined_bytes
                        .saturating_add(outcome.quarantined_bytes);
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
        // A shared frame is released only after its last consumer detaches: the same ownership
        // rule guards every release path, not only `evict`.
        if !paint_state.consumers.is_empty() {
            return false;
        }
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
            atlas_drop_calls: state.atlas_drop_calls,
            released_payload_bytes: state.released_payload_bytes,
            quarantined_payload_bytes: state.quarantined_payload_bytes,
            quarantine_capacity_bytes: quarantine_capacity(),
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

fn retained_cpu_bytes(state: &ServiceState) -> u64 {
    let raw: u64 = state
        .pools
        .iter()
        .map(|(_, pool)| {
            let usage = pool.usage();
            usage.resident.saturating_add(usage.resident_stuck)
        })
        .sum();
    let packed: u64 = state
        .frames
        .iter()
        .chain(&state.quarantined_views)
        .map(|frame| {
            frame
                .paint_state
                .lock()
                .expect("image frame paint state")
                .tiles
                .iter()
                .map(|tile| tile.transfer_bytes)
                .sum::<u64>()
        })
        .sum();
    raw.saturating_add(packed)
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

/// Whether admitting `incoming` more bytes would exceed the process budget.
///
/// Three axes are checked: the quarantine ledger, the summed staging reservations of every
/// generation and the summed in-flight reservations of every generation. Returns the retained
/// figure that the caller should report when the boundary is crossed.
fn budget_breach(
    staging_total: u64,
    in_flight_total: u64,
    quarantined: u64,
    capacity: u64,
    incoming: u64,
) -> Option<u64> {
    let retained = quarantined.max(staging_total).max(in_flight_total);
    let exceeds = |total: u64| total.saturating_add(incoming) > capacity;
    if exceeds(retained) || exceeds(staging_total) || exceeds(in_flight_total) {
        Some(retained)
    } else {
        None
    }
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
    level: u8,
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
    /// Allocate one process-unique pane/consumer identity, independent of its window.
    pub fn allocate() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

#[derive(Default)]
struct PaintState {
    owner_window: Option<AnyWindowHandle>,
    tiles: Vec<PreparedTile>,
    issued: Vec<bool>,
    complete_in_scene: bool,
    cancelled_before_upload: bool,
    /// Consumers currently presenting this frame. Eviction requires this to be empty.
    consumers: std::collections::BTreeSet<ConsumerId>,
}

/// Verified source and a bounded cache of complete tiles, tied to one revision/device identity.
pub struct PreparedImage {
    identity: UploadIdentity,
    source: Option<VerifiedRaster>,
    has_mips: bool,
    first_mip: u8,
    admission: Mutex<Option<media::AdmittedUpload>>,
    paint_state: Mutex<PaintState>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TileDemand {
    level: u8,
    spans: Vec<TileSpan>,
}

impl PreparedImage {
    pub fn identity(&self) -> &UploadIdentity {
        &self.identity
    }

    pub(crate) fn demand(
        &self,
        image_bounds: Bounds<gpui::Pixels>,
        viewport: Bounds<gpui::Pixels>,
        level: u8,
    ) -> TileDemand {
        let level = level.clamp(self.first_mip, 4);
        TileDemand {
            level,
            spans: visible_spans(image_bounds, viewport, level).collect(),
        }
    }

    pub fn progress(&self) -> FrameProgress {
        let state = self.paint_state.lock().expect("image frame paint state");
        FrameProgress {
            source_revision: self.identity.source_revision,
            device_generation: self.identity.device_generation,
            issued_tiles: state.issued.iter().filter(|issued| **issued).count(),
            tile_count: state.tiles.len(),
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
        self.paint_view(
            ViewPaint {
                image_bounds: bounds,
                viewport: bounds,
                mip_level: 0,
                source_revision: current_source_revision,
                device_generation: current_device_generation,
                cancel,
            },
            window,
        )
    }

    /// Only intersecting complete tiles are requested, with zero overscan and no pending queue.
    /// A new viewport replaces old demand before any process-wide upload reservation is taken.
    pub fn paint_view(
        &self,
        view: ViewPaint<'_>,
        window: &mut Window,
    ) -> gpui::Result<PaintStatus> {
        let ViewPaint {
            image_bounds,
            viewport,
            mip_level,
            source_revision,
            device_generation,
            cancel,
        } = view;
        window.paint_quad(fill(viewport, rgb(0x141820)));
        let paint_period = current_period_index();
        let mut state = self.paint_state.lock().expect("image frame paint state");
        state.complete_in_scene = false;
        if cancel.is_cancelled() || state.cancelled_before_upload {
            return Ok(PaintStatus::Cancelled);
        }
        if self.identity.source_revision != source_revision {
            return Ok(PaintStatus::StaleRevision {
                requested: self.identity.source_revision,
                current: source_revision,
            });
        }
        if self.identity.device_generation != device_generation {
            return Ok(PaintStatus::StaleDeviceGeneration {
                requested: self.identity.device_generation,
                current: device_generation,
            });
        }
        if image_bounds.size.width <= gpui::Pixels::ZERO
            || image_bounds.size.height <= gpui::Pixels::ZERO
            || viewport.size.width <= gpui::Pixels::ZERO
            || viewport.size.height <= gpui::Pixels::ZERO
        {
            return Ok(PaintStatus::EmptyBounds);
        }
        let window_id = window.window_handle();
        if state.owner_window.is_some_and(|owner| owner != window_id) {
            return Ok(PaintStatus::DifferentWindow);
        }
        let mut admission = self.admission.lock().expect("image frame admission");
        let Some(admission) = admission.as_mut() else {
            return Ok(PaintStatus::Evicted);
        };
        let level = if self.has_mips {
            mip_level.clamp(self.first_mip, 4)
        } else {
            0
        };
        let mut visible = 0;
        let mut missing = 0;
        let mut owner = state.owner_window;
        let PaintState { tiles, issued, .. } = &mut *state;
        let result =
            window.with_content_mask(Some(gpui::ContentMask { bounds: viewport }), |window| {
                for span in visible_spans(image_bounds, viewport, level) {
                    visible += 1;
                    let Some((index, tile)) = tiles
                        .iter()
                        .enumerate()
                        .find(|(_, tile)| tile.level == level && tile.span == span)
                    else {
                        missing += 1;
                        continue;
                    };
                    let bounds = tile_bounds(image_bounds, span, level);
                    if !issued[index] {
                        if cancel.is_cancelled() {
                            return Ok(PaintStatus::Cancelled);
                        }
                        if !period_still_current(paint_period)
                            || !reserve_process_period(paint_period, tile.transfer_bytes)
                        {
                            window.request_animation_frame();
                            return Ok(PaintStatus::AdmissionUnavailable);
                        }
                        if admission.stage() == UploadStage::Staged
                            && admission.begin_upload().is_err()
                        {
                            return Ok(PaintStatus::AdmissionUnavailable);
                        }
                        window.paint_image(
                            bounds,
                            bounds,
                            Corners::default(),
                            Arc::clone(&tile.image),
                            0,
                            false,
                        )?;
                        owner = Some(window_id);
                        issued[index] = true;
                        note_process_paint_issued(tile.transfer_bytes);
                    } else {
                        // Atlas cache hit under the caller-owned device-generation invariant.
                        window.paint_image(
                            bounds,
                            bounds,
                            Corners::default(),
                            Arc::clone(&tile.image),
                            0,
                            false,
                        )?;
                        note_process_repaint(1);
                    }
                }
                if missing > 0 {
                    return Ok(PaintStatus::Pending {
                        issued_tiles: visible - missing,
                        tile_count: visible,
                    });
                }
                Ok(PaintStatus::CompleteInScene {
                    tile_count: visible,
                })
            });
        state.owner_window = owner;
        match result {
            Ok(PaintStatus::CompleteInScene { .. }) => state.complete_in_scene = true,
            _ => window.paint_quad(fill(viewport, rgb(0x141820))),
        }
        result
    }
}

/// Geometry is DIP; the transform owner performs the physical-pixel conversion.
pub struct ViewPaint<'a> {
    pub image_bounds: Bounds<gpui::Pixels>,
    pub viewport: Bounds<gpui::Pixels>,
    pub mip_level: u8,
    pub source_revision: u64,
    pub device_generation: u64,
    pub cancel: &'a Cancellation,
}

fn visible_spans(
    image: Bounds<gpui::Pixels>,
    viewport: Bounds<gpui::Pixels>,
    level: u8,
) -> impl Iterator<Item = TileSpan> {
    let edge = FRAME_EDGE >> level;
    (0..edge)
        .step_by(TILE_EDGE as usize)
        .flat_map(move |y| {
            (0..edge)
                .step_by(TILE_EDGE as usize)
                .map(move |x| TileSpan {
                    x,
                    y,
                    width: TILE_EDGE.min(edge - x),
                    height: TILE_EDGE.min(edge - y),
                })
        })
        .filter(move |span| tile_bounds(image, *span, level).intersects(&viewport))
}

fn tile_bounds(bounds: Bounds<gpui::Pixels>, span: TileSpan, level: u8) -> Bounds<gpui::Pixels> {
    let width: f32 = bounds.size.width.into();
    let height: f32 = bounds.size.height.into();
    let edge = (FRAME_EDGE >> level) as f32;
    let scale_x = width / edge;
    let scale_y = height / edge;
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
    level: u8,
    wanted: Option<&[TileSpan]>,
) -> Result<Vec<PreparedTile>, ImageUploadError> {
    let edge = verified.size().width();
    let schedule = schedule_tiles(edge, TILE_EDGE.min(edge), PERIOD_UPLOAD_CAP_BYTES)
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
            if wanted.is_some_and(|wanted| !wanted.contains(span)) {
                continue;
            }
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
                level,
                span: *span,
                image,
                transfer_bytes,
            });
        }
    }
    if tiles.len() != wanted.map_or(edge.div_ceil(TILE_EDGE).pow(2) as usize, <[TileSpan]>::len) {
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

    /// Serializes the two tests that hold the process preparation guard.
    static PREPARATION_SERIAL: Mutex<()> = Mutex::new(());
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
    fn the_process_budget_boundary_is_inclusive_and_per_axis() {
        let capacity = 4096u64;
        let incoming = 1024u64;
        // The largest total that still admits `incoming` bytes exactly at the cap.
        let exact = capacity - incoming;
        let over = exact + 1;
        for axis in 0..3 {
            let below = match axis {
                0 => (exact - 1, 0, 0),
                1 => (0, exact - 1, 0),
                _ => (0, 0, exact - 1),
            };
            let at = match axis {
                0 => (exact, 0, 0),
                1 => (0, exact, 0),
                _ => (0, 0, exact),
            };
            let beyond = match axis {
                0 => (over, 0, 0),
                1 => (0, over, 0),
                _ => (0, 0, over),
            };
            assert_eq!(
                budget_breach(below.0, below.1, below.2, capacity, incoming),
                None,
                "axis {axis}: below the boundary is allowed"
            );
            assert_eq!(
                budget_breach(at.0, at.1, at.2, capacity, incoming),
                None,
                "axis {axis}: exactly at the boundary is allowed"
            );
            assert_eq!(
                budget_breach(beyond.0, beyond.1, beyond.2, capacity, incoming),
                Some(over),
                "axis {axis}: one byte over the boundary is refused"
            );
        }
        // All three axes exactly at the cap still admit; the largest axis is what gets reported.
        assert_eq!(
            budget_breach(exact, exact, exact, capacity, incoming),
            None,
            "three axes exactly at the cap still admit"
        );
        assert_eq!(
            budget_breach(exact, over, 0, capacity, incoming),
            Some(over),
            "the reported figure is the largest axis"
        );
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
            source: None,
            has_mips: false,
            first_mip: 0,
            admission: Mutex::new(None),
            paint_state: Mutex::new(PaintState::default()),
        });
        // A frame nobody retained can never gain a consumer.
        assert!(!service.attach(&frame, ConsumerId::allocate(), None));
        service
            .inner
            .state
            .lock()
            .expect("image upload service state")
            .frames
            .push(Arc::clone(&frame));
        let first = ConsumerId::allocate();
        let second = ConsumerId::allocate();
        assert!(service.attach(&frame, first, None));
        // Re-attaching the same consumer is idempotent success, not a spurious failure.
        assert!(service.attach(&frame, first, None));
        assert!(service.attach(&frame, second, None));
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

    #[test]
    fn a_concurrent_preparation_is_reported_as_busy() {
        let _serial = PREPARATION_SERIAL.lock().expect("test serialization");
        let service = ImageUploadService::global();
        let frame = demand_frame(0);
        let image = Bounds::new(point(px(0.0), px(0.0)), size(px(2048.0), px(2048.0)));
        let demand = frame.demand(image, image, 0);
        // Holding the process preparation guard models another viewer decoding at the same time.
        let _guard = PROCESS_PREPARATION.lock().expect("preparation guard");
        assert!(matches!(
            service.prepare_visible_now(&frame, &demand, &Cancellation::default()),
            Err(ImageUploadError::PreparationBusy)
        ));
    }

    #[test]
    fn a_frame_without_a_retained_source_cannot_prepare_tiles() {
        let _serial = PREPARATION_SERIAL.lock().expect("test serialization");
        let service = ImageUploadService::global();
        let frame = demand_frame(0);
        let image = Bounds::new(point(px(0.0), px(0.0)), size(px(2048.0), px(2048.0)));
        let demand = frame.demand(image, image, 0);
        // A permanent configuration failure is distinct from the transient busy result.
        assert!(matches!(
            service.prepare_visible_now(&frame, &demand, &Cancellation::default()),
            Err(ImageUploadError::InvalidRasterLayout)
        ));
    }

    #[test]
    fn allocated_consumers_are_unique_per_pane() {
        let first = ConsumerId::allocate();
        let second = ConsumerId::allocate();
        assert_ne!(first, second);
        assert_ne!(second, ConsumerId::allocate());
    }

    #[test]
    fn visible_demand_selects_only_intersecting_complete_tiles() {
        let frame = demand_frame(0);
        let image = Bounds::new(point(px(0.0), px(0.0)), size(px(2048.0), px(2048.0)));
        let corner = Bounds::new(point(px(0.0), px(0.0)), size(px(500.0), px(500.0)));
        let demand = frame.demand(image, corner, 0);
        assert_eq!(demand.level, 0);
        assert_eq!(demand.spans.len(), 1);
        assert_eq!((demand.spans[0].x, demand.spans[0].y), (0, 0));
        // The whole image at a downscaled level is one complete tile, not the 16 detail tiles.
        let preview = frame.demand(image, image, 2);
        assert_eq!((preview.level, preview.spans.len()), (2, 1));
        assert_eq!(
            (preview.spans[0].width, preview.spans[0].height),
            (512, 512)
        );
        // A preview-only frame never prepares detail below its first retained level.
        assert_eq!(demand_frame(2).demand(image, image, 0).level, 2);
    }

    fn demand_frame(first_mip: u8) -> Arc<PreparedImage> {
        Arc::new(PreparedImage {
            identity: media::UploadIdentity {
                key: RepresentationKey {
                    source: RepresentationSource::Local {
                        provenance: media::LocalProvenance::Fixture {
                            fixture: media::LocalId::new("img5-demand").expect("fixture id"),
                            manifest: media::LocalId::new("img5-manifest").expect("manifest id"),
                            source: media::LocalId::new("img5-source").expect("source id"),
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
                device_generation: 1,
                content_digest: [0u8; 32],
            },
            source: None,
            has_mips: true,
            first_mip,
            admission: Mutex::new(None),
            paint_state: Mutex::new(PaintState::default()),
        })
    }
}
