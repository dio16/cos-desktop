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
    /// One transfer moves the frame and it fits the budget.
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
pub fn rgba8_frame_bytes(edge: u32) -> u64 {
    u64::from(edge) * u64::from(edge) * 4
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
