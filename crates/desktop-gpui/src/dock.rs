//! Fixed-stride horizontal virtualization in GPUI's negative-offset coordinates.

pub(super) const ROW_WIDTH: f32 = 220.0;
pub(super) const GAP: f32 = 12.0;
pub(super) const STRIDE: f32 = ROW_WIDTH + GAP;
const OVERSCAN: usize = 2;

pub(super) fn extent(count: usize) -> f32 {
    if count == 0 {
        0.0
    } else {
        count as f32 * STRIDE - GAP
    }
}

pub(super) fn clamp_offset(count: usize, viewport: f32, offset: f32) -> f32 {
    offset.clamp(-(extent(count) - viewport.max(0.0)).max(0.0), 0.0)
}

pub(super) struct DockWindow {
    pub first: usize,
    pub end: usize,
    pub visible: usize,
    pub scroll: f32,
    pub left: f32,
    pub right: f32,
}

impl DockWindow {
    pub fn new(count: usize, viewport: f32, offset: f32) -> Self {
        let scroll = -clamp_offset(count, viewport, offset);
        let visible = ((scroll / STRIDE).floor() as usize).min(count.saturating_sub(1));
        let first = visible.saturating_sub(OVERSCAN);
        let end = (((scroll + viewport.max(0.0)) / STRIDE).ceil() as usize)
            .saturating_add(OVERSCAN)
            .max(first)
            .min(count);
        Self {
            first,
            end,
            visible,
            scroll,
            left: first as f32 * STRIDE,
            right: extent(count.saturating_sub(end)),
        }
    }
}

/// Reveal only the part outside the viewport; do not realign an already visible row.
pub(super) fn reveal(count: usize, index: usize, viewport: f32, offset: f32) -> f32 {
    let scroll = -clamp_offset(count, viewport, offset);
    let left = index as f32 * STRIDE;
    let right = left + ROW_WIDTH;
    let target = if left < scroll {
        left
    } else if right > scroll + viewport {
        right - viewport
    } else {
        scroll
    };
    clamp_offset(count, viewport, -target)
}

/// Why Compare is unavailable for this candidate count, if it is (ADR-0010 D-2).
///
/// Compare stays visible and states this reason; it is never silently hidden.
pub(super) fn compare_unavailable_reason(candidate_count: usize) -> Option<&'static str> {
    (candidate_count < 2)
        .then_some("two candidates are required in the current authorized observation")
}

/// Nearest valid row after a focused row disappears (ADR-0010 D-12).
///
/// The cursor keeps its position (clamped to the new length) without moving the
/// selection; an empty dock yields no active row.
pub(super) fn nearest_index(previous: Option<usize>, len: usize) -> Option<usize> {
    if len == 0 {
        None
    } else {
        Some(previous.unwrap_or(0).min(len - 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_reveal_uses_negative_offsets_and_keeps_the_row_visible() {
        let offset = reveal(10_000, 24, 800.0, 0.0);
        assert!(offset < 0.0);
        let window = DockWindow::new(10_000, 800.0, offset);
        assert!(window.first <= 24 && 24 < window.end);
        let screen_left = 24.0 * STRIDE + offset;
        assert!(screen_left >= 0.0 && screen_left + ROW_WIDTH <= 800.0);
        assert_eq!(reveal(10_000, 24, 800.0, offset), offset);
        assert!(window.end - window.first <= (800.0 / STRIDE).ceil() as usize + 2 * OVERSCAN + 1);
    }

    #[test]
    fn virtual_spacers_and_row_margins_preserve_total_extent_at_every_boundary() {
        for count in [0, 1, 3, 10_000] {
            for offset in [0.0, -3400.5, -1_000_000_000.0] {
                let window = DockWindow::new(count, 800.0, offset);
                let mounted = (window.first..window.end)
                    .map(|index| ROW_WIDTH + if index + 1 < count { GAP } else { 0.0 })
                    .sum::<f32>();
                assert_eq!(window.left + mounted + window.right, extent(count));
                assert!(window.first <= window.end && window.end <= count);
                assert!(window.scroll >= 0.0 && window.scroll <= (extent(count) - 800.0).max(0.0));
            }
        }
    }

    #[test]
    fn compare_states_why_it_is_unavailable_instead_of_hiding() {
        assert!(compare_unavailable_reason(0).is_some());
        assert!(compare_unavailable_reason(1).is_some());
        assert_eq!(compare_unavailable_reason(2), None);
        assert_eq!(compare_unavailable_reason(7), None);
    }

    #[test]
    fn a_disappearing_focus_target_moves_to_the_nearest_valid_row() {
        assert_eq!(nearest_index(Some(4), 6), Some(4));
        assert_eq!(nearest_index(Some(9), 6), Some(5));
        assert_eq!(nearest_index(None, 3), Some(0));
        assert_eq!(nearest_index(Some(2), 0), None);
        assert_eq!(nearest_index(None, 0), None);
    }

    #[test]
    fn fractional_scroll_and_resize_keep_a_real_viewport_not_the_overscan_origin() {
        let window = DockWindow::new(100, 800.0, -(24.0 * STRIDE + 17.5));
        assert_eq!(window.visible, 24);
        assert_eq!(window.first, 22);
        assert_eq!(window.scroll - window.visible as f32 * STRIDE, 17.5);
        let resized = DockWindow::new(100, 1200.0, -window.scroll);
        assert_eq!(resized.visible, window.visible);
        assert!(resized.end > window.end);
        assert_eq!(clamp_offset(1, 800.0, -500.0), 0.0);
    }
}
