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
    /// The ratio this entry must meet: 4.5:1 for body text, 3:1 for a meaningful non-text graphic.
    pub required: f32,
}

impl HardcodedText {
    pub fn ratio(&self) -> Option<f32> {
        self.background
            .map(|background| contrast_ratio(self.value, background))
    }
}

/// Every literal text colour the shell draws, with its surface where the surface is pinned too.
pub fn hardcoded_text_colours() -> Vec<HardcodedText> {
    vec![
        HardcodedText {
            value: 0x8b949e,
            background: Some(0x24272c),
            purpose: "unavailable surface label on its idle row",
            exempt: true,
            required: 4.5,
        },
        HardcodedText {
            value: 0x8b949e,
            background: Some(0x3c5268),
            purpose: "unavailable surface label on its selected row",
            exempt: true,
            required: 4.5,
        },
        HardcodedText {
            value: 0xbcc6d0,
            background: Some(0x1e2124),
            purpose: "flow node secondary label on the idle background",
            exempt: false,
            required: 4.5,
        },
        HardcodedText {
            value: 0xe6c278,
            background: Some(0x17191c),
            purpose: "work-surface notice on the shell background",
            exempt: false,
            required: 4.5,
        },
        HardcodedText {
            value: 0xe6c278,
            background: Some(0x3a4048),
            purpose: "flow accent label on the active node",
            exempt: false,
            required: 4.5,
        },
        HardcodedText {
            value: 0xe6c278,
            background: Some(0x272b31),
            purpose: "flow accent label on the matched node",
            exempt: false,
            required: 4.5,
        },
        HardcodedText {
            // Read from the entry point the drawing code uses, so a regression there fails here.
            value: crate::shell_style::flow_edge_colour(true),
            background: Some(0x17191c),
            purpose: "matched flow edge and direction glyph",
            exempt: false,
            // A direction glyph carries meaning but is not text: 3:1 is the applicable threshold.
            required: 3.0,
        },
        HardcodedText {
            value: crate::shell_style::flow_edge_colour(false),
            background: Some(0x17191c),
            purpose: "unmatched flow edge and direction glyph",
            exempt: false,
            // A direction glyph carries meaning but is not text: 3:1 is the applicable threshold.
            required: 3.0,
        },
        HardcodedText {
            value: 0x969ea8,
            background: Some(0x2f3338),
            purpose: "palette unavailable reason on the active row",
            exempt: false,
            required: 4.5,
        },
        HardcodedText {
            value: 0x969ea8,
            background: Some(0x24272c),
            purpose: "palette unavailable reason on the idle row",
            exempt: false,
            required: 4.5,
        },
        HardcodedText {
            value: 0x8b949e,
            background: Some(0x1c1f23),
            purpose: "palette hint line on the palette panel",
            exempt: false,
            required: 4.5,
        },
        HardcodedText {
            value: 0x9da3aa,
            background: Some(0x202328),
            purpose: "intervention detail on the row surface",
            exempt: false,
            required: 4.5,
        },
        HardcodedText {
            value: 0x9da3aa,
            background: Some(0x2a2f36),
            purpose: "intervention detail on the notification surface",
            exempt: false,
            required: 4.5,
        },
        HardcodedText {
            value: 0xbcc6d0,
            background: Some(0x202328),
            purpose: "intervention next-action on the row surface",
            exempt: false,
            required: 4.5,
        },
        HardcodedText {
            value: 0xbcc6d0,
            background: Some(0x2a2f36),
            purpose: "intervention next-action on the notification surface",
            exempt: false,
            required: 4.5,
        },
        HardcodedText {
            value: 0xd08a6a,
            // The reason text sits in the inspector panel, whose surface is the raised face set in
            // lib.rs. It *explains* an unavailable section rather than being part of a disabled
            // control, so it is ordinary information and is held to the threshold: 5.65:1.
            background: Some(0x202328),
            purpose: "inspector unavailable-section reason",
            exempt: false,
            required: 4.5,
        },
        HardcodedText {
            value: 0xe6e8eb,
            background: Some(0x24272c),
            purpose: "tooltip text on the tooltip surface",
            exempt: false,
            required: 4.5,
        },
        HardcodedText {
            value: 0x9dc0e0,
            background: Some(0x17191c),
            purpose: "matched flow edge label on the shell background",
            exempt: false,
            required: 4.5,
        },
        HardcodedText {
            value: 0x8b949e,
            background: Some(0x17191c),
            purpose: "unmatched flow edge label on the shell background",
            exempt: false,
            required: 4.5,
        },
        HardcodedText {
            value: 0xe6e8eb,
            background: Some(0x343a42),
            purpose: "matched flow node label on the active row",
            exempt: false,
            required: 4.5,
        },
        HardcodedText {
            value: 0xaeb6bf,
            background: Some(0x1e2124),
            purpose: "unmatched flow node heading on its idle background",
            exempt: false,
            required: 4.5,
        },
        HardcodedText {
            value: 0xaeb6bf,
            background: Some(0x3a4048),
            purpose: "unmatched flow node heading on its active background",
            exempt: false,
            required: 4.5,
        },
        HardcodedText {
            value: 0xbcc6d0,
            background: Some(0x3a4048),
            purpose: "flow node detail on the active node",
            exempt: false,
            required: 4.5,
        },
        HardcodedText {
            value: 0xbcc6d0,
            background: Some(0x272b31),
            purpose: "flow node detail on the matched node",
            exempt: false,
            required: 4.5,
        },
        HardcodedText {
            value: 0xaeb6bf,
            background: Some(0x1e2124),
            purpose: "flow node detail on an unmatched idle node",
            exempt: false,
            required: 4.5,
        },
        HardcodedText {
            value: 0xaeb6bf,
            background: Some(0x3a4048),
            purpose: "flow node detail on an unmatched active node",
            exempt: false,
            required: 4.5,
        },
        HardcodedText {
            value: 0xbcc6d0,
            background: Some(0x272b31),
            purpose: "flow node detail on a matched node",
            exempt: false,
            required: 4.5,
        },
    ]
}

/// Values the drawing code passes to `text_color` that this audit trusts because the code that
/// produces them is audited: the variable name, and where it comes from.
pub const DERIVED_TEXT_VARIABLES: &[(&str, &str)] = &[
    ("row_visual.text", "RowVisual from dock_row_visual"),
    ("color", "shell_style::flow_edge_colour"),
    (
        "shell_style::stop_control_visual(false, &colors).text",
        "the disabled stop control's resolved text",
    ),
    (
        "shell_style::stop_control_visual(false, &colors).text",
        "the disabled stop control's resolved text",
    ),
];

/// Keywords and condition identifiers that may appear **around** a colour expression.
///
/// The guard fails closed: an identifier outside the braces that is not listed here is rejected, so
/// a new condition has to be declared before its expression is accepted. That is deliberate - it
/// cannot silently grow a hole the way a permissive scan would.
pub const EXPRESSION_KEYWORDS: &[&str] = &["if", "else", "match", "true", "false"];

/// Condition components the drawing code really uses around a colour choice.
pub const CONDITION_IDENTIFIERS: &[&str] = &[
    "node",
    "edge",
    "matched",
    "is_active",
    "surface",
    "item",
    "selected",
    "active",
    "focused",
    "compared",
    "formal",
    "group_start",
    "reconcile_enabled",
    "stop_enabled",
    "retryable",
    "retryable_admission",
    "compare_unavailable",
    "index",
    "unavailable",
    "is_some",
    "none",
];

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
            purpose: "selection indicator on the shell background",
            indicator: colors.selection,
            surface: colors.background,
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
    ("raised surface", 0x343a42),
    ("button face", 0x202328),
];

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

    /// How one `text_color(rgb(...))` argument is accounted for.
    #[derive(Debug, PartialEq, Eq)]
    enum ColourSource {
        /// A palette role, named as the drawing code spells it.
        Role(String),
        /// A hex literal, as written.
        Literal(u32),
        /// A value derived elsewhere (for example a resolved `RowVisual`), which the audit trusts
        /// because the type it comes from is audited.
        Derived,
        /// Anything the audit does not know how to classify.
        Unknown(String),
    }

    fn classify_argument(argument: &str) -> Vec<ColourSource> {
        // Every branch is classified on its own: a known value in one arm must not excuse an
        // unknown one in another. The branches are the `{ ... }` blocks of an `if`/`match`
        // expression, or the whole argument when there are none.
        let mut branches: Vec<String> = Vec::new();
        let mut depth = 0usize;
        let mut current = String::new();
        let argument: String = argument
            .lines()
            .map(|line| match line.find("//") {
                Some(index) => &line[..index],
                None => line,
            })
            .collect::<Vec<_>>()
            .join(
                "
",
            );
        let argument = argument.as_str();
        for ch in argument.chars() {
            match ch {
                '{' => {
                    depth += 1;
                    if depth > 1 {
                        current.push(ch);
                    }
                }
                '}' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        branches.push(current.trim().to_string());
                        current.clear();
                    } else {
                        current.push(ch);
                    }
                }
                _ if depth > 0 => current.push(ch),
                _ => {}
            }
        }
        // With no braces there is no structure around a value: the whole argument is the value.
        if branches.is_empty() {
            return vec![classify_branch(argument)];
        }

        // Everything outside the braces is structure (`if`, `else`, `match`, the condition
        // expression and its operators). A colour must never appear there: `(if c { a } else { b })
        // ^ 0xc0ffee` combines the branches with a literal the branch scan would otherwise miss.
        let mut outside = String::new();
        let mut depth = 0usize;
        for ch in argument.chars() {
            match ch {
                '{' => depth += 1,
                '}' => depth = depth.saturating_sub(1),
                _ if depth == 0 => outside.push(ch),
                _ => {}
            }
        }
        if outside.contains("colors.") || has_hex_literal(&outside) {
            return vec![ColourSource::Unknown(outside.trim().to_string())];
        }
        // A colour expression is one value: an `if`/`else` chain, optionally compared in its
        // condition. Combining two branch expressions with an operator produces a colour the audit
        // never sees - `(if c { a } else { b }) ^ (if c { a } else { b })` is `0x000000` - so any
        // value-level operator outside the braces is rejected. Comparison and boolean operators are
        // allowed, because conditions need them.
        let mut structural = outside.clone();
        for allowed in ["==", "!=", "<=", ">=", "&&", "||"] {
            structural = structural.replace(allowed, " ");
        }
        // `&`, `|` and `!` are value operators here, not conditions: `a & b` and `!a` over colour
        // expressions produce colours the audit never sees. Only `&&`, `||` and `!=` (which are
        // removed above) are structural.
        if structural.chars().any(|ch| {
            matches!(
                ch,
                '^' | '+' | '-' | '*' | '/' | '%' | '|' | '&' | '!' | '<' | '>'
            )
        }) {
            return vec![ColourSource::Unknown(outside.trim().to_string())];
        }
        // Only structure may surround the branches: keywords, declared condition identifiers,
        // operators and punctuation. Anything else - a bare variable holding a colour, or an
        // operator combining the branches with one - is rejected.
        for token in outside.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_')) {
            if token.is_empty() {
                continue;
            }
            // A numeric literal outside the branches is a colour the audit cannot see:
            // `(if c { a } else { b }) ^ 1` is a different colour, not structure.
            if token.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
                return vec![ColourSource::Unknown(outside.trim().to_string())];
            }
            if EXPRESSION_KEYWORDS.contains(&token) || CONDITION_IDENTIFIERS.contains(&token) {
                continue;
            }
            return vec![ColourSource::Unknown(outside.trim().to_string())];
        }

        let mut sources = Vec::new();
        for branch in branches {
            sources.push(classify_branch(&branch));
        }
        sources
    }

    /// Whether the text contains a `0x…` literal anywhere, which outside a branch is a colour the
    /// audit would otherwise not see.
    fn has_hex_literal(text: &str) -> bool {
        let bytes = text.as_bytes();
        let mut index = 0usize;
        while index + 2 < bytes.len() {
            if bytes[index] == b'0' && (bytes[index + 1] | 32) == b'x' {
                let mut end = index + 2;
                while end < bytes.len() && (bytes[end] as char).is_ascii_hexdigit() {
                    end += 1;
                }
                if end > index + 2 {
                    return true;
                }
            }
            index += 1;
        }
        false
    }

    /// Classify one branch: it must be exactly a role, a literal, or a declared derived value.
    fn classify_branch(branch: &str) -> ColourSource {
        let trimmed = branch.trim();
        if let Some(rest) = trimmed.strip_prefix("colors.") {
            let name: String = rest
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
                .collect();
            if name.len() == rest.len() && !name.is_empty() {
                return ColourSource::Role(name);
            }
            return ColourSource::Unknown(trimmed.to_string());
        }
        if let Some(hex) = trimmed
            .strip_prefix("0x")
            .or_else(|| trimmed.strip_prefix("0X"))
        {
            let is_hex = !hex.is_empty() && hex.chars().all(|ch| ch.is_ascii_hexdigit());
            if let Some(Ok(value)) = is_hex.then(|| u32::from_str_radix(hex, 16)) {
                return ColourSource::Literal(value);
            }
            return ColourSource::Unknown(trimmed.to_string());
        }
        if DERIVED_TEXT_VARIABLES
            .iter()
            .any(|(name, _)| trimmed == *name)
        {
            return ColourSource::Derived;
        }
        // `if`/`else` keywords and their condition are not branches themselves, but an argument with
        // no braces and no known value lands here, and must not pass.
        ColourSource::Unknown(trimmed.to_string())
    }

    fn text_colour_arguments(source: &str) -> Vec<String> {
        let mut arguments = Vec::new();
        for (index, _) in source.match_indices("text_color(rgb(") {
            let rest = &source[index + "text_color(rgb(".len()..];
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
            arguments.push(value.trim().to_string());
        }
        arguments
    }

    #[test]
    fn the_classifier_separates_audited_from_unknown_colours() {
        // Positive controls: what the drawing code really writes.
        assert_eq!(
            classify_argument("colors.text"),
            vec![ColourSource::Role("text".to_string())]
        );
        assert_eq!(
            classify_argument("0xe6e8eb"),
            vec![ColourSource::Literal(0xe6e8eb)]
        );
        assert_eq!(
            classify_argument("if node.matched { 0xe6e8eb } else { 0xaeb6bf }"),
            vec![
                ColourSource::Literal(0xe6e8eb),
                ColourSource::Literal(0xaeb6bf)
            ]
        );
        assert_eq!(
            classify_argument("row_visual.text"),
            vec![ColourSource::Derived]
        );

        // Negative controls: the guard has to reject these, which is what the previous version
        // could not do.
        assert_eq!(
            classify_argument("colors.disabled_border"),
            vec![ColourSource::Role("disabled_border".to_string())]
        );
        assert_eq!(
            classify_argument("0x123456"),
            vec![ColourSource::Literal(0x123456)]
        );
        assert_eq!(
            classify_argument("some_other_colour"),
            vec![ColourSource::Unknown("some_other_colour".to_string())]
        );
        // A colour outside the braces must be rejected even though every branch is audited.
        match classify_argument("(if node.matched { 0xbcc6d0 } else { 0xaeb6bf }) ^ 0xc0ffee")
            .as_slice()
        {
            [ColourSource::Unknown(_)] => {}
            other => panic!("a literal outside the branches must be rejected, got {other:?}"),
        }
        // A role inside a branch is a legitimate audited value.
        assert_eq!(
            classify_argument("(if node.matched { 0xbcc6d0 } else { colors.text })"),
            vec![
                ColourSource::Literal(0xbcc6d0),
                ColourSource::Role("text".to_string())
            ]
        );
        // A bare variable combined with the branches is not a colour the audit can see.
        match classify_argument(
            "(if node.matched { 0xbcc6d0 } else { 0xaeb6bf }) ^ unregistered_color",
        )
        .as_slice()
        {
            [ColourSource::Unknown(_)] => {}
            other => {
                panic!("an unknown variable outside the branches must be rejected, got {other:?}")
            }
        }
        // Two branch expressions combined by an operator produce a colour the audit never sees.
        match classify_argument(
            "(if node.matched { 0xbcc6d0 } else { 0xaeb6bf }) ^ (if node.matched { 0xbcc6d0 } else { 0xaeb6bf })",
        )
        .as_slice()
        {
            [ColourSource::Unknown(_)] => {}
            other => panic!("combining two branch expressions must be rejected, got {other:?}"),
        }
        // Bitwise AND and logical NOT over branch expressions are colour operations too.
        match classify_argument(
            "(if node.matched { 0xbcc6d0 } else { 0xaeb6bf }) & (if node.matched { 0xaeb6bf } else { 0xbcc6d0 })",
        )
        .as_slice()
        {
            [ColourSource::Unknown(_)] => {}
            other => panic!("a bitwise AND of two branch expressions must be rejected, got {other:?}"),
        }
        // `!(if c { a } else { b })` is equally a colour operation.
        match classify_argument("!(if node.matched { 0xbcc6d0 } else { 0xaeb6bf })").as_slice() {
            [ColourSource::Unknown(_)] => {}
            other => {
                panic!("a logical NOT over a branch expression must be rejected, got {other:?}")
            }
        }
        // A comparison in the condition is structure, not a colour operation.
        assert!(
            classify_argument("if surface == item { 0x3c5268 } else { 0x24272c }")
                .iter()
                .all(|source| matches!(source, ColourSource::Literal(_))),
            "a condition comparison must stay acceptable"
        );
        // A decimal literal outside the branches shifts the colour and must not pass.
        match classify_argument("(if node.matched { 0xbcc6d0 } else { 0xaeb6bf }) ^ 1").as_slice() {
            [ColourSource::Unknown(_)] => {}
            other => {
                panic!("a numeric literal outside the branches must be rejected, got {other:?}")
            }
        }
        // A role *outside* the branches is not.
        match classify_argument("(if node.matched { 0xbcc6d0 } else { 0xaeb6bf }) ^ colors.border")
            .as_slice()
        {
            [ColourSource::Unknown(_)] => {}
            other => panic!("a role outside the branches must be rejected, got {other:?}"),
        }
    }

    #[test]
    fn every_text_colour_in_the_shell_is_either_audited_or_pinned() {
        // Read the drawing code and require every argument to be classified, and every classified
        // colour to be one the audit accounts for. This is what makes the hand-maintained list
        // self-checking: an unknown role, an unknown literal or an unclassifiable expression fails.
        // Every shell source file that draws text, so the guard is not limited to lib.rs.
        const SOURCES: &[(&str, &str)] = &[
            ("lib.rs", include_str!("lib.rs")),
            ("palette.rs", include_str!("palette.rs")),
            ("intervention.rs", include_str!("intervention.rs")),
            ("inspector.rs", include_str!("inspector.rs")),
            ("help.rs", include_str!("help.rs")),
        ];
        let mut arguments = Vec::new();
        for (name, source) in SOURCES {
            let found = text_colour_arguments(source);
            // Every listed file draws text today; a file that stops yielding sites means the scan
            // broke, not that the file became exempt.
            assert!(
                !found.is_empty(),
                "{name} is listed as a drawing file but the scan found nothing"
            );
            arguments.extend(found);
        }
        assert!(
            arguments.len() >= 20,
            "the scan found only {} arguments, so it is not reading the drawing code",
            arguments.len()
        );

        let audited = hardcoded_text_colours();
        for argument in &arguments {
            for source in classify_argument(argument) {
                match source {
                    ColourSource::Role(name) => assert!(
                        AUDITED_ROLES.contains(&name.as_str()),
                        "role colors.{name} is drawn but not in AUDITED_ROLES"
                    ),
                    ColourSource::Literal(value) => assert!(
                        audited.iter().any(|entry| entry.value == value),
                        "text colour {value:#x} is drawn but missing from hardcoded_text_colours()"
                    ),
                    ColourSource::Derived => {}
                    ColourSource::Unknown(text) => {
                        panic!(
                            "text colour `{text}` is neither a role, a literal nor a known derived value"
                        )
                    }
                }
            }
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
                ratio >= entry.required,
                "{}: {:#x} on its pinned surface is {ratio:.2}:1 (needs {:.1}:1)",
                entry.purpose,
                entry.value,
                entry.required
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
