//! High-contrast colour selection for the native shell (MP-AX1 / AX04 limited slice).
//!
//! The owner decision recorded in IMPLEMENTATION_MASTER_PLAN §19.104 is: High
//! Contrast is handled by **adding a high-contrast value set in the token layer**
//! and switching when the OS setting is present, with the operator changing the OS
//! setting manually and no automatic OS change.
//!
//! This module therefore owns exactly two colour sets:
//!
//! * [`ContrastScheme::Normal`] — the already-approved shell literals (the root
//!   background/text and the dock focus/selection/border colours that existed
//!   before this slice). No new palette value is introduced here.
//! * [`ContrastScheme::HighContrast`] — the **OS-provided system colours** handed
//!   in by the platform probe (window, window text, highlight, highlight text,
//!   gray text, button face, button text). Nothing is invented: when the probe
//!   cannot read them there is no high-contrast set and the shell keeps the
//!   normal scheme.
//!
//! The switch is a pure colour rebuild: [`ContrastState::observe`] never touches
//! view state (surface, task, viewed/compare identity or focus), which the
//! caller proves by keeping those outside this type.

use std::sync::Arc;
use std::sync::OnceLock;

use gpui::{Context, Task};

/// Which colour set the shell is drawing with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ContrastScheme {
    /// The approved shell palette.
    #[default]
    Normal,
    /// Windows High Contrast: system colours only.
    HighContrast,
}

/// The OS system colours used by the high-contrast set.
///
/// Every field is a `0xRRGGBB` value reported by the platform probe; this type
/// never supplies a fallback value of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SystemColors {
    /// Window background (`COLOR_WINDOW` / `HKCU\Control Panel\Colors\Window`).
    pub window: u32,
    /// Ordinary window text (`WindowText`).
    pub window_text: u32,
    /// Selection background (`Highlight`).
    pub highlight: u32,
    /// Text drawn on the selection (`HighlightText`).
    pub highlight_text: u32,
    /// Disabled text (`GrayText`).
    pub gray_text: u32,
    /// Button/pressed surface (`ButtonFace`).
    pub button_face: u32,
    /// Button text (`ButtonText`).
    pub button_text: u32,
}

/// The shell colour roles, resolved for one scheme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShellColors {
    /// Window/shell background.
    pub background: u32,
    /// Ordinary text.
    pub text: u32,
    /// Keyboard-focus indicator. Always distinct from `selection`.
    pub focus: u32,
    /// Selection background. Always distinct from `focus`.
    pub selection: u32,
    /// Text drawn on a selection.
    pub selection_text: u32,
    /// Disabled text/indicator. Always distinct from `focus` and `selection`.
    pub disabled: u32,
    /// Structural border.
    pub border: u32,
    /// Button/raised surface.
    pub button_face: u32,
    /// Destructive-action surface (the stop button's face).
    pub danger_face: u32,
    /// Text on the destructive-action surface.
    pub danger_text: u32,
    /// Destructive-action border.
    pub danger_border: u32,
    /// Raised surface (the receipt-check button).
    pub raised_face: u32,
    /// Surface of a disabled control.
    pub disabled_face: u32,
    /// Muted text (axis labels, secondary information).
    pub muted_text: u32,
    /// Border of a disabled control.
    pub disabled_border: u32,
}

/// The approved normal-scheme colours (the shell literals this slice reuses).
pub const NORMAL_COLORS: ShellColors = ShellColors {
    background: 0x17191c,
    text: 0xe6e8eb,
    focus: 0xe6c278,
    selection: 0xaeccec,
    selection_text: 0x17191c,
    disabled: 0x8b949e,
    border: 0x626870,
    button_face: 0x202328,
    danger_face: 0x5b2a2f,
    danger_text: 0xf4dfe0,
    danger_border: 0x91464a,
    raised_face: 0x343a42,
    disabled_face: 0x313438,
    muted_text: 0x9da3aa,
    disabled_border: 0x51565d,
};

/// Resolve the shell colours for one scheme.
///
/// Normal returns the approved literals; High Contrast maps the caller-provided
/// OS system colours one role at a time. The focus role uses the system button
/// text colour, which Windows high-contrast themes keep distinct from
/// `Highlight`, so focus and selection never collapse into one indicator.
pub fn colors_for(scheme: ContrastScheme, system: &SystemColors) -> ShellColors {
    match scheme {
        ContrastScheme::Normal => NORMAL_COLORS,
        ContrastScheme::HighContrast => ShellColors {
            background: system.window,
            text: system.window_text,
            focus: system.button_text,
            selection: system.highlight,
            selection_text: system.highlight_text,
            disabled: system.gray_text,
            border: system.window_text,
            button_face: system.button_face,
            danger_face: system.highlight,
            danger_text: system.highlight_text,
            danger_border: system.button_text,
            raised_face: system.button_face,
            disabled_face: system.window,
            muted_text: system.gray_text,
            disabled_border: system.gray_text,
        },
    }
}

/// Resolve one visual role while keeping the already-approved Normal value unchanged.
///
/// High Contrast callers supply an existing OS-palette role; they never manufacture a
/// colour. Keeping the scheme explicit avoids confusing a system palette that happens
/// to resemble the Normal palette with the Normal scheme.
pub fn role_color(scheme: ContrastScheme, normal: u32, high_contrast: u32) -> u32 {
    match scheme {
        ContrastScheme::Normal => normal,
        ContrastScheme::HighContrast => high_contrast,
    }
}

/// Choose readable text on its actual surface without replacing ordinary information
/// with a low-contrast disabled/gray role. Every candidate is from the resolved palette.
pub fn readable_text(colors: &ShellColors, preferred: u32, surface: u32) -> u32 {
    let candidates = [
        preferred,
        colors.text,
        colors.selection_text,
        colors.background,
        colors.button_face,
    ];
    candidates
        .into_iter()
        .find(|color| contrast_ratio(*color, surface) >= 4.5)
        .unwrap_or_else(|| {
            candidates
                .into_iter()
                .max_by(|left, right| {
                    contrast_ratio(*left, surface)
                        .partial_cmp(&contrast_ratio(*right, surface))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .unwrap_or(colors.text)
        })
}

/// The palette in force for one render: which scheme is active and the colours it resolved to.
///
/// Drawing code takes this one value instead of a scheme/colour pair, so a surface can never
/// resolve a role against a different scheme than the one it reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub scheme: ContrastScheme,
    pub colors: ShellColors,
}

impl Palette {
    /// Resolve one visual role, keeping the approved normal literal unchanged.
    pub fn color(&self, normal: u32, high_contrast: u32) -> u32 {
        role_color(self.scheme, normal, high_contrast)
    }

    /// Readable text on `surface`, resolved through this palette.
    pub fn readable(&self, preferred: u32, surface: u32) -> u32 {
        readable_text(&self.colors, preferred, surface)
    }
}

/// The Dock surface colour the focus ring is drawn on.
pub const FOCUS_SURFACE_ROLE: fn(&ShellColors) -> u32 = |colors| colors.button_face;

/// Relative luminance of an `0xRRGGBB` colour (WCAG 2.x).
fn relative_luminance(color: u32) -> f32 {
    let channel = |shift: u32| {
        let value = ((color >> shift) & 0xff) as f32 / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
}

/// WCAG contrast ratio between two `0xRRGGBB` colours (1.0 … 21.0).
pub fn contrast_ratio(first: u32, second: u32) -> f32 {
    let a = relative_luminance(first);
    let b = relative_luminance(second);
    let (lighter, darker) = if a >= b { (a, b) } else { (b, a) };
    (lighter + 0.05) / (darker + 0.05)
}

/// How the focused Dock ring is made visible against its surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FocusRing {
    /// A role whose contrast against the surface reaches the visible threshold.
    Visible(u32),
    /// Every role collapses into the surface colour: colour alone cannot show the
    /// ring. The caller must not claim a visible focus colour in this case.
    Invisible,
}

/// The minimum WCAG contrast ratio accepted for the focus ring.
pub const FOCUS_RING_MIN_CONTRAST: f32 = 3.0;

/// Pick a focus-ring colour that is actually visible on the Dock surface.
///
/// The role order is focus → selection → text: when the OS gives the focus colour
/// the same (or a near) value as the surface, the next distinct role is used, so a
/// monochrome-but-partly-distinct palette still gets a visible ring. If every role
/// collapses into the surface, the result is [`FocusRing::Invisible`] rather than a
/// ring that cannot be seen.
pub fn focus_ring(colors: &ShellColors) -> FocusRing {
    focus_ring_on(colors, FOCUS_SURFACE_ROLE(colors), &[])
}

/// Pick a focus-ring colour that is visible against the **actual** surface it is
/// drawn on.
///
/// Callers whose control surface is not `button_face` (the stop button draws on
/// `danger_face`) must pass that background explicitly, plus any additional role
/// that is valid on that surface (the stop button's `danger_text`); the contrast is
/// then evaluated against the real background instead of the Dock surface.
pub fn focus_ring_on(colors: &ShellColors, background: u32, extra_candidates: &[u32]) -> FocusRing {
    let candidates = [colors.focus, colors.selection, colors.text];
    for candidate in candidates.iter().chain(extra_candidates.iter()).copied() {
        if contrast_ratio(candidate, background) >= FOCUS_RING_MIN_CONTRAST {
            return FocusRing::Visible(candidate);
        }
    }
    FocusRing::Invisible
}

/// One foreground/background pair the shell actually draws text on.
#[derive(Clone, Copy, Debug)]
pub struct TextPair {
    /// What the pair is for, so a failure names the surface rather than two numbers.
    pub purpose: &'static str,
    pub foreground: u32,
    pub background: u32,
    /// Whether the text is large enough for the 3:1 threshold rather than the 4.5:1 one.
    pub large_text: bool,
    /// Whether WCAG exempts this pair from its contrast minimum because the control is inactive.
    pub exempt: bool,
}

impl TextPair {
    /// The WCAG AA ratio this pair must meet.
    pub fn required_ratio(&self) -> f32 {
        if self.large_text { 3.0 } else { 4.5 }
    }

    /// The ratio this pair actually has.
    pub fn actual_ratio(&self) -> f32 {
        contrast_ratio(self.foreground, self.background)
    }

    /// Whether the pair meets its threshold, or is exempt from having one.
    pub fn meets_threshold(&self) -> bool {
        self.exempt || self.actual_ratio() >= self.required_ratio()
    }
}

/// The text-on-surface pairs this audit enumerates, taken from the drawing code rather than
/// invented here.
///
/// **What this is:** the combinations a review of `lib.rs` found the shell drawing: the row states
/// resolved through `dock_row_visual`, the stop control through `stop_control_visual`, the axis
/// labels, the adopted-candidate label and the task rail's fixed pair.
///
/// **What this is not:** a proof of completeness. The list is maintained by hand against the
/// drawing code, so a new `text_color(...)` call can appear without a row here; the audit says so
/// instead of claiming every pair is covered.
pub fn drawn_text_pairs(colors: &ShellColors) -> Vec<TextPair> {
    let mut pairs = vec![
        TextPair {
            purpose: "body text on the shell background",
            foreground: colors.text,
            background: colors.background,
            large_text: false,
            exempt: false,
        },
        TextPair {
            purpose: "muted text (axis labels) on the shell background",
            foreground: colors.muted_text,
            background: colors.background,
            large_text: false,
            exempt: false,
        },
        TextPair {
            purpose: "text on a raised surface",
            foreground: colors.text,
            background: colors.raised_face,
            large_text: false,
            exempt: false,
        },
        TextPair {
            purpose: "text on a button face",
            foreground: colors.text,
            background: colors.button_face,
            large_text: false,
            exempt: false,
        },
        TextPair {
            purpose: "selection text on a selection",
            foreground: colors.selection_text,
            background: colors.selection,
            large_text: false,
            exempt: false,
        },
        TextPair {
            purpose: "destructive text on the destructive face",
            foreground: colors.danger_text,
            background: colors.danger_face,
            large_text: false,
            exempt: false,
        },
        // An axis label is ordinary information, not an inactive control: it is drawn with the
        // muted/disabled role but carries real content, so it is not exempt.
        TextPair {
            purpose: "axis label on the panel face",
            foreground: colors.disabled,
            background: colors.button_face,
            large_text: false,
            exempt: false,
        },
        TextPair {
            purpose: "adopted-candidate label on the dock row",
            foreground: colors.focus,
            background: colors.button_face,
            large_text: false,
            exempt: false,
        },
        TextPair {
            purpose: "task rail text on an idle row",
            foreground: colors.text,
            background: colors.button_face,
            large_text: false,
            exempt: false,
        },
        TextPair {
            purpose: "task rail text on the active row",
            foreground: colors.selection_text,
            background: colors.selection,
            large_text: false,
            exempt: false,
        },
        // The disabled stop control resolves its text through the same entry point the product
        // uses, so this row cannot drift from what is drawn.
        TextPair {
            purpose: "disabled stop control text on its face",
            foreground: stop_control_text_for_audit(colors),
            background: colors.disabled_face,
            large_text: false,
            // Inactive control: WCAG exempts it, and the row stays listed anyway.
            exempt: true,
        },
    ];
    // The four states the candidate dock actually generates (`Normal`, `Selected`, `Focused`,
    // `SelectedAndFocused`), plus `Disabled`, which the dock cannot currently reach but the model
    // defines and other surfaces may use. The disabled row is therefore a **model** row, not an
    // observation of the dock.
    for (state, purpose) in [
        (crate::shell_style::RowState::Normal, "plain dock row label"),
        (
            crate::shell_style::RowState::Selected,
            "selected dock row label",
        ),
        (
            crate::shell_style::RowState::Focused,
            "focused dock row label",
        ),
        (
            crate::shell_style::RowState::SelectedAndFocused,
            "selected and focused dock row label",
        ),
        (
            crate::shell_style::RowState::Disabled,
            // Model state: not reachable from the candidate dock today.
            "disabled dock row label (model state)",
        ),
    ] {
        pairs.push(TextPair {
            purpose,
            foreground: crate::shell_style::dock_row_visual(state, colors).text,
            background: colors.button_face,
            large_text: false,
            exempt: state == crate::shell_style::RowState::Disabled,
        });
    }
    pairs
}

/// The disabled stop control's text, resolved the way the product resolves it.
fn stop_control_text_for_audit(colors: &ShellColors) -> u32 {
    crate::shell_style::stop_control_visual(false, colors).text
}

/// A non-text indicator and the surface it is drawn against.
///
/// WCAG 1.4.11 asks for **3:1** from a graphic that is needed to identify a control or its state.
#[derive(Clone, Copy, Debug)]
pub struct NonTextPair {
    pub purpose: &'static str,
    pub indicator: u32,
    pub surface: u32,
    /// Whether this indicator is one the shell relies on to show state, and so must meet 3:1.
    pub carries_state: bool,
}

impl NonTextPair {
    pub fn ratio(&self) -> f32 {
        contrast_ratio(self.indicator, self.surface)
    }
}

/// The non-text indicators the shell draws, measured against the surfaces they sit on.
///
/// `carries_state` is the question that decides the requirement: a **focus ring, a selection bar or
/// a disabled boundary** tells the user something, so it needs 3:1; a **decorative separator** does
/// not. A boundary that both delineates a control and is its only cue is listed as carrying state.
pub fn non_text_pairs(colors: &ShellColors) -> Vec<NonTextPair> {
    vec![
        NonTextPair {
            purpose: "keyboard focus ring on the shell background",
            indicator: colors.focus,
            surface: colors.background,
            carries_state: true,
        },
        NonTextPair {
            purpose: "keyboard focus ring on a button face",
            indicator: colors.focus,
            surface: colors.button_face,
            carries_state: true,
        },
        NonTextPair {
            purpose: "keyboard focus ring on a raised surface",
            indicator: colors.focus,
            surface: colors.raised_face,
            carries_state: true,
        },
        NonTextPair {
            purpose: "selection indicator on the dock row surface",
            indicator: crate::shell_style::selection_indicator(colors, colors.button_face),
            surface: colors.button_face,
            carries_state: true,
        },
        NonTextPair {
            purpose: "structural separator on the shell background",
            indicator: colors.border,
            surface: colors.background,
            // Decorative: it separates regions and is not the only cue for any state.
            carries_state: false,
        },
        NonTextPair {
            purpose: "disabled control boundary on the shell background",
            indicator: colors.disabled_border,
            surface: colors.background,
            // Inactive control: exempt from the minimum, but measured and listed.
            carries_state: false,
        },
    ]
}

/// The indicators that carry state but fall below 3:1, as `(purpose, ratio)`.
pub fn failing_non_text_pairs(colors: &ShellColors) -> Vec<(&'static str, f32)> {
    non_text_pairs(colors)
        .into_iter()
        .filter(|pair| pair.carries_state && pair.ratio() < 3.0)
        .map(|pair| (pair.purpose, pair.ratio()))
        .collect()
}

/// The resting boundary of an interactive control, as the drawing code pins it.
///
/// It is measured and reported separately because it is **not** a state indicator: the control is
/// also identified by its label and its surface. Its ratio is recorded so a future decision about
/// the boundary has a number to start from.
/// The resting boundary of an interactive control, shared with the drawing code.
pub const RESTING_CONTROL_BORDER: u32 = crate::shell_style::RESTING_CONTROL_BORDER;

/// The surfaces a resting control boundary is actually drawn against.
pub const RESTING_BOUNDARY_SURFACES: &[(&str, u32)] = &[
    ("unselected surface row", 0x24272c),
    ("selected surface row", 0x3c5268),
    ("unselected flow node", 0x1e2124),
    ("intervention notification", 0x2a2f36),
    ("raised surface", 0x343a42),
    ("button face", 0x202328),
];

/// Palette role names exposed by the legacy contrast summary.
pub const AUDITED_ROLES: &[&str] = &[
    "text",
    "muted_text",
    "disabled",
    "focus",
    "danger_text",
    "selection_text",
];

/// The pairs that do not meet their threshold, as `(purpose, ratio, required)`.
pub fn failing_text_pairs(colors: &ShellColors) -> Vec<(&'static str, f32, f32)> {
    drawn_text_pairs(colors)
        .into_iter()
        .filter(|pair| !pair.meets_threshold())
        .map(|pair| (pair.purpose, pair.actual_ratio(), pair.required_ratio()))
        .collect()
}

/// The first candidate that clears `minimum` against `surface`, or the last candidate if none does.
///
/// Returning a value either way keeps this total: a caller never has to decide what "invisible"
/// means, and the fallback is the candidate with the best chance rather than a panic.
pub fn first_visible(candidates: &[u32], surface: u32) -> u32 {
    let mut best = candidates[0];
    for candidate in candidates {
        if contrast_ratio(*candidate, surface) >= 3.0 {
            return *candidate;
        }
        if contrast_ratio(*candidate, surface) > contrast_ratio(best, surface) {
            best = *candidate;
        }
    }
    best
}

/// Focus, selection and disabled must remain three different indicators.
pub fn focus_selection_and_disabled_are_distinct(colors: &ShellColors) -> bool {
    colors.focus != colors.selection
        && colors.focus != colors.disabled
        && colors.selection != colors.disabled
}

/// The shell's current contrast state: the scheme and its resolved colours.
#[derive(Clone, Copy, Debug)]
pub struct ContrastState {
    scheme: ContrastScheme,
    colors: ShellColors,
}

impl Default for ContrastState {
    fn default() -> Self {
        Self {
            scheme: ContrastScheme::Normal,
            colors: NORMAL_COLORS,
        }
    }
}

impl ContrastState {
    /// The current scheme.
    pub fn scheme(&self) -> ContrastScheme {
        self.scheme
    }

    /// The resolved colours for the current scheme.
    pub fn colors(&self) -> ShellColors {
        self.colors
    }

    /// Apply a probe reading.
    ///
    /// Returns `true` exactly when the resolved colours changed, so the caller
    /// only repaints when there is something new. Nothing outside the colour
    /// state is touched: surface, task identity, viewed/compare selection and
    /// focus all live elsewhere and cannot be reset from here.
    pub fn observe(&mut self, scheme: ContrastScheme, system: &SystemColors) -> bool {
        let colors = colors_for(scheme, system);
        if self.scheme == scheme && self.colors == colors {
            return false;
        }
        self.scheme = scheme;
        self.colors = colors;
        true
    }
}

/// A bounded platform probe: `Some(reading)` when the OS reports a usable
/// scheme, `None` when it cannot be read (the shell then keeps its current
/// state instead of inventing high-contrast colours).
pub type ContrastProbe = Arc<dyn Fn() -> Option<(ContrastScheme, SystemColors)> + Send + Sync>;

/// The platform probe registered by the composition root before the shell opens.
///
/// This is a registration seam, not a second configuration source: the shell
/// reads it once per poll and never mutates it.
pub fn set_platform_probe(probe: ContrastProbe) {
    let _ = PROBE.set(probe);
}

/// The registered platform probe, if any.
pub(crate) fn platform_probe() -> Option<ContrastProbe> {
    PROBE.get().cloned()
}

static PROBE: OnceLock<ContrastProbe> = OnceLock::new();

/// Poll the registered platform probe while the shell lives and repaint only when
/// the resolved colours changed.
///
/// A change is a colour rebuild only: the shell's surface, task identity,
/// viewed/compare selection and focus live in their own fields, so the operator
/// toggling High Contrast neither restarts the app nor resets the UI state.
pub(crate) fn contrast_probe_worker(
    probe: ContrastProbe,
    cx: &mut Context<crate::Shell>,
) -> Task<()> {
    cx.spawn(
        async move |this: gpui::WeakEntity<crate::Shell>, cx: &mut gpui::AsyncApp| {
            let background = cx.background_executor().clone();
            loop {
                background.timer(std::time::Duration::from_secs(2)).await;
                // The OS query is a blocking process call: run it on the background
                // executor, never on the UI thread, and only apply its result here.
                let probe = Arc::clone(&probe);
                let reading = background.spawn(async move { probe() }).await;
                let Some((scheme, system)) = reading else {
                    continue;
                };
                let updated = this.update(cx, move |shell, cx| {
                    if shell.observe_contrast(scheme, &system) {
                        cx.notify();
                    }
                });
                if updated.is_err() {
                    break;
                }
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn system() -> SystemColors {
        SystemColors {
            window: 0x000000,
            window_text: 0xffffff,
            highlight: 0x1aebff,
            highlight_text: 0x000000,
            gray_text: 0x8c8c8c,
            button_face: 0x202020,
            button_text: 0xffff00,
        }
    }

    #[test]
    fn the_normal_scheme_uses_the_approved_palette() {
        let state = ContrastState::default();
        assert_eq!(state.scheme(), ContrastScheme::Normal);
        assert_eq!(state.colors(), NORMAL_COLORS);
        // The literals are the ones the shell already used.
        assert_eq!(state.colors().background, 0x17191c);
        assert_eq!(state.colors().text, 0xe6e8eb);
        assert_eq!(state.colors().focus, 0xe6c278);
        assert_eq!(state.colors().selection, 0xaeccec);
        assert_eq!(state.colors().border, 0x626870);
    }

    #[test]
    fn the_high_contrast_scheme_uses_only_os_system_colors() {
        let system = system();
        let colors = colors_for(ContrastScheme::HighContrast, &system);
        assert_eq!(colors.background, system.window);
        assert_eq!(colors.text, system.window_text);
        assert_eq!(colors.focus, system.button_text);
        assert_eq!(colors.selection, system.highlight);
        assert_eq!(colors.selection_text, system.highlight_text);
        assert_eq!(colors.disabled, system.gray_text);
        assert_eq!(colors.border, system.window_text);
        assert_eq!(colors.button_face, system.button_face);
        assert_eq!(colors.danger_face, system.highlight);
        assert_eq!(colors.danger_text, system.highlight_text);
        assert_eq!(colors.danger_border, system.button_text);
        assert_eq!(colors.raised_face, system.button_face);
        assert_eq!(colors.disabled_face, system.window);
        assert_eq!(colors.muted_text, system.gray_text);
        assert_eq!(colors.disabled_border, system.gray_text);
        assert_ne!(
            colors.background, NORMAL_COLORS.background,
            "high contrast must come from the OS, not the designed palette"
        );
    }

    /// The black High Contrast theme Windows offers.
    fn hc_black() -> SystemColors {
        SystemColors {
            window: 0x000000,
            window_text: 0xffffff,
            highlight: 0x000080,
            highlight_text: 0xffffff,
            gray_text: 0x808080,
            button_face: 0x000000,
            button_text: 0xffffff,
        }
    }

    /// The white High Contrast theme, which is the mirror case.
    fn hc_white() -> SystemColors {
        SystemColors {
            window: 0xffffff,
            window_text: 0x000000,
            highlight: 0x316ac5,
            highlight_text: 0xffffff,
            gray_text: 0x6d6d6d,
            button_face: 0xffffff,
            button_text: 0x000000,
        }
    }

    /// The set of values a probe actually offers.
    fn probe_values(system: &SystemColors) -> Vec<u32> {
        vec![
            system.window,
            system.window_text,
            system.highlight,
            system.highlight_text,
            system.gray_text,
            system.button_face,
            system.button_text,
        ]
    }

    fn palette_values(colors: &ShellColors) -> Vec<u32> {
        vec![
            colors.background,
            colors.text,
            colors.focus,
            colors.selection,
            colors.selection_text,
            colors.disabled,
            colors.border,
            colors.button_face,
            colors.danger_face,
            colors.danger_text,
            colors.danger_border,
            colors.raised_face,
            colors.disabled_face,
            colors.muted_text,
            colors.disabled_border,
        ]
    }

    #[test]
    fn a_probe_sequence_repaints_only_when_the_palette_changes() {
        // Normal -> HC black -> HC black again -> HC white -> normal. The repaint flag must be
        // exactly "the resolved colours changed", or the shell repaints on every probe tick.
        let mut state = ContrastState::default();
        let steps: [(ContrastScheme, SystemColors, bool); 5] = [
            (ContrastScheme::Normal, hc_black(), false),
            (ContrastScheme::HighContrast, hc_black(), true),
            (ContrastScheme::HighContrast, hc_black(), false),
            (ContrastScheme::HighContrast, hc_white(), true),
            (ContrastScheme::Normal, hc_white(), true),
        ];
        for (index, (scheme, system, expected)) in steps.iter().enumerate() {
            let changed = state.observe(*scheme, system);
            assert_eq!(
                changed, *expected,
                "step {index} ({scheme:?}) reported changed={changed}"
            );
            assert_eq!(state.scheme(), *scheme);
            // State-indicator measurements remain independent of text drawing literals.
            assert!(
                failing_non_text_pairs(&state.colors()).is_empty(),
                "step {index} left state indicators below 3:1"
            );
        }
    }

    #[test]
    fn the_repaint_flag_describes_the_palette_not_the_probe() {
        // Normal ignores the probe entirely, so two different probes resolve to the same palette.
        // That is where two readings can differ in input and agree in output: the first Normal
        // reading must repaint (the palette changed from High Contrast), the second must not.
        let mut state = ContrastState::default();
        assert!(
            state.observe(ContrastScheme::HighContrast, &hc_black()),
            "entering High Contrast must repaint"
        );
        assert!(
            state.observe(ContrastScheme::Normal, &hc_black()),
            "leaving High Contrast must repaint"
        );
        let normal = palette_values(&state.colors());
        assert!(
            !state.observe(ContrastScheme::Normal, &hc_white()),
            "a different probe resolving to the same palette must not repaint"
        );
        assert_eq!(palette_values(&state.colors()), normal);
        assert_eq!(normal, palette_values(&NORMAL_COLORS));
    }
    #[test]
    fn the_high_contrast_palette_only_uses_values_the_probe_offered() {
        // A role that quietly came from the normal palette would be a real defect: under High
        // Contrast every colour must follow the system theme.
        for system in [hc_black(), hc_white()] {
            let colors = colors_for(ContrastScheme::HighContrast, &system);
            let offered = probe_values(&system);
            for value in palette_values(&colors) {
                assert!(
                    offered.contains(&value),
                    "{value:#x} is not one of the probe's values, so it does not follow the scheme"
                );
            }
        }
    }

    #[test]
    fn the_high_contrast_mapping_is_deterministic_and_follows_the_probe() {
        let black = colors_for(ContrastScheme::HighContrast, &hc_black());
        let black_again = colors_for(ContrastScheme::HighContrast, &hc_black());
        let white = colors_for(ContrastScheme::HighContrast, &hc_white());
        assert_eq!(palette_values(&black), palette_values(&black_again));
        assert_eq!(black.background, 0x000000);
        assert_eq!(black.text, 0xffffff);
        assert_eq!(white.background, 0xffffff);
        assert_eq!(white.text, 0x000000);
        // The two themes must not resolve to the same palette, or the probe is being ignored.
        assert_ne!(palette_values(&black), palette_values(&white));
    }

    #[test]
    fn a_theme_that_separates_window_and_button_face_keeps_the_selection_visible() {
        // The reviewer's case: window white, button face black. An indicator chosen against the
        // window would be invisible on the row it is actually drawn on.
        let split = SystemColors {
            window: 0xffffff,
            window_text: 0x000000,
            highlight: 0xffffff,
            highlight_text: 0x000000,
            gray_text: 0x6d6d6d,
            button_face: 0x000000,
            button_text: 0xffffff,
        };
        let colors = colors_for(ContrastScheme::HighContrast, &split);
        assert_ne!(
            colors.background, colors.button_face,
            "the probe must separate the two surfaces for this test to mean anything"
        );
        let indicator = crate::shell_style::selection_indicator(&colors, colors.button_face);
        let ratio = contrast_ratio(indicator, colors.button_face);
        assert!(
            ratio >= 3.0,
            "the selection indicator is {ratio:.2}:1 on the row surface it is drawn on"
        );
        // And the row visual really uses that value.
        assert_eq!(
            crate::shell_style::row_visual(
                crate::shell_style::RowState::Selected,
                &colors,
                colors.button_face
            )
            .border,
            indicator
        );
    }
    #[test]
    fn both_high_contrast_themes_keep_state_indicators_visible() {
        for (name, system) in [("black", hc_black()), ("white", hc_white())] {
            let colors = colors_for(ContrastScheme::HighContrast, &system);
            let failing_non_text = failing_non_text_pairs(&colors);
            assert!(
                failing_non_text.is_empty(),
                "{name} theme, state indicators: {failing_non_text:?}"
            );
        }
    }
    #[test]
    fn the_state_indicators_meet_the_non_text_threshold() {
        let failing = failing_non_text_pairs(&NORMAL_COLORS);
        assert!(
            failing.is_empty(),
            "state indicators below 3:1: {failing:?}"
        );
        for pair in non_text_pairs(&NORMAL_COLORS) {
            eprintln!(
                "{}: {:#x} on {:#x} is {:.2}:1 (state: {})",
                pair.purpose,
                pair.indicator,
                pair.surface,
                pair.ratio(),
                pair.carries_state
            );
        }
    }

    #[test]
    fn the_resting_control_boundary_ratio_is_recorded() {
        // Not a threshold assertion: the boundary is not the control's only cue. The number is
        // printed and asserted to be a real measurement so a change to either value is visible.
        let mut measured = 0usize;
        for (surface_name, surface) in RESTING_BOUNDARY_SURFACES {
            let ratio = contrast_ratio(RESTING_CONTROL_BORDER, *surface);
            eprintln!("resting control boundary on {surface_name}: {ratio:.2}:1");
            assert!(ratio > 0.0, "{surface_name} produced no ratio");
            measured += 1;
        }
        assert_eq!(measured, RESTING_BOUNDARY_SURFACES.len());
        // The owner decision keeps the boundary decorative, so there is no threshold here: a change
        // that raises it must not fail this test. The ratios are printed so any change is visible.
    }
    #[test]
    fn focus_selection_and_disabled_are_distinct_in_both_schemes() {
        let system = system();
        for scheme in [ContrastScheme::Normal, ContrastScheme::HighContrast] {
            let colors = colors_for(scheme, &system);
            assert!(
                focus_selection_and_disabled_are_distinct(&colors),
                "focus, selection and disabled must be three indicators in {scheme:?}"
            );
        }
    }

    #[test]
    fn semantic_roles_preserve_normal_values_and_keep_information_readable() {
        assert_eq!(
            role_color(ContrastScheme::Normal, 0x24272c, 0xffffff),
            0x24272c
        );
        let mut system = hc_white();
        system.gray_text = 0xaaaaaa;
        let colors = colors_for(ContrastScheme::HighContrast, &system);
        assert_eq!(
            role_color(ContrastScheme::HighContrast, 0x24272c, colors.button_face),
            colors.button_face
        );
        let foreground = readable_text(&colors, colors.disabled, colors.background);
        assert!(
            contrast_ratio(foreground, colors.background) >= 4.5,
            "ordinary information must remain readable on the surface it is drawn on"
        );
        assert!(
            foreground != colors.disabled,
            "gray system text must not be trusted for ordinary information when it is too faint"
        );
    }
    #[test]
    fn ordinary_text_resolves_against_each_actual_high_contrast_surface() {
        for (name, system) in [("black", hc_black()), ("white", hc_white())] {
            let colors = colors_for(ContrastScheme::HighContrast, &system);
            for (surface_name, surface) in [
                ("window", colors.background),
                ("raised", colors.raised_face),
                ("button", colors.button_face),
                ("selected", colors.selection),
                ("disabled", colors.disabled_face),
            ] {
                let foreground = readable_text(&colors, colors.muted_text, surface);
                let ratio = contrast_ratio(foreground, surface);
                assert!(
                    ratio >= 4.5,
                    "{name} theme ordinary text on {surface_name} surface is {ratio:.2}:1"
                );
            }
        }
    }

    /// Scope of this test: the **colour state only**. `ContrastState` holds the
    /// scheme and its resolved colours and nothing else, so a switch cannot address
    /// task/viewed/compare/focus state — but this unit test does not observe the
    /// real `Shell`; that separation is structural (`Shell::observe_contrast`
    /// mutates only its private `contrast` field) and the native HC toggle is
    /// `NOT_RUN`. No view-state guarantee is claimed from this test.
    #[test]
    fn a_switch_rebuilds_only_the_colour_state() {
        let mut state = ContrastState::default();
        let normal = state.colors();
        let changed_to_hc = state.observe(ContrastScheme::HighContrast, &system());
        assert!(changed_to_hc, "the scheme really changed");
        assert_eq!(state.scheme(), ContrastScheme::HighContrast);
        assert_ne!(state.colors(), normal, "tokens were rebuilt");

        // The same scheme again is a no-op: no repaint and no colour churn.
        assert!(!state.observe(ContrastScheme::HighContrast, &system()));
        let changed_back = state.observe(ContrastScheme::Normal, &system());
        assert!(changed_back);
        assert_eq!(state.scheme(), ContrastScheme::Normal);
        assert_eq!(state.colors(), NORMAL_COLORS);
    }
}
