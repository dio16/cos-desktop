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
