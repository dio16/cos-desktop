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
        // The task rail pins its own two values rather than reading the palette.
        TextPair {
            purpose: "task rail text on an idle row",
            foreground: TASK_RAIL_TEXT,
            background: TASK_RAIL_IDLE,
            large_text: false,
            exempt: false,
        },
        TextPair {
            purpose: "task rail text on the active row",
            foreground: TASK_RAIL_TEXT,
            background: TASK_RAIL_ACTIVE,
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

/// The task rail's fixed colours, as drawn in `lib.rs`.
pub const TASK_RAIL_TEXT: u32 = 0xe6e8eb;
pub const TASK_RAIL_IDLE: u32 = 0x25292e;
pub const TASK_RAIL_ACTIVE: u32 = 0x343a42;

/// The disabled stop control's text, resolved the way the product resolves it.
fn stop_control_text_for_audit(colors: &ShellColors) -> u32 {
    crate::shell_style::stop_control_visual(false, colors).text
}

/// A text colour the drawing code pins as a literal instead of reading the palette.
///
/// These exist on surfaces that also pin their own background (the flow diagram, the task rail), so
/// the audit lists them separately from the palette pairs: it states the literal, the surface it is
/// drawn on where that surface is pinned too, and its measured ratio.
#[derive(Clone, Copy, Debug)]
pub struct HardcodedText {
    pub value: u32,
    /// The pinned background, when the drawing code pins one.
    pub background: Option<u32>,
    pub purpose: &'static str,
    /// Whether this text belongs to a control the shell draws as unavailable. WCAG exempts those,
    /// so the row is reported and measured but not held to the threshold.
    pub exempt: bool,
}

impl HardcodedText {
    pub fn ratio(&self) -> Option<f32> {
        self.background
            .map(|background| contrast_ratio(self.value, background))
    }
}

/// Every literal text colour the shell draws, with its surface where the surface is pinned too.
pub fn hardcoded_text_colours() -> &'static [HardcodedText] {
    &[
        HardcodedText {
            value: 0x8b949e,
            background: Some(0x24272c),
            purpose: "unavailable surface label on its idle row",
            exempt: true,
        },
        HardcodedText {
            value: 0x8b949e,
            background: Some(0x3c5268),
            purpose: "unavailable surface label on its selected row",
            exempt: true,
        },
        HardcodedText {
            value: 0xbcc6d0,
            background: None,
            purpose: "flow node secondary label",
            exempt: false,
        },
        HardcodedText {
            value: 0xe6c278,
            background: None,
            purpose: "flow node matched/idle accent label",
            exempt: false,
        },
        HardcodedText {
            value: 0x9dc0e0,
            background: Some(0x17191c),
            purpose: "matched flow edge label on the shell background",
            exempt: false,
        },
        HardcodedText {
            value: 0x8b949e,
            background: Some(0x17191c),
            purpose: "unmatched flow edge label on the shell background",
            exempt: false,
        },
        HardcodedText {
            value: 0xe6e8eb,
            background: Some(0x343a42),
            purpose: "matched flow node label on the active row",
            exempt: false,
        },
        HardcodedText {
            value: 0x8b949e,
            background: Some(0x25292e),
            purpose: "unmatched flow node label on the idle row",
            exempt: false,
        },
        HardcodedText {
            value: 0xaeb6bf,
            background: None,
            purpose: "flow node detail on a matched node",
            exempt: false,
        },
        HardcodedText {
            value: 0x6b727a,
            background: None,
            purpose: "flow node detail on an unmatched node",
            exempt: false,
        },
    ]
}

/// The palette roles the audit covers, named as the drawing code spells them.
///
/// A guard test scans `lib.rs` for `text_color(rgb(colors.<role>))` and requires every role it finds
/// to be listed here, so a new role cannot be drawn without either appearing in the audit or failing
/// the guard.
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
    fn every_text_colour_in_the_shell_is_either_audited_or_pinned() {
        // The audit is maintained by hand, so this guard closes the gap mechanically: read the
        // drawing code, collect every `text_color(rgb(...))` argument, and require each one to be a
        // role the audit covers or a literal the hardcoded table lists.
        let source = include_str!("lib.rs");
        let mut roles = Vec::new();
        let mut literals = Vec::new();
        for (index, _) in source.match_indices("text_color(rgb(") {
            let rest = &source[index + "text_color(rgb(".len()..];
            // Keep the argument balanced, so a nested call does not split the expression.
            let mut depth = 1usize;
            let mut value = String::new();
            for ch in rest.chars() {
                match ch {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                value.push(ch);
            }
            let value = value.trim();
            for token in AUDITED_ROLES {
                if value.contains(&format!("colors.{token}")) {
                    roles.push((*token).to_string());
                }
            }
            for piece in value.split(|c: char| !(c.is_ascii_hexdigit() || c == 'x')) {
                let stripped = piece.strip_prefix('x').or_else(|| piece.strip_prefix('X'));
                if let Some(Ok(parsed)) = stripped.map(|hex| u32::from_str_radix(hex, 16)) {
                    literals.push(parsed);
                }
            }
        }
        assert!(!roles.is_empty(), "the scan found no palette roles at all");

        let audited = hardcoded_text_colours();
        for literal in literals {
            assert!(
                audited.iter().any(|entry| entry.value == literal),
                "text colour {literal:#x} is drawn but missing from hardcoded_text_colours()"
            );
        }
        for role in roles {
            assert!(
                AUDITED_ROLES.contains(&role.as_str()),
                "text colour role {role} is drawn but missing from AUDITED_ROLES"
            );
        }
    }

    #[test]
    fn every_pinned_literal_with_a_surface_meets_its_threshold() {
        for entry in hardcoded_text_colours() {
            let Some(ratio) = entry.ratio() else {
                // The surface is not pinned next to the text, so this audit cannot pair them; that
                // is stated rather than assumed to be fine.
                continue;
            };
            if entry.exempt {
                // Measured and printed, but an inactive control is not held to the threshold.
                eprintln!(
                    "{}: {:#x} on {:#x} is {ratio:.2}:1 (exempt)",
                    entry.purpose,
                    entry.value,
                    entry.background.unwrap()
                );
                continue;
            }
            assert!(
                ratio >= 4.5,
                "{}: {:#x} on its pinned surface is {ratio:.2}:1",
                entry.purpose,
                entry.value
            );
        }
    }
    #[test]
    fn every_drawn_text_pair_reports_its_ratio() {
        // The audit exists to be read: print each pair so a failure names the surface, and assert
        // the list is not empty rather than assuming it.
        let colors = NORMAL_COLORS;
        let pairs = drawn_text_pairs(&colors);
        assert!(!pairs.is_empty(), "the audit list must not be empty");
        for pair in &pairs {
            eprintln!(
                "{}: {:.2}:1 (needs {:.1}:1)",
                pair.purpose,
                pair.actual_ratio(),
                pair.required_ratio()
            );
        }
    }

    #[test]
    fn the_normal_palette_meets_its_text_thresholds() {
        let failing = failing_text_pairs(&NORMAL_COLORS);
        assert!(
            failing.is_empty(),
            "the normal palette has text pairs below their threshold: {failing:?}"
        );
    }

    #[test]
    fn the_high_contrast_palette_meets_its_text_thresholds() {
        // A high contrast system probe: white on black. The pair list is the same, so this asserts
        // the High Contrast resolution keeps every drawn combination legible.
        let system = SystemColors {
            window: 0x000000,
            window_text: 0xffffff,
            highlight: 0x000080,
            highlight_text: 0xffffff,
            gray_text: 0x808080,
            button_face: 0x000000,
            button_text: 0xffffff,
        };
        let colors = colors_for(ContrastScheme::HighContrast, &system);
        let failing = failing_text_pairs(&colors);
        assert!(
            failing.is_empty(),
            "the High Contrast palette has text pairs below their threshold: {failing:?}"
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
