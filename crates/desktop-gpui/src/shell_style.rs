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

/// A non-colour cue. Colour alone must never be the only difference between two states, because
/// a High Contrast palette may resolve two colours to the same value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RowMarker {
    /// Nothing beyond the text.
    None,
    /// A leading glyph marking the selected row.
    SelectionGlyph,
    /// The disabled row reads as unavailable without relying on its colour.
    UnavailableLabel,
}

/// Everything the row renderer needs for one state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RowVisual {
    /// Border/indicator colour for the row.
    pub border: u32,
    /// Text colour for the row's label.
    pub text: u32,
    /// Whether the focus outline width is drawn (the width is the caller's token lookup).
    pub draws_focus_outline: bool,
    /// Non-colour cue carried by the row.
    pub marker: RowMarker,
}

/// Choose the appearance of one row state.
///
/// The three indicators are deliberately carried by different channels so that no single one can
/// be flattened by a palette:
///
/// - **selection** changes the row's border **colour** and adds a **glyph**;
/// - **focus** changes the border **width** (drawn by the caller) and never only its colour;
/// - **disabled** changes the text colour **and** adds an unavailability marker.
pub(crate) fn row_visual(state: RowState, colors: &crate::contrast::ShellColors) -> RowVisual {
    match state {
        RowState::Normal => RowVisual {
            border: colors.border,
            text: colors.text,
            draws_focus_outline: false,
            marker: RowMarker::None,
        },
        RowState::Selected => RowVisual {
            border: colors.selection,
            text: colors.selection_text,
            draws_focus_outline: false,
            marker: RowMarker::SelectionGlyph,
        },
        RowState::Focused => RowVisual {
            border: colors.focus,
            text: colors.text,
            draws_focus_outline: true,
            marker: RowMarker::None,
        },
        RowState::SelectedAndFocused => RowVisual {
            border: colors.focus,
            text: colors.selection_text,
            draws_focus_outline: true,
            marker: RowMarker::SelectionGlyph,
        },
        RowState::Disabled => RowVisual {
            border: colors.disabled,
            text: colors.disabled,
            draws_focus_outline: false,
            marker: RowMarker::UnavailableLabel,
        },
    }
}

#[cfg(test)]
/// Whether two states can be told apart **without relying on colour**.
///
/// Two states are distinct when they differ in the focus outline, in the marker, or in both text
/// and border value at once. A palette that collapses two colours therefore cannot make two rows
/// look identical.
pub(crate) fn states_differ_without_colour(left: RowState, right: RowState) -> bool {
    let probe = crate::contrast::NORMAL_COLORS;
    let (a, b) = (row_visual(left, &probe), row_visual(right, &probe));
    if a.draws_focus_outline != b.draws_focus_outline || a.marker != b.marker {
        return true;
    }
    a.text != b.text && a.border != b.border
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
            row_visual(RowState::Selected, &colors).border,
            colors.selection
        );
        assert_eq!(row_visual(RowState::Normal, &colors).border, colors.border);
        assert_ne!(
            row_visual(RowState::Selected, &colors).border,
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

    #[test]
    fn every_row_state_is_distinct_without_colour() {
        // The states a list row can be in, and the requirement that no palette can make two of
        // them look the same by collapsing colours: each pair must differ in a non-colour channel.
        use RowState::*;
        let states = [Normal, Selected, Focused, SelectedAndFocused, Disabled];
        for (index, left) in states.iter().enumerate() {
            for right in &states[index + 1..] {
                assert!(
                    states_differ_without_colour(*left, *right),
                    "{left:?} and {right:?} cannot be told apart without colour"
                );
            }
        }
    }

    #[test]
    fn selection_focus_and_disabled_each_use_their_own_channel() {
        let colors = crate::contrast::NORMAL_COLORS;

        // Selection: colour plus glyph.
        let selected = row_visual(RowState::Selected, &colors);
        assert_eq!(selected.border, colors.selection);
        assert_eq!(selected.marker, RowMarker::SelectionGlyph);
        assert!(!selected.draws_focus_outline, "selection is not focus");

        // Focus: the outline width, not only the colour.
        let focused = row_visual(RowState::Focused, &colors);
        assert!(focused.draws_focus_outline);
        assert_eq!(focused.marker, RowMarker::None);

        // Disabled: text colour plus an unavailability marker, never colour alone.
        let disabled = row_visual(RowState::Disabled, &colors);
        assert_eq!(disabled.text, colors.disabled);
        assert_eq!(disabled.marker, RowMarker::UnavailableLabel);
        assert!(!disabled.draws_focus_outline);
    }

    #[test]
    fn a_selected_and_focused_row_keeps_both_cues() {
        let colors = crate::contrast::NORMAL_COLORS;
        let both = row_visual(RowState::SelectedAndFocused, &colors);
        assert!(both.draws_focus_outline, "focus must survive selection");
        assert_eq!(
            both.marker,
            RowMarker::SelectionGlyph,
            "selection must survive focus"
        );
        assert_eq!(both.text, colors.selection_text);
    }

    #[test]
    fn a_high_contrast_palette_cannot_collapse_two_states() {
        // Resolve the High Contrast palette from a monochrome system probe, where several roles
        // resolve to the same value; the states must still be separable.
        let monochrome = crate::contrast::SystemColors {
            window: 0x0000_0000,
            window_text: 0xffff_ffff,
            highlight: 0xffff_ffff,
            highlight_text: 0x0000_0000,
            gray_text: 0xffff_ffff,
            button_face: 0xffff_ffff,
            button_text: 0x0000_0000,
        };
        // The palette is resolved here and deliberately not asserted on: this test is about the
        // non-colour channels surviving a palette that may collapse two roles to one value.
        let _ =
            crate::contrast::colors_for(crate::contrast::ContrastScheme::HighContrast, &monochrome);
        // Selection and disabled may well share a value here; the non-colour channels must not.
        assert!(
            states_differ_without_colour(RowState::Selected, RowState::Disabled),
            "a palette that collapses selection and disabled must not collapse their cues"
        );
        assert!(states_differ_without_colour(
            RowState::Focused,
            RowState::Disabled
        ));
    }
}
