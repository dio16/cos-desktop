//! Fixed-stride horizontal virtualization in GPUI's negative-offset coordinates,
//! plus the deterministic presentation result of one dock event (ADR-0010 D-3/D-9).

use app_core::navigation::{Navigation, Surface};
use desktop_protocol::{CandidateRef, TaskId};
use ui_model::presentation::{
    CompareSide, Presentation, RestoreOutcome, SelectionEffect, ViewedCandidate,
};

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
    (candidate_count < 2).then_some("現在の認可済み観測情報には候補が2件必要です")
}

/// Why a Compare side-change control cannot act, keyed to the same availability
/// the click handler uses, so the visible explanation and the affordance agree.
pub(super) fn side_change_hint(a_exists: bool, b_exists: bool) -> Option<&'static str> {
    match (a_exists, b_exists) {
        (false, _) => Some("A側とB側の候補がそろうまで、A側・B側を変更する操作は使えません。"),
        (true, false) => Some("B側の候補がそろうまで、B側を変更する操作は使えません。"),
        (true, true) => None,
    }
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

/// The single Compare admission every surface entry path uses (ADR-0010 D-2).
///
/// `navigate()`, the dock/switcher and the `NextSurface` (Ctrl+Tab) action all call
/// this, so Compare can never be entered while it is unavailable and the reason is
/// identical everywhere.
pub(super) fn admit(
    target: Surface,
    candidate_count: usize,
    navigation: &mut Navigation,
) -> Result<(), String> {
    if target == Surface::Compare
        && let Some(reason) = compare_unavailable_reason(candidate_count)
    {
        return Err(format!("比較は利用できません: {reason}。"));
    }
    navigation.navigate(target);
    Ok(())
}

/// The canonical surface cycle, identical to `Navigation::next()`'s specification.
pub(super) fn next_surface(surface: Surface) -> Surface {
    match surface {
        Surface::Artifact => Surface::Compare,
        Surface::Compare => Surface::Flow,
        Surface::Flow => Surface::Artifact,
    }
}

/// The `NextSurface` action's decision path, driven by the same admission as `navigate()`.
pub(super) fn advance(navigation: &mut Navigation, candidate_count: usize) -> DockEvent {
    let target = next_surface(navigation.surface());
    match admit(target, candidate_count, navigation) {
        Ok(()) => DockEvent {
            surface: Some(target),
            notice: None,
        },
        Err(notice) => DockEvent {
            surface: None,
            notice: Some(notice),
        },
    }
}

/// The deterministic result of one dock event: which surface must be shown and what
/// reason (if any) must be stated. The shell applies this to focus and rendering; the
/// decision itself is pure so it is testable without a GPU or a window (A2).
pub(super) struct DockEvent {
    pub surface: Option<Surface>,
    pub notice: Option<String>,
}

/// A dock click (D-3/A1).
///
/// The newly viewed candidate is shown on the artifact surface unless Compare is the
/// active surface or this click completes the compare pair (then Compare is shown).
pub(super) fn select(
    presentation: &mut Presentation,
    identity: CandidateRef,
    surface: Surface,
) -> DockEvent {
    let effect = presentation.select(ViewedCandidate(identity));
    let target = match effect {
        SelectionEffect::CompareSideB => Some(Surface::Compare),
        SelectionEffect::CompareSideA | SelectionEffect::DuplicateSelection => {
            Some(Surface::Artifact)
        }
        SelectionEffect::ViewedOnly => (surface != Surface::Compare).then_some(Surface::Artifact),
        SelectionEffect::Changed(_) | SelectionEffect::DuplicateSide => None,
    };
    let notice = matches!(
        effect,
        SelectionEffect::DuplicateSelection | SelectionEffect::DuplicateSide
    )
    .then(|| "比較には異なる候補が2件必要です。".to_owned());
    DockEvent {
        surface: target,
        notice,
    }
}

/// Arm an explicit change-side action; the next dock click updates that side (D-3).
pub(super) fn change_side(presentation: &mut Presentation, side: CompareSide) -> DockEvent {
    let armed = presentation.arm_change(side);
    DockEvent {
        surface: None,
        notice: Some(if armed {
            format!(
                "比較対象{}を変更します。ドックで候補を選択してください。",
                side.label()
            )
        } else {
            "その比較対象は利用できません。".to_owned()
        }),
    }
}

/// Close Compare and return to the artifact surface with the viewed candidate (D-7).
pub(super) fn close(presentation: &mut Presentation) -> DockEvent {
    presentation.close_compare();
    DockEvent {
        surface: Some(Surface::Artifact),
        notice: None,
    }
}

/// The documented unavailable notice after a REC3 restore decision (D-8).
pub(super) fn restore_notice(outcome: &RestoreOutcome) -> Option<String> {
    let mut missing = Vec::new();
    if outcome.viewed_missing {
        missing.push("表示中の候補");
    }
    if outcome.pair_missing {
        missing.push("比較ペア");
    }
    (!missing.is_empty()).then(|| {
        format!(
            "復元した{}は、現在の認可済み観測情報では利用できません。",
            missing.join("と")
        )
    })
}

/// A task switch discards the task-scoped references and starts empty (D-9).
pub(super) fn switch_task(presentation: &mut Presentation, task: TaskId) -> DockEvent {
    presentation.reset_for_task(task);
    DockEvent {
        surface: Some(Surface::Artifact),
        notice: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(label: &str) -> CandidateRef {
        CandidateRef {
            task_id: TaskId::fixture("task/Task-184"),
            run_id: desktop_protocol::RunId::fixture("run/Task-184/1"),
            iteration: 3,
            candidate_id: desktop_protocol::CandidateId::fixture(format!("candidate/{label}")),
            artifact_ref: desktop_protocol::ArtifactId::fixture(format!("artifact/{label}")),
        }
    }

    fn viewed(label: &str) -> ViewedCandidate {
        ViewedCandidate(candidate(label))
    }

    #[test]
    fn next_surface_is_the_canonical_cycle() {
        for surface in Surface::ALL {
            let mut navigation = Navigation::default();
            navigation.navigate(surface);
            navigation.next();
            assert_eq!(next_surface(surface), navigation.surface());
        }
    }

    #[test]
    fn ctrl_tab_never_enters_compare_without_two_candidates() {
        for count in [0usize, 1] {
            let mut navigation = Navigation::default();
            assert_eq!(navigation.surface(), Surface::Artifact);
            let event = advance(&mut navigation, count);
            assert_eq!(
                navigation.surface(),
                Surface::Artifact,
                "Ctrl+Tab must not reach Compare with {count} candidate(s)"
            );
            assert_eq!(event.surface, None);
            let notice = event.notice.unwrap_or_default();
            assert!(
                notice.contains("比較は利用できません") && notice.contains("候補が2件必要です"),
                "the same reason is stated: {notice}"
            );
        }
        for count in [2usize, 3] {
            let mut navigation = Navigation::default();
            let event = advance(&mut navigation, count);
            assert_eq!(event.surface, Some(Surface::Compare));
            assert_eq!(navigation.surface(), Surface::Compare);
            assert!(event.notice.is_none());
            let event = advance(&mut navigation, count);
            assert_eq!(event.surface, Some(Surface::Flow));
            assert_eq!(navigation.surface(), Surface::Flow);
        }
    }

    #[test]
    fn a_dock_selection_from_flow_shows_the_artifact_surface() {
        let mut presentation = Presentation::default();
        let event = select(&mut presentation, candidate("a"), Surface::Flow);
        assert_eq!(event.surface, Some(Surface::Artifact));
        assert!(event.notice.is_none());
        assert_eq!(presentation.viewed(), Some(&viewed("a")));
        assert_eq!(presentation.compare_pair(), None);
    }

    #[test]
    fn the_second_selection_completes_the_pair_and_shows_compare() {
        let mut presentation = Presentation::default();
        assert_eq!(
            select(&mut presentation, candidate("a"), Surface::Artifact).surface,
            Some(Surface::Artifact)
        );
        let event = select(&mut presentation, candidate("b"), Surface::Artifact);
        assert_eq!(event.surface, Some(Surface::Compare));
        assert_eq!(
            presentation.compare_pair(),
            Some(&[viewed("a"), viewed("b")])
        );
        assert_eq!(presentation.viewed(), Some(&viewed("b")));
    }

    #[test]
    fn a_plain_selection_while_compare_is_shown_keeps_compare() {
        let mut presentation = Presentation::default();
        select(&mut presentation, candidate("a"), Surface::Artifact);
        select(&mut presentation, candidate("b"), Surface::Artifact);
        let event = select(&mut presentation, candidate("c"), Surface::Compare);
        assert_eq!(event.surface, None, "the compare surface stays active");
        assert_eq!(presentation.viewed(), Some(&viewed("c")));
        assert_eq!(
            presentation.compare_pair(),
            Some(&[viewed("a"), viewed("b")])
        );
    }

    #[test]
    fn a_plain_selection_with_a_held_pair_from_flow_shows_the_artifact_surface() {
        let mut presentation = Presentation::default();
        select(&mut presentation, candidate("a"), Surface::Artifact);
        select(&mut presentation, candidate("b"), Surface::Artifact);
        let event = select(&mut presentation, candidate("c"), Surface::Flow);
        assert_eq!(event.surface, Some(Surface::Artifact));
        assert_eq!(presentation.viewed(), Some(&viewed("c")));
        assert_eq!(
            presentation.compare_pair(),
            Some(&[viewed("a"), viewed("b")])
        );
    }

    #[test]
    fn the_side_change_hint_matches_the_enabled_side() {
        assert_eq!(
            side_change_hint(false, false),
            Some("A側とB側の候補がそろうまで、A側・B側を変更する操作は使えません。")
        );
        assert_eq!(
            side_change_hint(true, false),
            Some("B側の候補がそろうまで、B側を変更する操作は使えません。"),
            "a pending A side keeps its own change available, so only B is called unavailable"
        );
        assert_eq!(side_change_hint(true, true), None);
    }

    #[test]
    fn one_candidate_selected_twice_never_forms_a_pair() {
        let mut presentation = Presentation::default();
        assert_eq!(
            select(&mut presentation, candidate("a"), Surface::Artifact).surface,
            Some(Surface::Artifact)
        );
        let event = select(&mut presentation, candidate("a"), Surface::Artifact);
        assert_eq!(
            presentation.compare_pair(),
            None,
            "one candidate cannot fill both sides"
        );
        assert_eq!(event.surface, Some(Surface::Artifact));
        let notice = event.notice.unwrap_or_default();
        assert!(
            notice.contains("比較には異なる候補が2件必要です"),
            "the refusal states its reason: {notice}"
        );
    }

    #[test]
    fn change_side_arms_and_rejects_the_other_side_identity() {
        let mut presentation = Presentation::default();
        select(&mut presentation, candidate("a"), Surface::Artifact);
        select(&mut presentation, candidate("b"), Surface::Artifact);
        change_side(&mut presentation, CompareSide::A);
        assert_eq!(presentation.changing_side(), Some(CompareSide::A));
        let event = select(&mut presentation, candidate("b"), Surface::Compare);
        assert_eq!(
            presentation.compare_pair(),
            Some(&[viewed("a"), viewed("b")]),
            "a refused change leaves the pair untouched"
        );
        assert_eq!(event.surface, None);
        let notice = event.notice.unwrap_or_default();
        assert!(
            notice.contains("比較には異なる候補が2件必要です"),
            "the refusal states its reason: {notice}"
        );
    }

    #[test]
    fn close_returns_to_the_artifact_surface_and_keeps_viewed() {
        let mut presentation = Presentation::default();
        select(&mut presentation, candidate("a"), Surface::Artifact);
        select(&mut presentation, candidate("b"), Surface::Artifact);
        let event = close(&mut presentation);
        assert_eq!(event.surface, Some(Surface::Artifact));
        assert!(event.notice.is_none());
        assert_eq!(presentation.compare_pair(), None);
        assert_eq!(presentation.viewed(), Some(&viewed("b")));
    }

    #[test]
    fn a_restored_identical_identity_pair_is_unavailable_with_a_notice() {
        let mut presentation = Presentation::default();
        let a = candidate("a");
        let duplicate = [a.clone(), a.clone()];
        let outcome = presentation.restore(None, Some(&duplicate), |reference| *reference == a);
        assert!(outcome.pair_missing);
        assert_eq!(presentation.compare_pair(), None);
        assert!(restore_notice(&outcome).is_some());
    }

    #[test]
    fn a_task_switch_starts_empty_and_shows_the_artifact_surface() {
        let mut presentation = Presentation::default();
        select(&mut presentation, candidate("a"), Surface::Artifact);
        select(&mut presentation, candidate("b"), Surface::Artifact);
        let event = switch_task(&mut presentation, TaskId::fixture("task/other"));
        assert_eq!(event.surface, Some(Surface::Artifact));
        assert_eq!(presentation.viewed(), None);
        assert_eq!(presentation.compare_pair(), None);
    }

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
