//! MP-IMG3: whether a decoded frame can be uploaded within the provisional budget.
//!
//! The design canon sets a provisional **upload budget of 4 MiB at 60 Hz / 2 MiB at 120 Hz**,
//! while one 2048×2048 RGBA8 frame is **16 MiB**. §19.242 recorded that the next step must
//! establish whether the pinned GPUI can upload **partially**, and must report [`UploadPlan::Unavailable`]
//! if neither a chunked path nor an in-budget preview path is demonstrated.
//!
//! ## What the pinned GPUI exposes (checked, not assumed)
//!
//! - `Window::paint_image` takes `Arc<RenderImage>` - an entire image, painted as a whole
//!   (`crates/gpui/src/window.rs`, `paint_image`).
//! - The atlas entry point is `PlatformAtlas::get_or_insert_with(key, build)`, where `build`
//!   returns the whole `(Size<DevicePixels>, Cow<[u8]>)` for a tile (`crates/gpui/src/platform.rs`).
//!   `AtlasBackend::insert` likewise takes a complete image.
//!
//! There is no public entry point that takes a rectangle, a row range, or a byte range **of one
//! frame**, so a single 16 MiB image cannot be **partially updated** through the API this
//! workspace pins.
//!
//! What that does **not** prove: the same pinned source exposes `RenderImage::new`, and
//! `paint_image` takes a draw area, so an application could build **several smaller images** and
//! upload and compose them itself. Whether that route satisfies the budget is
//! **`NOT_ESTABLISHED`** - it was not evaluated here, and this module does not conclude that
//! full-resolution display is impossible.
//!
//! This module decides **only** the single-image question. It performs no upload, measures no
//! throughput and says nothing about whether a smaller preview path could satisfy the budget.

/// The provisional upload budget from the design canon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefreshRate {
    /// 60 Hz: 4 MiB per frame.
    Hz60,
    /// 120 Hz: 2 MiB per frame.
    Hz120,
}

impl RefreshRate {
    /// Bytes the canon allows one upload to move at this refresh rate.
    pub fn budget_bytes(self) -> u64 {
        match self {
            Self::Hz60 => 4 * 1024 * 1024,
            Self::Hz120 => 2 * 1024 * 1024,
        }
    }
}

/// What the pinned GPUI can actually do with a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadCapability {
    /// An image is painted and inserted into the atlas as a whole; there is no partial entry point.
    WholeImageOnly,
}

/// The answer for one frame at one refresh rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadPlan {
    /// The frame fits the budget as one image. This is a **byte-budget** statement: whether the
    /// transfer itself succeeds is not established here.
    Whole { bytes: u64, budget: u64 },
    /// A single image of this size cannot be moved within the budget.
    ///
    /// This says nothing about application-side tiling, which is `NOT_ESTABLISHED` (see the module
    /// docs).
    Unavailable {
        frame_bytes: u64,
        budget: u64,
        /// How many budget-sized units the frame is. This is an arithmetic ratio, **not** a count
        /// of transfers that can actually be issued.
        minimum_budget_units: u64,
    },
}

impl UploadPlan {
    /// Whether the frame **fits the byte budget** as one image.
    ///
    /// This is a budget statement only: it is not evidence that the upload succeeds on the GPU,
    /// that the size is aligned as the backend requires, or that the transferred byte count
    /// matches. Those are `NOT_ESTABLISHED` until measured.
    pub fn fits_budget(&self) -> bool {
        matches!(self, Self::Whole { .. })
    }
}

/// Decide whether a frame of `frame_bytes` can be uploaded under `budget`.
///
/// With [`UploadCapability::WholeImageOnly`] the frame must fit in one transfer: there is no
/// sub-frame entry point to split it across several.
pub fn plan_upload(capability: UploadCapability, frame_bytes: u64, budget: u64) -> UploadPlan {
    match capability {
        UploadCapability::WholeImageOnly => {
            if frame_bytes <= budget {
                UploadPlan::Whole {
                    bytes: frame_bytes,
                    budget,
                }
            } else {
                // Round up, so a frame one byte over the budget reports two units rather than one.
                let minimum_budget_units = frame_bytes.div_ceil(budget.max(1));
                UploadPlan::Unavailable {
                    frame_bytes,
                    budget,
                    minimum_budget_units,
                }
            }
        }
    }
}

/// Bytes in one RGBA8 frame of the given edge length.
///
/// Saturates rather than wrapping: an edge large enough to overflow a `u64` byte count is not a
/// frame this boundary could ever admit, and a saturated figure keeps the arithmetic total.
pub fn rgba8_frame_bytes(edge: u32) -> u64 {
    let side = u64::from(edge);
    side.saturating_mul(side).saturating_mul(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    const APPROVED_EDGE: u32 = 2048;

    #[test]
    fn the_approved_frame_is_unavailable_at_both_refresh_rates() {
        let frame = rgba8_frame_bytes(APPROVED_EDGE);
        assert_eq!(frame, 16 * 1024 * 1024, "2048x2048 RGBA8 is 16 MiB");

        for rate in [RefreshRate::Hz60, RefreshRate::Hz120] {
            let plan = plan_upload(UploadCapability::WholeImageOnly, frame, rate.budget_bytes());
            assert!(
                !plan.fits_budget(),
                "{rate:?}: a whole-image-only uploader cannot move 16 MiB within the budget"
            );
            match plan {
                UploadPlan::Unavailable {
                    frame_bytes,
                    budget,
                    minimum_budget_units,
                } => {
                    assert_eq!(frame_bytes, frame);
                    assert_eq!(budget, rate.budget_bytes());
                    // 16 MiB in 4 MiB transfers is four; in 2 MiB transfers, eight.
                    let expected = match rate {
                        RefreshRate::Hz60 => 4,
                        RefreshRate::Hz120 => 8,
                    };
                    assert_eq!(minimum_budget_units, expected);
                }
                other => panic!("expected Unavailable, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_frame_that_fits_is_available_and_the_boundary_is_inclusive() {
        for rate in [RefreshRate::Hz60, RefreshRate::Hz120] {
            let budget = rate.budget_bytes();
            let exact = plan_upload(UploadCapability::WholeImageOnly, budget, budget);
            assert_eq!(
                exact,
                UploadPlan::Whole {
                    bytes: budget,
                    budget
                },
                "{rate:?}: a frame exactly at the budget fits"
            );
        }
    }

    #[test]
    fn one_byte_over_the_budget_is_unavailable() {
        let budget = RefreshRate::Hz60.budget_bytes();
        let plan = plan_upload(UploadCapability::WholeImageOnly, budget + 1, budget);
        assert!(!plan.fits_budget());
        match plan {
            UploadPlan::Unavailable {
                minimum_budget_units,
                ..
            } => assert_eq!(
                minimum_budget_units, 2,
                "rounding up reports two budget units"
            ),
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn the_preview_sized_frame_is_still_too_large_for_120hz() {
        // 1024x1024 RGBA8 is 4 MiB: it fits 60 Hz and not 120 Hz. Recorded so the boundary is not
        // mistaken for "any frame is unavailable".
        let frame = rgba8_frame_bytes(1024);
        assert_eq!(frame, 4 * 1024 * 1024);
        assert!(
            plan_upload(
                UploadCapability::WholeImageOnly,
                frame,
                RefreshRate::Hz60.budget_bytes()
            )
            .fits_budget()
        );
        assert!(
            !plan_upload(
                UploadCapability::WholeImageOnly,
                frame,
                RefreshRate::Hz120.budget_bytes()
            )
            .fits_budget()
        );
    }
}

/// A grid of tiles that together cover one frame, each of which fits the budget on its own.
///
/// This is the **arithmetic** of application-side tiling: how many sub-images of what size would
/// be needed. It is not evidence that the application can issue them within the frame time, that
/// the per-image overhead leaves the budget intact, or that GPUI accepts that many images in one
/// paint pass - those are `NOT_ESTABLISHED`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TilePlan {
    /// Edge length in pixels of a full tile.
    pub tile_edge: u32,
    /// Tiles along each axis, `ceil(frame_edge / tile_edge)`.
    pub tiles_per_axis: u32,
    /// Total tiles, `tiles_per_axis ** 2`.
    pub tile_count: u64,
    /// Bytes of one full tile.
    pub tile_bytes: u64,
    /// Bytes of the whole frame, which the tiles together still add up to.
    pub frame_bytes: u64,
    /// Whether a single full tile fits the budget (the point of tiling).
    pub tile_fits_budget: bool,
    /// How many **frame periods** the whole frame would take at one tile per period.
    ///
    /// The budget is a per-frame allowance, so this is the lower bound on how long a full
    /// resolution upload takes - it is not a duration, because no transfer was measured.
    pub frame_periods_at_one_tile: u64,
}

/// Plan a square tiling of `frame_edge` pixels so that each full tile fits `budget` bytes.
///
/// Returns `None` when there is nothing to plan: a zero-sized frame, or a budget below the size of
/// a single RGBA8 pixel (4 bytes), where no tiling can help.
///
/// The budget is a **per-frame** allowance shared by the whole process, so a plan that needs four
/// tiles of 4 MiB does **not** fit inside one 4 MiB frame period - it spreads the cost over
/// several frames. See the module documentation.
pub fn plan_tiled_upload(frame_edge: u32, budget: u64) -> Option<TilePlan> {
    if frame_edge == 0 {
        return None;
    }
    // Largest edge whose RGBA8 bytes still fit: edge^2 * 4 <= budget.
    let mut tile_edge = (budget / 4).isqrt() as u32;
    if tile_edge == 0 {
        return None;
    }
    tile_edge = tile_edge.min(frame_edge);
    let tile_bytes = u64::from(tile_edge) * u64::from(tile_edge) * 4;
    let tiles_per_axis = frame_edge.div_ceil(tile_edge);
    let frame_bytes = rgba8_frame_bytes(frame_edge);
    Some(TilePlan {
        tile_edge,
        tiles_per_axis,
        tile_count: u64::from(tiles_per_axis) * u64::from(tiles_per_axis),
        tile_bytes,
        frame_bytes,
        tile_fits_budget: tile_bytes <= budget,
        // Computed in u64 so a very large grid does not saturate at the u32 ceiling.
        frame_periods_at_one_tile: u64::from(tiles_per_axis) * u64::from(tiles_per_axis),
    })
}

#[test]
fn the_frame_period_count_matches_the_tile_count_for_large_grids() {
    // A budget of four bytes makes every pixel its own tile: the two figures must still agree, which
    // they did not when the count was computed in u32 and saturated.
    let plan = plan_tiled_upload(65536, 4).expect("one pixel per tile");
    assert_eq!(plan.tile_count, 65536u64 * 65536);
    assert_eq!(
        plan.frame_periods_at_one_tile, plan.tile_count,
        "the period count must equal the tile count at one tile per period"
    );
}

#[cfg(test)]
mod tiling_tests {
    use super::*;

    const APPROVED_EDGE: u32 = 2048;

    #[test]
    fn a_16_mib_frame_splits_into_budget_sized_tiles() {
        // 4 MiB at 60 Hz is exactly a 1024x1024 RGBA8 tile, so the frame is a 2x2 grid.
        let plan = plan_tiled_upload(APPROVED_EDGE, RefreshRate::Hz60.budget_bytes())
            .expect("a 4 MiB budget fits a tile");
        assert_eq!(plan.tile_edge, 1024);
        assert_eq!(plan.tile_bytes, 4 * 1024 * 1024);
        assert_eq!(plan.tiles_per_axis, 2);
        assert_eq!(plan.tile_count, 4);
        assert!(plan.tile_fits_budget);
        assert_eq!(plan.frame_bytes, 16 * 1024 * 1024);

        // 2 MiB at 120 Hz is a 724-pixel edge (724^2 * 4 = 2,096,704 B), giving 3x3 tiles.
        let plan = plan_tiled_upload(APPROVED_EDGE, RefreshRate::Hz120.budget_bytes())
            .expect("a 2 MiB budget fits a tile");
        assert_eq!(plan.tile_edge, 724);
        assert_eq!(plan.tile_bytes, 724 * 724 * 4);
        assert!(plan.tile_bytes <= RefreshRate::Hz120.budget_bytes());
        assert_eq!(plan.tiles_per_axis, 3);
        assert_eq!(plan.tile_count, 9);
    }

    #[test]
    fn every_tile_fits_and_the_tiles_cover_the_frame_exactly() {
        // Walk the grid the way an uploader would: each tile's own bytes must fit the budget, and
        // the tiles must cover every pixel exactly once - no gap, no overlap.
        for budget in [
            RefreshRate::Hz60.budget_bytes(),
            RefreshRate::Hz120.budget_bytes(),
            1024 * 1024,
            512 * 1024,
        ] {
            let plan = plan_tiled_upload(APPROVED_EDGE, budget).expect("a tile fits");
            assert!(plan.tile_fits_budget, "budget {budget}: a tile must fit");
            let mut covered = 0u64;
            for ty in 0..plan.tiles_per_axis {
                for tx in 0..plan.tiles_per_axis {
                    let x0 = tx * plan.tile_edge;
                    let y0 = ty * plan.tile_edge;
                    let width = plan.tile_edge.min(APPROVED_EDGE - x0);
                    let height = plan.tile_edge.min(APPROVED_EDGE - y0);
                    let bytes = u64::from(width) * u64::from(height) * 4;
                    assert!(
                        bytes <= budget,
                        "budget {budget}: tile ({tx},{ty}) is {bytes} B, over the budget"
                    );
                    covered += u64::from(width) * u64::from(height);
                }
            }
            assert_eq!(
                covered,
                u64::from(APPROVED_EDGE) * u64::from(APPROVED_EDGE),
                "budget {budget}: the tiles must cover the frame exactly"
            );
        }
    }

    #[test]
    fn a_budget_that_already_fits_the_frame_needs_one_tile() {
        let plan = plan_tiled_upload(APPROVED_EDGE, rgba8_frame_bytes(APPROVED_EDGE))
            .expect("the frame fits");
        assert_eq!(plan.tile_edge, APPROVED_EDGE);
        assert_eq!(plan.tile_count, 1, "one tile covers the whole frame");
        assert!(plan.tile_fits_budget);
    }

    #[test]
    fn a_budget_below_one_pixel_has_no_tiling() {
        assert!(
            plan_tiled_upload(APPROVED_EDGE, 3).is_none(),
            "three bytes cannot hold one RGBA8 pixel"
        );
        assert!(
            plan_tiled_upload(APPROVED_EDGE, 4).is_some(),
            "four bytes hold exactly one pixel"
        );
    }
}

#[cfg(test)]
mod bounds_tests {
    use super::*;

    #[test]
    fn a_zero_sized_frame_has_no_plan_instead_of_panicking() {
        assert!(plan_tiled_upload(0, RefreshRate::Hz60.budget_bytes()).is_none());
        assert!(plan_tiled_upload(0, 4).is_none());
    }

    #[test]
    fn a_huge_edge_saturates_instead_of_wrapping_or_panicking() {
        // u32::MAX squared is about 1.8e19, and times four that exceeds u64::MAX. The function must
        // report saturation rather than wrapping or panicking, and normal sizes must be exact.
        let bytes = rgba8_frame_bytes(u32::MAX);
        assert!(
            bytes >= u64::from(u32::MAX) * u64::from(u32::MAX),
            "a saturated figure must not be smaller than the square of the edge"
        );
        assert_eq!(rgba8_frame_bytes(2048), 16 * 1024 * 1024);
        assert_eq!(rgba8_frame_bytes(1), 4);
    }

    #[test]
    fn a_full_resolution_upload_spans_several_frame_periods() {
        // The 4 MiB budget is per frame, so the four tiles need four frame periods - the point the
        // review asked to make explicit.
        let plan = plan_tiled_upload(2048, RefreshRate::Hz60.budget_bytes()).expect("plan");
        assert_eq!(plan.tile_count, 4);
        assert_eq!(plan.frame_periods_at_one_tile, 4);
        assert_eq!(plan.frame_bytes, 16 * 1024 * 1024);

        let plan = plan_tiled_upload(2048, RefreshRate::Hz120.budget_bytes()).expect("plan");
        assert_eq!(plan.tile_count, 9);
        assert_eq!(plan.frame_periods_at_one_tile, 9);
    }
}

/// One tile's rectangle within the frame, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileSpan {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl TileSpan {
    /// Bytes this tile occupies as RGBA8.
    pub fn bytes(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height) * 4
    }
}

/// One frame period's worth of tiles, all of which together fit the budget.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadPeriod {
    pub tiles: Vec<TileSpan>,
    /// Total bytes this period would move.
    pub bytes: u64,
}

/// A distribution of a frame's tiles across frame periods.
///
/// Each period stays within the per-frame budget, so a full-resolution upload spread over these
/// periods never asks for more than the budget in any one of them. This is **arithmetic**: the
/// periods are not scheduled against a clock, no transfer is performed, and whether the
/// application can actually keep to one period's worth of work per frame is `NOT_ESTABLISHED`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadSchedule {
    pub periods: Vec<UploadPeriod>,
    pub budget: u64,
}

impl UploadSchedule {
    /// How many frame periods the schedule needs.
    pub fn period_count(&self) -> usize {
        self.periods.len()
    }

    /// Whether every period stays within the budget.
    pub fn every_period_fits(&self) -> bool {
        self.periods
            .iter()
            .all(|period| period.bytes <= self.budget)
    }
}

/// Distribute a `frame_edge`-square frame's tiles of `tile_edge` pixels across frame periods that
/// each fit `budget` bytes.
///
/// Tiles are packed greedily in row-major order: a tile joins the current period when it still
/// fits, otherwise a new period starts. Greedy packing is not claimed to minimise the period count
/// for every grid, and the tests only pin what it does do.
///
/// Returns `None` for a zero-sized frame or tile, or when a single tile does not fit the budget at
/// all - no distribution can fix that.
pub fn schedule_tiles(frame_edge: u32, tile_edge: u32, budget: u64) -> Option<UploadSchedule> {
    if frame_edge == 0 || tile_edge == 0 || budget < 4 {
        return None;
    }
    let mut periods: Vec<UploadPeriod> = Vec::new();
    for y in (0..frame_edge).step_by(tile_edge as usize) {
        for x in (0..frame_edge).step_by(tile_edge as usize) {
            let span = TileSpan {
                x,
                y,
                width: tile_edge.min(frame_edge - x),
                height: tile_edge.min(frame_edge - y),
            };
            let bytes = span.bytes();
            if bytes > budget {
                return None;
            }
            match periods.last_mut() {
                Some(period) if period.bytes + bytes <= budget => {
                    period.tiles.push(span);
                    period.bytes += bytes;
                }
                _ => periods.push(UploadPeriod {
                    tiles: vec![span],
                    bytes,
                }),
            }
        }
    }
    Some(UploadSchedule { periods, budget })
}

#[cfg(test)]
mod schedule_tests {
    use super::*;

    const APPROVED_EDGE: u32 = 2048;

    #[test]
    fn a_1024_tile_grid_needs_one_period_per_tile() {
        // Each 1024x1024 tile is exactly the 60 Hz budget, so nothing can share a period.
        let schedule = schedule_tiles(APPROVED_EDGE, 1024, RefreshRate::Hz60.budget_bytes())
            .expect("a schedule exists");
        assert_eq!(schedule.period_count(), 4);
        assert!(schedule.every_period_fits());
        for period in &schedule.periods {
            assert_eq!(
                period.tiles.len(),
                1,
                "one tile per period at exactly budget"
            );
            assert_eq!(period.bytes, RefreshRate::Hz60.budget_bytes());
        }
    }

    #[test]
    fn smaller_tiles_pack_several_per_period() {
        // 512x512 tiles are 1 MiB each, so four of them fill a 4 MiB period: 16 tiles, 4 periods.
        let budget = RefreshRate::Hz60.budget_bytes();
        let schedule = schedule_tiles(APPROVED_EDGE, 512, budget).expect("a schedule exists");
        assert_eq!(schedule.period_count(), 4);
        assert!(schedule.every_period_fits());
        for period in &schedule.periods {
            assert_eq!(period.tiles.len(), 4);
            assert_eq!(period.bytes, budget);
        }
    }

    #[test]
    fn the_schedule_covers_every_pixel_exactly_once() {
        for (tile_edge, budget) in [
            (1024u32, RefreshRate::Hz60.budget_bytes()),
            (724, RefreshRate::Hz120.budget_bytes()),
            (512, RefreshRate::Hz60.budget_bytes()),
            (300, 512 * 1024),
        ] {
            let schedule = schedule_tiles(APPROVED_EDGE, tile_edge, budget).expect("a schedule");
            let mut covered = 0u64;
            let mut seen = vec![0u64; (APPROVED_EDGE as usize) * (APPROVED_EDGE as usize) / 64 + 1];
            for period in &schedule.periods {
                assert!(
                    period.bytes <= budget,
                    "tile_edge {tile_edge}: a period exceeds the budget"
                );
                for span in &period.tiles {
                    assert!(
                        span.x + span.width <= APPROVED_EDGE,
                        "tile runs off the frame"
                    );
                    assert!(
                        span.y + span.height <= APPROVED_EDGE,
                        "tile runs off the frame"
                    );
                    for y in span.y..span.y + span.height {
                        for x in span.x..span.x + span.width {
                            let index = (y as usize) * (APPROVED_EDGE as usize) + x as usize;
                            let word = index / 64;
                            let bit = 1u64 << (index % 64);
                            assert_eq!(seen[word] & bit, 0, "pixel ({x},{y}) is covered twice");
                            seen[word] |= bit;
                        }
                    }
                    covered += u64::from(span.width) * u64::from(span.height);
                }
            }
            assert_eq!(
                covered,
                u64::from(APPROVED_EDGE) * u64::from(APPROVED_EDGE),
                "tile_edge {tile_edge}: the schedule must cover the frame"
            );
        }
    }

    #[test]
    fn an_impossible_schedule_is_refused() {
        // A tile larger than the budget cannot be placed in any period.
        assert!(schedule_tiles(APPROVED_EDGE, 1024, 1024 * 1024).is_none());
        assert!(schedule_tiles(0, 1024, RefreshRate::Hz60.budget_bytes()).is_none());
        assert!(schedule_tiles(APPROVED_EDGE, 0, RefreshRate::Hz60.budget_bytes()).is_none());
        assert!(schedule_tiles(APPROVED_EDGE, 1024, 3).is_none());
    }
}
