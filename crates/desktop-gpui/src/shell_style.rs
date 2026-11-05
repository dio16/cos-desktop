//! One-way adapters from the resolved token set to GPUI shell inputs.
//!
//! Every function here takes the injected [`DesignTokens`]; nothing reads a
//! constant, keeps a default or re-derives a token set, so the composition root
//! stays the only resolution point and an override cannot be bypassed by a
//! surface. The adapters are pure: they never touch a window, GPU or texture.
//!
//! Two limits are declared rather than hidden. The Japanese fallback line
//! height token is not applied by [`TextSpec`]: one GPUI text run carries one
//! line height, and mixed-script line height/clipping is outside drawing
//! acceptance until it is verified natively. An unusable platform scale factor
//! is refused by [`separator`] instead of being replaced with a width.

use std::sync::Arc;

use gpui::{
    Div, Font, FontFallbacks, FontFeatures, FontStyle, FontWeight, Pixels, SharedString, Styled, px,
};

use crate::tokens::{DesignTokens, TextStyle, TokenError};

/// GPUI text inputs for one accepted type scale entry.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TextSpec {
    pub size: Pixels,
    pub weight: FontWeight,
    pub line_height: Pixels,
    pub font: Font,
}

impl TextSpec {
    /// The UI text input for one accepted type scale entry.
    ///
    /// The family is the declared Latin UI primary; the ratified fallback order
    /// (`Segoe UI`, then `Yu Gothic UI`, then `Meiryo`) is handed to the
    /// platform shaping layer instead of being resolved or split per glyph here.
    /// The lighter end of the entry's accepted weight range is the default.
    ///
    /// The Japanese fallback line height token is **not** applied: one GPUI text
    /// run carries one line height, so the Latin `line_height_dip` is what the
    /// shell draws with, and the mixed-script fallback line height stays outside
    /// drawing acceptance until mixed-script line height and clipping are
    /// verified natively.
    pub(crate) fn ui(tokens: &DesignTokens, style: TextStyle) -> Self {
        let metrics = tokens.typography.metrics(style);
        let weight = FontWeight(f32::from(metrics.weight_min));
        Self {
            size: px(metrics.size_dip),
            weight,
            line_height: px(metrics.line_height_dip),
            font: Font {
                family: SharedString::from(tokens.typography.latin_ui.as_ref()),
                features: font_features(tokens),
                fallbacks: Some(FontFallbacks(Arc::new(vec![
                    tokens.typography.latin_ui_fallback.to_string(),
                    tokens.typography.japanese_ui.to_string(),
                    tokens.typography.japanese_ui_fallback.to_string(),
                ]))),
                weight,
                style: FontStyle::Normal,
            },
        }
    }

    /// Apply one accepted text input to an element.
    pub(crate) fn apply(self, element: Div) -> Div {
        element
            .font(self.font)
            .text_size(self.size)
            .line_height(self.line_height)
            .font_weight(self.weight)
    }
}

/// The pinned revision's own OpenType feature channel.
///
/// `gpui::FontFeatures` is this revision's tag/value list
/// (`FontFeatures::tag_value_list`), and the pinned Windows backend maps each
/// entry to `DWRITE_FONT_FEATURE` in `gpui_windows::apply_font_features`. The
/// revision has no named `tnum` helper, so the accepted `tabular_numerals`
/// token writes the standard OpenType `tnum` tag through that list with an
/// explicit parameter: `1` enables tabular figures and `0` disables them.
/// `tnum` is not one of the backend's ligature special cases, so the parameter
/// reaches `AddFontFeature` unchanged; the token never leaves the feature
/// unspecified for the font or backend default.
fn font_features(tokens: &DesignTokens) -> FontFeatures {
    let parameter = if tokens.typography.tabular_numerals {
        1
    } else {
        0
    };
    FontFeatures(Arc::new(vec![("tnum".to_string(), parameter)]))
}

/// The task rail width from the resolved token set.
pub(crate) fn rail_width(tokens: &DesignTokens) -> Pixels {
    px(tokens.geometry.task_rail_dip)
}

/// The inspector width from the resolved token set.
pub(crate) fn inspector_width(tokens: &DesignTokens) -> Pixels {
    px(tokens.geometry.inspector_dip)
}

/// The keyboard focus outline width from the resolved token set.
pub(crate) fn focus_outline(tokens: &DesignTokens) -> Pixels {
    px(tokens.geometry.focus_outline_dip)
}

/// The single keyboard-focus ring of the Dock.
///
/// Focus is never colour-only: a focused Dock draws the approved focus-outline
/// width and an unfocused one draws the structural hairline, so the two states
/// stay distinguishable even when the OS gives one colour to every role (a
/// monochrome High Contrast palette). The colour still comes from the resolved
/// contrast scheme, never from a literal here. Fails closed on an unusable scale
/// factor: `Err` means no ring at all rather than a fabricated width.
pub(crate) fn dock_focus_ring(
    tokens: &DesignTokens,
    focused: bool,
    scale_factor: f32,
    colors: &crate::contrast::ShellColors,
) -> Result<(Pixels, u32), TokenError> {
    let width = if focused {
        focus_outline(tokens)
    } else {
        separator(tokens, scale_factor)?
    };
    let color = if focused {
        match crate::contrast::focus_ring(colors) {
            crate::contrast::FocusRing::Visible(color) => color,
            // Every role collapses into the surface: the width still marks focus, but
            // no visible colour is claimed (the token test classifies this case).
            crate::contrast::FocusRing::Invisible => colors.focus,
        }
    } else {
        colors.border
    };
    Ok((width, color))
}

/// Focus representation for a pressable control (the Context Bar's stop controls).
///
/// Focus is guaranteed to change the border **width** — the approved focus outline
/// when focused, the structural hairline when not — in addition to the colour. A High
/// Contrast palette that gives the focus colour and the resting border the same value
/// therefore still shows keyboard focus, so this reuses the same width rule as the
/// Dock ring instead of trusting colour alone. Fails closed on an unusable scale
/// factor: `Err` means no border width rather than a fabricated one.
pub(crate) fn control_focus(
    tokens: &DesignTokens,
    focused: bool,
    scale_factor: f32,
    colors: &crate::contrast::ShellColors,
    background: u32,
    resting: u32,
    extra_candidates: &[u32],
) -> Result<(Pixels, u32), TokenError> {
    // Validate the scale before branching: an unusable scale factor fails closed for
    // the focused state too, instead of returning a fabricated outline width.
    let hairline = separator(tokens, scale_factor)?;
    if focused {
        let color = match crate::contrast::focus_ring_on(colors, background, extra_candidates) {
            crate::contrast::FocusRing::Visible(color) => color,
            crate::contrast::FocusRing::Invisible => colors.focus,
        };
        Ok((focus_outline(tokens), color))
    } else {
        Ok((hairline, resting))
    }
}

/// Structural border of one dock row: one physical pixel, never the focus width.
pub(crate) fn dock_row_border(
    tokens: &DesignTokens,
    scale_factor: f32,
) -> Result<Pixels, TokenError> {
    separator(tokens, scale_factor)
}

/// The states a list row can be in, as far as its appearance is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RowState {
    Normal,
    Selected,
    Focused,
    SelectedAndFocused,
    Disabled,
}

/// Everything the row renderer needs for one state.
///
/// The parts are the ones the shell actually draws: `border` and `text` are the colours, and `cue`
/// is the text drawn beside the row's label (the same strings the accessibility state label uses).
/// Nothing here describes a focus ring: the Dock container draws that from its own focus flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RowVisual {
    pub border: u32,
    pub text: u32,
    /// Non-colour cue drawn beside the row's label. Empty for a plain row.
    pub cue: &'static str,
}

/// The minimum contrast a row's text must keep against the surface it is drawn on.
pub(crate) const ROW_TEXT_MIN_CONTRAST: f32 = 4.5;

/// Choose the appearance of one row state.
///
/// `surface` is the colour the row is actually drawn on, so the returned visual describes what the
/// shell will do rather than what the state model would like:
///
/// - the **text** colour is chosen for contrast against `surface` (a palette that resolves the
///   role's text to the surface itself cannot make the label invisible);
/// - **selection**, **focus** and **disabled** each carry a cue in text, so no colour collapse can
///   merge two of them;
pub(crate) fn row_visual(
    state: RowState,
    colors: &crate::contrast::ShellColors,
    surface: u32,
) -> RowVisual {
    let (border, preferred_text, cue) = match state {
        RowState::Normal => (colors.border, colors.text, ""),
        RowState::Selected => (selection_indicator(colors), colors.selection_text, "閲覧中"),
        RowState::Focused => (colors.focus, colors.text, "キーボード操作対象"),
        RowState::SelectedAndFocused => (
            colors.focus,
            colors.selection_text,
            "閲覧中・キーボード操作対象",
        ),
        RowState::Disabled => (colors.disabled, colors.disabled, "利用不可"),
    };
    // Falling back through the palette in a fixed order keeps the choice deterministic while the
    // contrast check decides: the first candidate that clears the minimum wins, and if none does
    // the best one is used rather than an invisible label.
    let text = [preferred_text, colors.text, colors.background]
        .into_iter()
        .find(|candidate| {
            crate::contrast::contrast_ratio(*candidate, surface) >= ROW_TEXT_MIN_CONTRAST
        })
        .unwrap_or_else(|| {
            [preferred_text, colors.text, colors.background]
                .into_iter()
                .max_by(|left, right| {
                    crate::contrast::contrast_ratio(*left, surface)
                        .partial_cmp(&crate::contrast::contrast_ratio(*right, surface))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .unwrap_or(preferred_text)
        });
    RowVisual { border, text, cue }
}

/// The visual for a candidate dock row.
///
/// The surface is decided **here**, not by the caller, so the drawing code cannot pass a different
/// one than the tests check. A regression in this function fails the contrast test; a caller cannot
/// silently reintroduce a wrong surface because it no longer supplies one.
pub(crate) fn dock_row_visual(state: RowState, colors: &crate::contrast::ShellColors) -> RowVisual {
    row_visual(state, colors, colors.button_face)
}

/// The visual for the stop control, whose surface is the disabled face when it is not enabled.
pub(crate) fn stop_control_visual(
    enabled: bool,
    colors: &crate::contrast::ShellColors,
) -> RowVisual {
    if enabled {
        RowVisual {
            border: colors.focus,
            text: colors.danger_text,
            cue: "",
        }
    } else {
        row_visual(RowState::Disabled, colors, colors.disabled_face)
    }
}

/// The colour the shell draws a selection indicator with.
///
/// `colors.selection` is the theme's own highlight, but a high contrast theme can hand out a
/// highlight that is nearly invisible on its background (the navy `0x000080` on black measures
/// **1.31:1**). A selection indicator has to be seen, so this falls back through the palette in a
/// fixed order and returns the first candidate that clears the **3:1** a meaningful graphic needs.
pub(crate) fn selection_indicator(colors: &crate::contrast::ShellColors) -> u32 {
    crate::contrast::first_visible(
        &[colors.selection, colors.focus, colors.text, colors.disabled],
        colors.background,
    )
}

/// The resting boundary of an interactive control (a list row, a strip row).
///
/// The owner decision of 2026-11-05 keeps it decorative: the control is identified by its label and
/// surface, so WCAG does not require 3:1 from this line. It lives here so the drawing code and the
/// audit read the same value instead of copying it.
pub(crate) const RESTING_CONTROL_BORDER: u32 = 0x343940;

/// The colour of a flow edge and its direction glyph.
///
/// Shared by the drawing code and the contrast audit so the two cannot disagree. The unmatched
/// value was `0x3c4046`, which measures **1.69:1** on the shell background - below the 3:1 a
/// meaningful non-text graphic needs - so it was raised to `0x6b727a` (**3.62:1**).
pub(crate) fn flow_edge_colour(matched: bool) -> u32 {
    if matched { 0x6f9bc6 } else { 0x6b727a }
}

/// The text a row draws for its label, including the state's non-colour cue.
pub(crate) fn dock_row_text(label: &str, visual: &RowVisual) -> String {
    if visual.cue.is_empty() {
        label.to_string()
    } else {
        format!("{label}（{}）", visual.cue)
    }
}

#[cfg(test)]
/// Whether two states can be told apart using only what the shell draws.
///
/// Colour differences count only when the resolved values actually differ, so a palette that
/// collapses two roles cannot be what separates the states: the cue text or the border has to do
/// that work.
pub(crate) fn row_states_differ(
    left: RowState,
    right: RowState,
    colors: &crate::contrast::ShellColors,
    surface: u32,
) -> bool {
    let (a, b) = (
        row_visual(left, colors, surface),
        row_visual(right, colors, surface),
    );
    a.cue != b.cue || a.border != b.border
}

/// One physical pixel of structural separator at `scale_factor`, in DIP.
///
/// Fails closed: an unusable platform scale factor (zero, negative,
/// non-finite) returns the closed [`TokenError`] instead of substituting a
/// width. The render path consumes only `Ok` and draws no separator for that
/// frame, so an invalid scale factor can never render a fabricated hairline.
pub(crate) fn separator(tokens: &DesignTokens, scale_factor: f32) -> Result<Pixels, TokenError> {
    tokens.geometry.separator_dip(scale_factor).map(px)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokens::DesignTokensOverride;

    #[test]
    fn the_single_resolved_set_reaches_every_layout_input() {
        let tokens = DesignTokens::accepted();
        assert_eq!(rail_width(&tokens), px(240.0));
        assert_eq!(inspector_width(&tokens), px(320.0));
        assert_eq!(focus_outline(&tokens), px(2.0));
        assert_eq!(separator(&tokens, 1.0), Ok(px(1.0)));
        assert_eq!(separator(&tokens, 1.25), Ok(px(0.8)));
        assert_eq!(separator(&tokens, 1.5), Ok(px(1.0 / 1.5)));
        assert_eq!(separator(&tokens, 2.0), Ok(px(0.5)));
    }

    #[test]
    fn the_dock_focus_ring_uses_the_approved_token_and_never_moves_layout() {
        let tokens = DesignTokens::accepted();
        let colors = crate::contrast::NORMAL_COLORS;
        let (focused_width, focused_color) =
            dock_focus_ring(&tokens, true, 1.0, &colors).expect("a usable scale");
        let (unfocused_width, unfocused_color) =
            dock_focus_ring(&tokens, false, 1.0, &colors).expect("a usable scale");
        assert_eq!(focused_width, focus_outline(&tokens));
        assert_eq!(focused_width, px(2.0));
        assert!(
            focused_width > unfocused_width,
            "focus must be distinguishable by width even with identical colours"
        );
        assert_ne!(focused_color, unfocused_color);
        assert_eq!(unfocused_color, colors.border);
        // The focus ring must be *visible* on the Dock surface, not merely thicker.
        assert_eq!(
            crate::contrast::focus_ring(&crate::contrast::NORMAL_COLORS),
            crate::contrast::FocusRing::Visible(crate::contrast::NORMAL_COLORS.focus)
        );
        // A close-colour palette falls back to the next role that is visible.
        let mut close = crate::contrast::NORMAL_COLORS;
        close.focus = close.button_face;
        close.selection = close.button_face;
        match crate::contrast::focus_ring(&close) {
            crate::contrast::FocusRing::Visible(color) => {
                assert!(
                    crate::contrast::contrast_ratio(color, close.button_face)
                        >= crate::contrast::FOCUS_RING_MIN_CONTRAST,
                    "the fallback role is visible against the surface"
                );
            }
            crate::contrast::FocusRing::Invisible => {
                panic!("a visible text role exists, the ring must use it")
            }
        }
        // A truly monochrome palette cannot show a colour ring: that case must be
        // classified as invisible, never claimed as visible by width alone.
        let monochrome = crate::contrast::SystemColors {
            window: 0x000000,
            window_text: 0x000000,
            highlight: 0x000000,
            highlight_text: 0x000000,
            gray_text: 0x000000,
            button_face: 0x000000,
            button_text: 0x000000,
        };
        let flat =
            crate::contrast::colors_for(crate::contrast::ContrastScheme::HighContrast, &monochrome);
        assert_eq!(
            crate::contrast::focus_ring(&flat),
            crate::contrast::FocusRing::Invisible
        );
    }

    #[test]
    fn a_same_colour_high_contrast_control_still_shows_focus_by_width() {
        // High Contrast palette where the focus colour and the stop button's resting
        // border are the same OS value (button text), as the review reported.
        let system = crate::contrast::SystemColors {
            window: 0x000000,
            window_text: 0xffffff,
            highlight: 0x0078d7,
            highlight_text: 0xffffff,
            gray_text: 0x6d6d6d,
            button_face: 0x101010,
            button_text: 0xffffff,
        };
        let colors =
            crate::contrast::colors_for(crate::contrast::ContrastScheme::HighContrast, &system);
        let resting = colors.danger_border;
        assert_eq!(
            colors.focus, resting,
            "the review's same-colour focus/resting case"
        );
        let tokens = DesignTokens::accepted();
        // The stop button draws on its own danger face, and danger_text is a valid
        // candidate on that surface.
        let background = colors.danger_face;
        let (focused_width, focused_color) = control_focus(
            &tokens,
            true,
            1.0,
            &colors,
            background,
            resting,
            &[colors.danger_text],
        )
        .expect("a usable scale");
        let (resting_width, resting_color) =
            control_focus(&tokens, false, 1.0, &colors, background, resting, &[])
                .expect("a usable scale");
        assert!(
            focused_width > resting_width,
            "focus changes the border width even when the colour is identical"
        );
        assert!(
            crate::contrast::contrast_ratio(focused_color, background)
                >= crate::contrast::FOCUS_RING_MIN_CONTRAST,
            "the focus border is visible against the button's own background"
        );
        assert_eq!(resting_color, resting);
        assert_eq!(focused_width, focus_outline(&tokens));

        // The review's palette: button_face black, button_text white and highlight
        // white, so danger_face is white; the ring must fall back to danger_text.
        let white_on_white = crate::contrast::SystemColors {
            window: 0x000000,
            window_text: 0xffffff,
            highlight: 0xffffff,
            highlight_text: 0x000000,
            gray_text: 0x101010,
            button_face: 0x000000,
            button_text: 0xffffff,
        };
        let palette = crate::contrast::colors_for(
            crate::contrast::ContrastScheme::HighContrast,
            &white_on_white,
        );
        let (_, ring_color) = control_focus(
            &tokens,
            true,
            1.0,
            &palette,
            palette.danger_face,
            palette.danger_border,
            &[palette.danger_text],
        )
        .expect("a usable scale");
        assert!(
            crate::contrast::contrast_ratio(ring_color, palette.danger_face)
                >= crate::contrast::FOCUS_RING_MIN_CONTRAST,
            "a white danger face must not get a white ring"
        );

        // An unusable scale factor fails closed for the focused state too.
        assert!(
            control_focus(
                &tokens,
                true,
                0.0,
                &palette,
                palette.danger_face,
                palette.danger_border,
                &[]
            )
            .is_err()
        );
    }

    #[test]
    fn a_focus_outline_override_reaches_the_dock_ring() {
        let tokens = DesignTokensOverride {
            focus_outline_dip: Some(3.0),
            ..DesignTokensOverride::default()
        }
        .resolve_accepted()
        .expect("a valid override resolves");
        let colors = crate::contrast::NORMAL_COLORS;
        let focused = dock_focus_ring(&tokens, true, 1.0, &colors).expect("a usable scale");
        let unfocused = dock_focus_ring(&tokens, false, 1.0, &colors).expect("a usable scale");
        assert_eq!(focused.0, px(3.0));
        assert!(
            unfocused.0 < focused.0,
            "the unfocused ring stays a hairline"
        );
    }

    #[test]
    fn dock_rows_keep_a_one_physical_pixel_structural_border() {
        let tokens = DesignTokens::accepted();
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let width = dock_row_border(&tokens, scale).expect("a usable scale factor");
            assert_eq!(
                width * scale,
                px(1.0),
                "a structural row border is one physical pixel at {scale}x"
            );
            assert_ne!(
                width,
                focus_outline(&tokens),
                "a row border is structural, not the focus indicator"
            );
        }
    }

    #[test]
    fn a_disabled_scale_factor_draws_no_row_border() {
        let tokens = DesignTokens::accepted();
        assert!(dock_row_border(&tokens, 0.0).is_err());
    }

    #[test]
    fn row_colour_expresses_selection_and_never_focus() {
        let colors = crate::contrast::NORMAL_COLORS;
        assert_eq!(
            row_visual(RowState::Selected, &colors, colors.background).border,
            colors.selection
        );
        assert_eq!(
            row_visual(RowState::Normal, &colors, colors.background).border,
            colors.border
        );
        assert_ne!(
            row_visual(RowState::Selected, &colors, colors.background).border,
            crate::contrast::colors_for(
                crate::contrast::ContrastScheme::HighContrast,
                &crate::contrast::SystemColors {
                    window: 0,
                    window_text: 0xffffff,
                    highlight: 0x1aebff,
                    highlight_text: 0,
                    gray_text: 0x8c8c8c,
                    button_face: 0x202020,
                    button_text: 0xffff00,
                }
            )
            .focus,
            "selection and focus are distinct in both schemes"
        );
    }

    #[test]
    fn an_override_reaches_the_layout_inputs_without_a_second_source() {
        let tokens = DesignTokensOverride {
            task_rail_dip: Some(300.0),
            inspector_dip: Some(400.0),
            focus_outline_dip: Some(3.0),
            separator_physical_pixels: Some(2.0),
            ..DesignTokensOverride::default()
        }
        .resolve_accepted()
        .expect("a valid override resolves");
        assert_eq!(rail_width(&tokens), px(300.0));
        assert_eq!(inspector_width(&tokens), px(400.0));
        assert_eq!(focus_outline(&tokens), px(3.0));
        assert_eq!(separator(&tokens, 2.0), Ok(px(1.0)));
        // An unaffected surface keeps its accepted value: nothing re-derives.
        assert_eq!(tokens.geometry.thumbnail_dip, 72.0);
    }

    #[test]
    fn text_specs_carry_the_ratified_size_weight_line_height_and_family() {
        let tokens = DesignTokens::accepted();
        let entries: [(TextStyle, f32, u16, f32); 7] = [
            (TextStyle::Xs, 12.0, 400, 16.0),
            (TextStyle::Sm, 13.0, 400, 18.0),
            (TextStyle::Body, 14.0, 400, 20.0),
            (TextStyle::BodyStrong, 14.0, 600, 20.0),
            (TextStyle::Control, 16.0, 400, 24.0),
            (TextStyle::Title, 20.0, 600, 28.0),
            (TextStyle::Display, 24.0, 600, 32.0),
        ];
        for (style, size, weight, line_height) in entries {
            let spec = TextSpec::ui(&tokens, style);
            assert_eq!(spec.size, px(size), "{}", style.name());
            assert_eq!(
                spec.weight,
                FontWeight(f32::from(weight)),
                "{}",
                style.name()
            );
            assert_eq!(spec.line_height, px(line_height), "{}", style.name());
            assert_eq!(spec.font.family.as_ref(), "Segoe UI Variable");
            assert_eq!(spec.font.weight, FontWeight(f32::from(weight)));
            assert_eq!(spec.font.style, FontStyle::Normal);
            let fallbacks = spec
                .font
                .fallbacks
                .as_ref()
                .expect("the ratified fallback order is declared");
            assert_eq!(
                fallbacks.0.as_slice(),
                ["Segoe UI", "Yu Gothic UI", "Meiryo"],
                "{}",
                style.name()
            );
        }
    }

    #[test]
    fn a_type_override_reaches_the_text_inputs() {
        let tokens = DesignTokensOverride {
            body_size_dip: Some(15.0),
            body_line_height_dip: Some(21.0),
            ..DesignTokensOverride::default()
        }
        .resolve_accepted()
        .expect("a valid override resolves");
        let spec = TextSpec::ui(&tokens, TextStyle::Body);
        assert_eq!(spec.size, px(15.0));
        assert_eq!(spec.line_height, px(21.0));
        // Other entries keep the accepted values.
        assert_eq!(TextSpec::ui(&tokens, TextStyle::Xs).size, px(12.0));
    }

    #[test]
    fn the_tabular_numerals_token_explicitly_enables_or_disables_the_feature() {
        let accepted = DesignTokens::accepted();
        assert!(accepted.typography.tabular_numerals);
        // The enabled token writes an explicit OpenType enable (value 1)
        // through the pinned revision's own tag/value list.
        for style in TextStyle::ALL {
            let spec = TextSpec::ui(&accepted, style);
            assert_eq!(
                spec.font.features.tag_value_list(),
                [("tnum".to_string(), 1)],
                "{}",
                style.name()
            );
        }
        // Disabling the token writes an explicit OpenType disable (value 0),
        // so the feature is decided by the token rather than by the font or
        // backend default; the pinned backend passes the parameter through.
        let disabled = DesignTokensOverride {
            tabular_numerals: Some(false),
            ..DesignTokensOverride::default()
        }
        .resolve_accepted()
        .expect("a valid override resolves");
        let spec = TextSpec::ui(&disabled, TextStyle::Body);
        assert_eq!(
            spec.font.features.tag_value_list(),
            [("tnum".to_string(), 0)]
        );
        assert_ne!(spec.font.features, FontFeatures::default());
    }

    #[test]
    fn an_unusable_scale_factor_produces_no_rendered_width() {
        let tokens = DesignTokens::accepted();
        for scale_factor in [0.0_f32, -1.0, f32::NAN, f32::INFINITY] {
            assert!(
                matches!(
                    separator(&tokens, scale_factor),
                    Err(TokenError::NonPositiveScaleFactor { .. })
                ),
                "{scale_factor}"
            );
            // The render path consumes `Ok` only: an error yields no width,
            // never a substituted one.
            assert_eq!(
                separator(&tokens, scale_factor).ok(),
                None,
                "{scale_factor}"
            );
        }
        assert_eq!(separator(&tokens, 1.25).ok(), Some(px(0.8)));
    }

    fn monochrome_high_contrast() -> crate::contrast::ShellColors {
        let monochrome = crate::contrast::SystemColors {
            window: 0x000000,
            window_text: 0xffffff,
            highlight: 0xffffff,
            highlight_text: 0xffffff,
            gray_text: 0x000000,
            button_face: 0xffffff,
            button_text: 0xffffff,
        };
        crate::contrast::colors_for(crate::contrast::ContrastScheme::HighContrast, &monochrome)
    }

    #[test]
    fn a_collapsing_palette_still_leaves_every_row_label_readable() {
        // The surfaces the product actually draws on: dock rows sit on the raised face and a
        // disabled stop button on the disabled face. Testing against `background` would let the
        // product regress to a wrong surface without any test noticing.
        let colors = monochrome_high_contrast();
        // The same entry points the drawing code calls, so a wrong surface inside them fails here.
        for (label, surface) in [
            ("dock row", colors.button_face),
            ("disabled stop", colors.disabled_face),
        ] {
            for state in [
                RowState::Normal,
                RowState::Selected,
                RowState::Focused,
                RowState::SelectedAndFocused,
                RowState::Disabled,
            ] {
                let visual = if label == "dock row" {
                    dock_row_visual(state, &colors)
                } else {
                    stop_control_visual(false, &colors)
                };
                let ratio = crate::contrast::contrast_ratio(visual.text, surface);
                assert!(
                    ratio >= ROW_TEXT_MIN_CONTRAST,
                    "{label} {state:?}: text contrast is {ratio:.2}:1 against its real surface"
                );
            }
        }
    }

    #[test]
    fn every_pair_of_states_is_separable_under_a_collapsing_palette() {
        use RowState::*;
        let colors = monochrome_high_contrast();
        let states = [Normal, Selected, Focused, SelectedAndFocused, Disabled];
        for (index, left) in states.iter().enumerate() {
            for right in &states[index + 1..] {
                assert!(
                    row_states_differ(*left, *right, &colors, colors.background),
                    "{left:?} and {right:?} are indistinguishable under this palette"
                );
            }
        }
    }

    #[test]
    fn the_cue_is_drawn_beside_the_label() {
        let colors = crate::contrast::NORMAL_COLORS;
        let surface = colors.background;
        assert_eq!(
            dock_row_text("候補", &row_visual(RowState::Selected, &colors, surface)),
            "候補（閲覧中）"
        );
        assert_eq!(
            dock_row_text("候補", &row_visual(RowState::Focused, &colors, surface)),
            "候補（キーボード操作対象）"
        );
        assert_eq!(
            dock_row_text(
                "候補",
                &row_visual(RowState::SelectedAndFocused, &colors, surface)
            ),
            "候補（閲覧中・キーボード操作対象）"
        );
        assert_eq!(
            dock_row_text("停止", &row_visual(RowState::Disabled, &colors, surface)),
            "停止（利用不可）"
        );
        // A plain row draws its label unchanged, so the cue is a real difference and not noise.
        assert_eq!(
            dock_row_text("候補", &row_visual(RowState::Normal, &colors, surface)),
            "候補"
        );
    }
}
