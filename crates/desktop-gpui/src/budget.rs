//! Declared provisional rendering budgets and their field-wise override.
//!
//! Every value here is an owner-ratified planning default, **not** a measured
//! result, and every one is replaceable: a caller resolves a
//! [`RenderBudgetsOverride`] over these defaults (or over another explicit
//! base) instead of editing a constant or hardcoding the number at a use
//! site. Resolution is pure and refuses an invalid value with
//! [`RenderBudgetError`] rather than clamping it.
//!
//! This module is plain data: it owns no GPUI resource, allocates nothing,
//! performs no I/O and does no device or worker work. A consumer that owns
//! media decode, GPU upload or scheduling takes the resolved values as data
//! and keeps its own resource ownership.
//!
//! No `desktop-gpui` use site hardcodes one of these numbers today. When the
//! media, GPU upload or bounded-scheduling work lands, it reads the resolved
//! budget instead of restating a value; an existing limit that is not one of
//! these budgets (a trace quota, dock geometry) keeps owning its own constant.
//!
//! The minimum upload unit below is a **declared assumption** — one 256 x 256
//! RGBA8 tile — not a measurement, and these rules follow from it: a
//! per-frame upload cap may not be smaller than one unit, and the upload
//! staging budget, the combined in-flight decoded/upload payload budget and
//! the image/graph GPU cache must each be able to hold at least one unit. A
//! smaller unit would have to be proven before any of those rules is relaxed.

/// Smallest indivisible upload unit: one 256 x 256 RGBA8 tile, a declared
/// assumption rather than a measurement.
///
/// A per-frame upload cap below this could never admit one complete unit, and
/// staging, the combined in-flight payload or the image/graph GPU cache below
/// it could never make progress at all. Resolution refuses those combinations
/// instead of clamping them; a smaller unit would have to be proven before the
/// rule is relaxed.
pub const MIN_UPLOAD_UNIT_BYTES: u64 = 262_144;

const MIB: u64 = 1024 * 1024;

/// Frame-rate classes whose per-frame upload caps are declared independently.
///
/// The two caps are separate values on purpose: neither may be derived from,
/// or silently substituted for, the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameRate {
    Hz60,
    Hz120,
}

impl FrameRate {
    #[must_use]
    pub const fn hz(self) -> u16 {
        match self {
            Self::Hz60 => 60,
            Self::Hz120 => 120,
        }
    }
}

/// Declared provisional render budgets for one process and window.
///
/// The byte budgets are absolute ceilings, not per-item sizes: thumbnails and
/// previews share the CPU decoded-image cache, and tiles and previews share
/// the image/graph GPU cache. The count fields bound concurrency and queue
/// depth, never the total work a viewport may request over time.
///
/// A value from [`Self::ratified`] or [`RenderBudgetsOverride::resolve`] has
/// already passed [`Self::validate`], which is the single refusal point; a
/// caller that assembles a budget field by field must validate it before use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderBudgets {
    /// CPU decoded-image cache ceiling, thumbnails and previews counted
    /// together.
    pub cpu_decoded_image_cache_bytes: u64,
    /// Image/graph GPU residency ceiling; tiles and previews share it.
    pub gpu_image_graph_cache_bytes: u64,
    /// Upload staging ceiling.
    pub upload_staging_bytes: u64,
    /// Combined in-flight decoded/upload payload ceiling.
    pub in_flight_payload_bytes: u64,
    /// Per-frame upload cap at 60 Hz.
    pub upload_cap_bytes_60_hz: u64,
    /// Per-frame upload cap at 120 Hz.
    pub upload_cap_bytes_120_hz: u64,
    /// Concurrent thumbnail and preview decode jobs per process.
    pub thumbnail_preview_decode_jobs: usize,
    /// Concurrent full-image and tile decode jobs per process.
    pub full_tile_decode_jobs: usize,
    /// Queued visible/near-visible image requests; a new viewport supersedes
    /// obsolete entries rather than growing the queue.
    pub queued_visible_image_requests: usize,
    /// Active graph-layout jobs per graph.
    pub graph_layout_jobs_per_graph: usize,
    /// Latest pending graph-layout revisions per graph.
    pub graph_layout_pending_revisions: usize,
    /// Background search/index jobs per window.
    pub background_search_index_jobs_per_window: usize,
}

impl RenderBudgets {
    /// Declared provisional default for [`Self::cpu_decoded_image_cache_bytes`].
    pub const CPU_DECODED_IMAGE_CACHE_BYTES: u64 = 256 * MIB;
    /// Declared provisional default for [`Self::gpu_image_graph_cache_bytes`].
    pub const GPU_IMAGE_GRAPH_CACHE_BYTES: u64 = 512 * MIB;
    /// Declared provisional default for [`Self::upload_staging_bytes`].
    pub const UPLOAD_STAGING_BYTES: u64 = 64 * MIB;
    /// Declared provisional default for [`Self::in_flight_payload_bytes`].
    pub const IN_FLIGHT_PAYLOAD_BYTES: u64 = 64 * MIB;
    /// Declared provisional default for [`Self::upload_cap_bytes_60_hz`].
    pub const UPLOAD_CAP_BYTES_60_HZ: u64 = 4 * MIB;
    /// Declared provisional default for [`Self::upload_cap_bytes_120_hz`].
    pub const UPLOAD_CAP_BYTES_120_HZ: u64 = 2 * MIB;
    /// Declared provisional default for [`Self::thumbnail_preview_decode_jobs`].
    pub const THUMBNAIL_PREVIEW_DECODE_JOBS: usize = 2;
    /// Declared provisional default for [`Self::full_tile_decode_jobs`].
    pub const FULL_TILE_DECODE_JOBS: usize = 1;
    /// Declared provisional default for [`Self::queued_visible_image_requests`].
    pub const QUEUED_VISIBLE_IMAGE_REQUESTS: usize = 16;
    /// Declared provisional default for [`Self::graph_layout_jobs_per_graph`].
    pub const GRAPH_LAYOUT_JOBS_PER_GRAPH: usize = 1;
    /// Declared provisional default for [`Self::graph_layout_pending_revisions`].
    pub const GRAPH_LAYOUT_PENDING_REVISIONS: usize = 1;
    /// Declared provisional default for
    /// [`Self::background_search_index_jobs_per_window`].
    pub const BACKGROUND_SEARCH_INDEX_JOBS_PER_WINDOW: usize = 2;

    /// The owner-ratified declared provisional defaults.
    ///
    /// No consumer may hardcode these numbers at a use site; resolve an
    /// override instead.
    #[must_use]
    pub const fn ratified() -> Self {
        Self {
            cpu_decoded_image_cache_bytes: Self::CPU_DECODED_IMAGE_CACHE_BYTES,
            gpu_image_graph_cache_bytes: Self::GPU_IMAGE_GRAPH_CACHE_BYTES,
            upload_staging_bytes: Self::UPLOAD_STAGING_BYTES,
            in_flight_payload_bytes: Self::IN_FLIGHT_PAYLOAD_BYTES,
            upload_cap_bytes_60_hz: Self::UPLOAD_CAP_BYTES_60_HZ,
            upload_cap_bytes_120_hz: Self::UPLOAD_CAP_BYTES_120_HZ,
            thumbnail_preview_decode_jobs: Self::THUMBNAIL_PREVIEW_DECODE_JOBS,
            full_tile_decode_jobs: Self::FULL_TILE_DECODE_JOBS,
            queued_visible_image_requests: Self::QUEUED_VISIBLE_IMAGE_REQUESTS,
            graph_layout_jobs_per_graph: Self::GRAPH_LAYOUT_JOBS_PER_GRAPH,
            graph_layout_pending_revisions: Self::GRAPH_LAYOUT_PENDING_REVISIONS,
            background_search_index_jobs_per_window: Self::BACKGROUND_SEARCH_INDEX_JOBS_PER_WINDOW,
        }
    }

    /// The per-frame upload cap declared for `rate`.
    ///
    /// A caller passes the measured refresh class it renders at; the two caps
    /// are independent declared values, so neither stands in for the other.
    #[must_use]
    pub const fn upload_cap_bytes(&self, rate: FrameRate) -> u64 {
        match rate {
            FrameRate::Hz60 => self.upload_cap_bytes_60_hz,
            FrameRate::Hz120 => self.upload_cap_bytes_120_hz,
        }
    }

    /// Refuse a budget that cannot bound the work it claims to bound.
    ///
    /// Zero concurrency, zero byte ceilings, a byte ceiling that cannot hold
    /// one indivisible upload unit and an upload cap below that unit are
    /// errors, never clamped values.
    pub fn validate(&self) -> Result<(), RenderBudgetError> {
        if self.cpu_decoded_image_cache_bytes == 0 {
            return Err(RenderBudgetError::ZeroCpuDecodedImageCacheBytes);
        }
        if self.gpu_image_graph_cache_bytes == 0 {
            return Err(RenderBudgetError::ZeroGpuImageGraphCacheBytes);
        }
        if self.upload_staging_bytes == 0 {
            return Err(RenderBudgetError::ZeroUploadStagingBytes);
        }
        if self.in_flight_payload_bytes == 0 {
            return Err(RenderBudgetError::ZeroInFlightPayloadBytes);
        }
        if self.thumbnail_preview_decode_jobs == 0 {
            return Err(RenderBudgetError::ZeroThumbnailPreviewDecodeJobs);
        }
        if self.full_tile_decode_jobs == 0 {
            return Err(RenderBudgetError::ZeroFullTileDecodeJobs);
        }
        if self.queued_visible_image_requests == 0 {
            return Err(RenderBudgetError::ZeroQueuedVisibleImageRequests);
        }
        if self.graph_layout_jobs_per_graph == 0 {
            return Err(RenderBudgetError::ZeroGraphLayoutJobsPerGraph);
        }
        if self.graph_layout_pending_revisions == 0 {
            return Err(RenderBudgetError::ZeroGraphLayoutPendingRevisions);
        }
        if self.background_search_index_jobs_per_window == 0 {
            return Err(RenderBudgetError::ZeroBackgroundSearchIndexJobsPerWindow);
        }
        if self.upload_staging_bytes < MIN_UPLOAD_UNIT_BYTES {
            return Err(RenderBudgetError::UploadStagingBelowMinimumUnit {
                staging_bytes: self.upload_staging_bytes,
            });
        }
        if self.in_flight_payload_bytes < MIN_UPLOAD_UNIT_BYTES {
            return Err(RenderBudgetError::InFlightPayloadBelowMinimumUnit {
                payload_bytes: self.in_flight_payload_bytes,
            });
        }
        if self.gpu_image_graph_cache_bytes < MIN_UPLOAD_UNIT_BYTES {
            return Err(RenderBudgetError::GpuImageGraphCacheBelowMinimumUnit {
                cache_bytes: self.gpu_image_graph_cache_bytes,
            });
        }
        let caps = [
            (FrameRate::Hz60, self.upload_cap_bytes_60_hz),
            (FrameRate::Hz120, self.upload_cap_bytes_120_hz),
        ];
        for (rate, cap_bytes) in caps {
            if cap_bytes < MIN_UPLOAD_UNIT_BYTES {
                return Err(RenderBudgetError::UploadCapBelowMinimumUnit {
                    rate_hz: rate.hz(),
                    cap_bytes,
                });
            }
        }
        Ok(())
    }
}

impl Default for RenderBudgets {
    fn default() -> Self {
        Self::ratified()
    }
}

/// Field-wise override of [`RenderBudgets`]; `None` keeps the base value.
///
/// A caller or a future config source sets only the fields it changes and
/// resolves them over the ratified defaults or over another explicit base.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderBudgetsOverride {
    pub cpu_decoded_image_cache_bytes: Option<u64>,
    pub gpu_image_graph_cache_bytes: Option<u64>,
    pub upload_staging_bytes: Option<u64>,
    pub in_flight_payload_bytes: Option<u64>,
    pub upload_cap_bytes_60_hz: Option<u64>,
    pub upload_cap_bytes_120_hz: Option<u64>,
    pub thumbnail_preview_decode_jobs: Option<usize>,
    pub full_tile_decode_jobs: Option<usize>,
    pub queued_visible_image_requests: Option<usize>,
    pub graph_layout_jobs_per_graph: Option<usize>,
    pub graph_layout_pending_revisions: Option<usize>,
    pub background_search_index_jobs_per_window: Option<usize>,
}

impl RenderBudgetsOverride {
    /// Resolve this override over `base` without mutating either side.
    ///
    /// Pure: the result depends only on these two values. An invalid resolved
    /// budget is refused with [`RenderBudgetError`], never clamped.
    pub fn resolve(self, base: RenderBudgets) -> Result<RenderBudgets, RenderBudgetError> {
        let resolved = RenderBudgets {
            cpu_decoded_image_cache_bytes: self
                .cpu_decoded_image_cache_bytes
                .unwrap_or(base.cpu_decoded_image_cache_bytes),
            gpu_image_graph_cache_bytes: self
                .gpu_image_graph_cache_bytes
                .unwrap_or(base.gpu_image_graph_cache_bytes),
            upload_staging_bytes: self
                .upload_staging_bytes
                .unwrap_or(base.upload_staging_bytes),
            in_flight_payload_bytes: self
                .in_flight_payload_bytes
                .unwrap_or(base.in_flight_payload_bytes),
            upload_cap_bytes_60_hz: self
                .upload_cap_bytes_60_hz
                .unwrap_or(base.upload_cap_bytes_60_hz),
            upload_cap_bytes_120_hz: self
                .upload_cap_bytes_120_hz
                .unwrap_or(base.upload_cap_bytes_120_hz),
            thumbnail_preview_decode_jobs: self
                .thumbnail_preview_decode_jobs
                .unwrap_or(base.thumbnail_preview_decode_jobs),
            full_tile_decode_jobs: self
                .full_tile_decode_jobs
                .unwrap_or(base.full_tile_decode_jobs),
            queued_visible_image_requests: self
                .queued_visible_image_requests
                .unwrap_or(base.queued_visible_image_requests),
            graph_layout_jobs_per_graph: self
                .graph_layout_jobs_per_graph
                .unwrap_or(base.graph_layout_jobs_per_graph),
            graph_layout_pending_revisions: self
                .graph_layout_pending_revisions
                .unwrap_or(base.graph_layout_pending_revisions),
            background_search_index_jobs_per_window: self
                .background_search_index_jobs_per_window
                .unwrap_or(base.background_search_index_jobs_per_window),
        };
        resolved.validate()?;
        Ok(resolved)
    }

    /// Resolve this override over [`RenderBudgets::ratified`].
    pub fn resolve_default(self) -> Result<RenderBudgets, RenderBudgetError> {
        self.resolve(RenderBudgets::ratified())
    }
}

/// Closed refusal for an invalid resolved budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderBudgetError {
    ZeroCpuDecodedImageCacheBytes,
    ZeroGpuImageGraphCacheBytes,
    ZeroUploadStagingBytes,
    ZeroInFlightPayloadBytes,
    ZeroThumbnailPreviewDecodeJobs,
    ZeroFullTileDecodeJobs,
    ZeroQueuedVisibleImageRequests,
    ZeroGraphLayoutJobsPerGraph,
    ZeroGraphLayoutPendingRevisions,
    ZeroBackgroundSearchIndexJobsPerWindow,
    UploadStagingBelowMinimumUnit { staging_bytes: u64 },
    InFlightPayloadBelowMinimumUnit { payload_bytes: u64 },
    GpuImageGraphCacheBelowMinimumUnit { cache_bytes: u64 },
    UploadCapBelowMinimumUnit { rate_hz: u16, cap_bytes: u64 },
}

impl std::fmt::Display for RenderBudgetError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ZeroCpuDecodedImageCacheBytes => {
                formatter.write_str("the CPU decoded-image cache budget must be greater than zero")
            }
            Self::ZeroGpuImageGraphCacheBytes => {
                formatter.write_str("the image/graph GPU cache budget must be greater than zero")
            }
            Self::ZeroUploadStagingBytes => {
                formatter.write_str("the upload staging budget must be greater than zero")
            }
            Self::ZeroInFlightPayloadBytes => formatter.write_str(
                "the combined in-flight decoded/upload payload budget must be greater than zero",
            ),
            Self::ZeroThumbnailPreviewDecodeJobs => formatter
                .write_str("thumbnail/preview decode concurrency must be greater than zero"),
            Self::ZeroFullTileDecodeJobs => {
                formatter.write_str("full/tile decode concurrency must be greater than zero")
            }
            Self::ZeroQueuedVisibleImageRequests => formatter
                .write_str("the visible image request queue bound must be greater than zero"),
            Self::ZeroGraphLayoutJobsPerGraph => {
                formatter.write_str("graph-layout concurrency per graph must be greater than zero")
            }
            Self::ZeroGraphLayoutPendingRevisions => formatter
                .write_str("pending graph-layout revisions per graph must be greater than zero"),
            Self::ZeroBackgroundSearchIndexJobsPerWindow => formatter.write_str(
                "background search/index concurrency per window must be greater than zero",
            ),
            Self::UploadStagingBelowMinimumUnit { .. } => write!(
                formatter,
                "the upload staging budget cannot hold even one indivisible upload unit of {MIN_UPLOAD_UNIT_BYTES} bytes"
            ),
            Self::InFlightPayloadBelowMinimumUnit { .. } => write!(
                formatter,
                "the combined in-flight decoded/upload payload budget cannot hold even one indivisible upload unit of {MIN_UPLOAD_UNIT_BYTES} bytes"
            ),
            Self::GpuImageGraphCacheBelowMinimumUnit { .. } => write!(
                formatter,
                "the image/graph GPU cache budget cannot hold even one indivisible upload unit of {MIN_UPLOAD_UNIT_BYTES} bytes"
            ),
            Self::UploadCapBelowMinimumUnit { rate_hz, cap_bytes } => write!(
                formatter,
                "the {rate_hz} Hz per-frame upload cap of {cap_bytes} bytes is below one indivisible upload unit of {MIN_UPLOAD_UNIT_BYTES} bytes"
            ),
        }
    }
}

impl std::error::Error for RenderBudgetError {}

#[cfg(test)]
mod tests {
    use super::{
        FrameRate, MIB, MIN_UPLOAD_UNIT_BYTES, RenderBudgetError, RenderBudgets,
        RenderBudgetsOverride,
    };

    /// One override setter plus the budget it must resolve to.
    type BudgetCase = (&'static str, fn(&mut RenderBudgetsOverride), RenderBudgets);

    /// One override setter plus the refusal it must produce.
    type RefusalCase = (
        &'static str,
        fn(&mut RenderBudgetsOverride),
        RenderBudgetError,
    );

    #[test]
    fn ratified_defaults_resolve_to_the_owner_ratified_numbers() {
        let budgets = RenderBudgets::default();
        assert_eq!(budgets, RenderBudgets::ratified());
        assert_eq!(budgets.cpu_decoded_image_cache_bytes, 268_435_456);
        assert_eq!(budgets.gpu_image_graph_cache_bytes, 536_870_912);
        assert_eq!(budgets.upload_staging_bytes, 67_108_864);
        assert_eq!(budgets.in_flight_payload_bytes, 67_108_864);
        assert_eq!(budgets.upload_cap_bytes_60_hz, 4_194_304);
        assert_eq!(budgets.upload_cap_bytes_120_hz, 2_097_152);
        assert_eq!(budgets.thumbnail_preview_decode_jobs, 2);
        assert_eq!(budgets.full_tile_decode_jobs, 1);
        assert_eq!(budgets.queued_visible_image_requests, 16);
        assert_eq!(budgets.graph_layout_jobs_per_graph, 1);
        assert_eq!(budgets.graph_layout_pending_revisions, 1);
        assert_eq!(budgets.background_search_index_jobs_per_window, 2);
        assert_eq!(
            RenderBudgetsOverride::default()
                .resolve_default()
                .expect("the ratified defaults are valid"),
            budgets
        );
        assert!(budgets.validate().is_ok());
    }

    #[test]
    fn each_byte_override_is_honoured_and_leaves_every_other_field_at_the_base() {
        let base = RenderBudgets::ratified();
        let cases: [BudgetCase; 6] = [
            (
                "cpu_decoded_image_cache_bytes",
                |overrides| overrides.cpu_decoded_image_cache_bytes = Some(MIB),
                RenderBudgets {
                    cpu_decoded_image_cache_bytes: MIB,
                    ..base
                },
            ),
            (
                "gpu_image_graph_cache_bytes",
                |overrides| overrides.gpu_image_graph_cache_bytes = Some(MIB),
                RenderBudgets {
                    gpu_image_graph_cache_bytes: MIB,
                    ..base
                },
            ),
            (
                "upload_staging_bytes",
                |overrides| overrides.upload_staging_bytes = Some(MIB),
                RenderBudgets {
                    upload_staging_bytes: MIB,
                    ..base
                },
            ),
            (
                "in_flight_payload_bytes",
                |overrides| overrides.in_flight_payload_bytes = Some(MIB),
                RenderBudgets {
                    in_flight_payload_bytes: MIB,
                    ..base
                },
            ),
            (
                "upload_cap_bytes_60_hz",
                |overrides| overrides.upload_cap_bytes_60_hz = Some(MIB),
                RenderBudgets {
                    upload_cap_bytes_60_hz: MIB,
                    ..base
                },
            ),
            (
                "upload_cap_bytes_120_hz",
                |overrides| overrides.upload_cap_bytes_120_hz = Some(MIB),
                RenderBudgets {
                    upload_cap_bytes_120_hz: MIB,
                    ..base
                },
            ),
        ];
        for (field, set, expected) in cases {
            let mut overrides = RenderBudgetsOverride::default();
            set(&mut overrides);
            let resolved = overrides
                .resolve(base)
                .expect("a single valid override resolves");
            assert_eq!(resolved, expected, "{field}");
            assert_ne!(resolved, base, "{field}");
        }
    }

    #[test]
    fn each_count_override_is_honoured_and_leaves_every_other_field_at_the_base() {
        let base = RenderBudgets::ratified();
        let cases: [BudgetCase; 6] = [
            (
                "thumbnail_preview_decode_jobs",
                |overrides| overrides.thumbnail_preview_decode_jobs = Some(7),
                RenderBudgets {
                    thumbnail_preview_decode_jobs: 7,
                    ..base
                },
            ),
            (
                "full_tile_decode_jobs",
                |overrides| overrides.full_tile_decode_jobs = Some(7),
                RenderBudgets {
                    full_tile_decode_jobs: 7,
                    ..base
                },
            ),
            (
                "queued_visible_image_requests",
                |overrides| overrides.queued_visible_image_requests = Some(7),
                RenderBudgets {
                    queued_visible_image_requests: 7,
                    ..base
                },
            ),
            (
                "graph_layout_jobs_per_graph",
                |overrides| overrides.graph_layout_jobs_per_graph = Some(7),
                RenderBudgets {
                    graph_layout_jobs_per_graph: 7,
                    ..base
                },
            ),
            (
                "graph_layout_pending_revisions",
                |overrides| overrides.graph_layout_pending_revisions = Some(7),
                RenderBudgets {
                    graph_layout_pending_revisions: 7,
                    ..base
                },
            ),
            (
                "background_search_index_jobs_per_window",
                |overrides| overrides.background_search_index_jobs_per_window = Some(7),
                RenderBudgets {
                    background_search_index_jobs_per_window: 7,
                    ..base
                },
            ),
        ];
        for (field, set, expected) in cases {
            let mut overrides = RenderBudgetsOverride::default();
            set(&mut overrides);
            let resolved = overrides
                .resolve(base)
                .expect("a single valid override resolves");
            assert_eq!(resolved, expected, "{field}");
            assert_ne!(resolved, base, "{field}");
        }
    }

    #[test]
    fn zero_and_too_small_values_are_refused_instead_of_clamped() {
        let base = RenderBudgets::ratified();
        let cases: [RefusalCase; 14] = [
            (
                "zero cpu cache",
                |overrides| overrides.cpu_decoded_image_cache_bytes = Some(0),
                RenderBudgetError::ZeroCpuDecodedImageCacheBytes,
            ),
            (
                "zero gpu cache",
                |overrides| overrides.gpu_image_graph_cache_bytes = Some(0),
                RenderBudgetError::ZeroGpuImageGraphCacheBytes,
            ),
            (
                "zero staging",
                |overrides| overrides.upload_staging_bytes = Some(0),
                RenderBudgetError::ZeroUploadStagingBytes,
            ),
            (
                "zero in-flight payload",
                |overrides| overrides.in_flight_payload_bytes = Some(0),
                RenderBudgetError::ZeroInFlightPayloadBytes,
            ),
            (
                "zero thumbnail/preview jobs",
                |overrides| overrides.thumbnail_preview_decode_jobs = Some(0),
                RenderBudgetError::ZeroThumbnailPreviewDecodeJobs,
            ),
            (
                "zero full/tile jobs",
                |overrides| overrides.full_tile_decode_jobs = Some(0),
                RenderBudgetError::ZeroFullTileDecodeJobs,
            ),
            (
                "zero queued requests",
                |overrides| overrides.queued_visible_image_requests = Some(0),
                RenderBudgetError::ZeroQueuedVisibleImageRequests,
            ),
            (
                "zero graph-layout jobs",
                |overrides| overrides.graph_layout_jobs_per_graph = Some(0),
                RenderBudgetError::ZeroGraphLayoutJobsPerGraph,
            ),
            (
                "zero pending revisions",
                |overrides| overrides.graph_layout_pending_revisions = Some(0),
                RenderBudgetError::ZeroGraphLayoutPendingRevisions,
            ),
            (
                "zero background jobs",
                |overrides| overrides.background_search_index_jobs_per_window = Some(0),
                RenderBudgetError::ZeroBackgroundSearchIndexJobsPerWindow,
            ),
            (
                "zero 60 Hz cap",
                |overrides| overrides.upload_cap_bytes_60_hz = Some(0),
                RenderBudgetError::UploadCapBelowMinimumUnit {
                    rate_hz: 60,
                    cap_bytes: 0,
                },
            ),
            (
                "sub-unit 60 Hz cap",
                |overrides| overrides.upload_cap_bytes_60_hz = Some(MIN_UPLOAD_UNIT_BYTES - 1),
                RenderBudgetError::UploadCapBelowMinimumUnit {
                    rate_hz: 60,
                    cap_bytes: MIN_UPLOAD_UNIT_BYTES - 1,
                },
            ),
            (
                "zero 120 Hz cap",
                |overrides| overrides.upload_cap_bytes_120_hz = Some(0),
                RenderBudgetError::UploadCapBelowMinimumUnit {
                    rate_hz: 120,
                    cap_bytes: 0,
                },
            ),
            (
                "sub-unit 120 Hz cap",
                |overrides| overrides.upload_cap_bytes_120_hz = Some(MIN_UPLOAD_UNIT_BYTES - 1),
                RenderBudgetError::UploadCapBelowMinimumUnit {
                    rate_hz: 120,
                    cap_bytes: MIN_UPLOAD_UNIT_BYTES - 1,
                },
            ),
        ];
        for (case, set, expected) in cases {
            let mut overrides = RenderBudgetsOverride::default();
            set(&mut overrides);
            assert_eq!(overrides.resolve(base), Err(expected), "{case}");
        }
        // The refusal is closed and legible: it names the boundary, not a clamp.
        assert_eq!(
            RenderBudgetError::UploadCapBelowMinimumUnit {
                rate_hz: 120,
                cap_bytes: MIN_UPLOAD_UNIT_BYTES - 1,
            }
            .to_string(),
            "the 120 Hz per-frame upload cap of 262143 bytes is below one indivisible upload unit of 262144 bytes"
        );
        // Exactly one indivisible unit is the smallest accepted cap.
        let smallest = RenderBudgetsOverride {
            upload_cap_bytes_60_hz: Some(MIN_UPLOAD_UNIT_BYTES),
            upload_cap_bytes_120_hz: Some(MIN_UPLOAD_UNIT_BYTES),
            ..RenderBudgetsOverride::default()
        }
        .resolve(base)
        .expect("one indivisible unit is a representable cap");
        assert_eq!(
            smallest.upload_cap_bytes(FrameRate::Hz60),
            MIN_UPLOAD_UNIT_BYTES
        );
        assert_eq!(
            smallest.upload_cap_bytes(FrameRate::Hz120),
            MIN_UPLOAD_UNIT_BYTES
        );
    }

    #[test]
    fn the_60_hz_and_120_hz_caps_are_independent_and_never_substitute_for_each_other() {
        let base = RenderBudgets::ratified();
        assert_eq!(FrameRate::Hz60.hz(), 60);
        assert_eq!(FrameRate::Hz120.hz(), 120);
        assert_eq!(base.upload_cap_bytes(FrameRate::Hz60), 4 * MIB);
        assert_eq!(base.upload_cap_bytes(FrameRate::Hz120), 2 * MIB);
        assert_ne!(
            base.upload_cap_bytes(FrameRate::Hz60),
            base.upload_cap_bytes(FrameRate::Hz120)
        );

        let raised_60 = RenderBudgetsOverride {
            upload_cap_bytes_60_hz: Some(8 * MIB),
            ..RenderBudgetsOverride::default()
        }
        .resolve(base)
        .expect("8 MiB at 60 Hz is valid");
        assert_eq!(raised_60.upload_cap_bytes(FrameRate::Hz60), 8 * MIB);
        assert_eq!(raised_60.upload_cap_bytes(FrameRate::Hz120), 2 * MIB);

        let raised_120 = RenderBudgetsOverride {
            upload_cap_bytes_120_hz: Some(MIB),
            ..RenderBudgetsOverride::default()
        }
        .resolve(base)
        .expect("1 MiB at 120 Hz is valid");
        assert_eq!(raised_120.upload_cap_bytes(FrameRate::Hz120), MIB);
        assert_eq!(raised_120.upload_cap_bytes(FrameRate::Hz60), 4 * MIB);

        // A cap larger than the other rate's staging default is still a
        // decision, not an implicit fallback to the smaller declared cap.
        let unsplit_120 = RenderBudgetsOverride {
            upload_cap_bytes_120_hz: Some(6 * MIB),
            ..RenderBudgetsOverride::default()
        }
        .resolve(base)
        .expect("6 MiB at 120 Hz is valid");
        assert_eq!(unsplit_120.upload_cap_bytes(FrameRate::Hz120), 6 * MIB);
    }

    #[test]
    fn resolution_layers_over_an_explicit_base_purely() {
        let base = RenderBudgets {
            cpu_decoded_image_cache_bytes: 512 * MIB,
            ..RenderBudgets::ratified()
        };
        let overrides = RenderBudgetsOverride {
            full_tile_decode_jobs: Some(2),
            ..RenderBudgetsOverride::default()
        };
        let resolved = overrides
            .resolve(base)
            .expect("valid override over a custom base");
        assert_eq!(resolved.cpu_decoded_image_cache_bytes, 512 * MIB);
        assert_eq!(resolved.full_tile_decode_jobs, 2);
        // Purity: the same inputs always produce the same budget, and the
        // base is not consumed or mutated by resolution.
        assert_eq!(overrides.resolve(base), Ok(resolved));
        assert_eq!(
            base,
            RenderBudgets {
                cpu_decoded_image_cache_bytes: 512 * MIB,
                ..RenderBudgets::ratified()
            }
        );
        assert_eq!(
            base.full_tile_decode_jobs,
            RenderBudgets::FULL_TILE_DECODE_JOBS
        );
    }

    #[test]
    fn resolved_bounds_size_existing_bounded_limits_without_changing_them() {
        let budgets = RenderBudgets::default();
        // The crate already builds bounded constructs from a plain `usize`
        // bound (a bounded `sync_channel` in the R0 probe, saturating trace
        // admission) and byte ceilings from `u64`. A resolved budget supplies
        // exactly those shapes, so no consumer has to restate the number.
        let (sender, _receiver) =
            std::sync::mpsc::sync_channel::<()>(budgets.queued_visible_image_requests);
        for admitted in 0..budgets.queued_visible_image_requests {
            assert!(sender.try_send(()).is_ok(), "admitted {admitted}");
        }
        assert!(matches!(
            sender.try_send(()),
            Err(std::sync::mpsc::TrySendError::Full(()))
        ));
        assert_eq!(
            usize::try_from(budgets.upload_cap_bytes(FrameRate::Hz60))
                .expect("a cap fits one usize on this target"),
            4 * MIB as usize
        );
        // These unrelated declared limits keep their own values: the render
        // budget type does not take over a trace quota or a dock geometry.
        assert_eq!(crate::trace::MAX_DOCK_LINES, 4096);
        assert_eq!(crate::trace::MAX_RAIL_LINES, 8192);
    }

    #[test]
    fn byte_ceilings_that_cannot_hold_one_upload_unit_are_refused() {
        let base = RenderBudgets::ratified();
        let refusals: [RefusalCase; 3] = [
            (
                "upload staging one byte below one unit",
                |overrides| overrides.upload_staging_bytes = Some(MIN_UPLOAD_UNIT_BYTES - 1),
                RenderBudgetError::UploadStagingBelowMinimumUnit {
                    staging_bytes: MIN_UPLOAD_UNIT_BYTES - 1,
                },
            ),
            (
                "in-flight payload one byte below one unit",
                |overrides| overrides.in_flight_payload_bytes = Some(MIN_UPLOAD_UNIT_BYTES - 1),
                RenderBudgetError::InFlightPayloadBelowMinimumUnit {
                    payload_bytes: MIN_UPLOAD_UNIT_BYTES - 1,
                },
            ),
            (
                "GPU cache one byte below one unit",
                |overrides| overrides.gpu_image_graph_cache_bytes = Some(MIN_UPLOAD_UNIT_BYTES - 1),
                RenderBudgetError::GpuImageGraphCacheBelowMinimumUnit {
                    cache_bytes: MIN_UPLOAD_UNIT_BYTES - 1,
                },
            ),
        ];
        for (case, set, expected) in refusals {
            let mut overrides = RenderBudgetsOverride::default();
            set(&mut overrides);
            assert_eq!(overrides.resolve(base), Err(expected), "{case}");
        }

        // The reviewer's exact combination: two ceilings that cannot hold one
        // unit are refused, and the first violated ceiling is named.
        let unworkable = RenderBudgetsOverride {
            upload_staging_bytes: Some(1),
            in_flight_payload_bytes: Some(1),
            ..RenderBudgetsOverride::default()
        };
        assert_eq!(
            unworkable.resolve_default(),
            Err(RenderBudgetError::UploadStagingBelowMinimumUnit { staging_bytes: 1 })
        );

        // Exactly one indivisible unit is the smallest accepted ceiling for
        // each capacity the minimum unit has to fit into.
        let smallest = RenderBudgetsOverride {
            gpu_image_graph_cache_bytes: Some(MIN_UPLOAD_UNIT_BYTES),
            upload_staging_bytes: Some(MIN_UPLOAD_UNIT_BYTES),
            in_flight_payload_bytes: Some(MIN_UPLOAD_UNIT_BYTES),
            ..RenderBudgetsOverride::default()
        }
        .resolve_default()
        .expect("one indivisible unit per capacity is representable");
        assert_eq!(smallest.upload_staging_bytes, MIN_UPLOAD_UNIT_BYTES);
        assert_eq!(smallest.in_flight_payload_bytes, MIN_UPLOAD_UNIT_BYTES);
        assert_eq!(smallest.gpu_image_graph_cache_bytes, MIN_UPLOAD_UNIT_BYTES);

        // The refusal is legible and names the declared minimum unit.
        assert_eq!(
            RenderBudgetError::InFlightPayloadBelowMinimumUnit { payload_bytes: 1 }.to_string(),
            "the combined in-flight decoded/upload payload budget cannot hold even one indivisible upload unit of 262144 bytes"
        );
    }
}
