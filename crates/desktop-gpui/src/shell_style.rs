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

/// Focus ring colour for a focused container in the Dock.
pub(crate) const DOCK_FOCUS_COLOR: u32 = 0xe6c278;
/// Structural border colour when the Dock holds no keyboard focus.
pub(crate) const DOCK_STRUCTURE_COLOR: u32 = 0x626870;
/// Selection colour of one dock row; independent of keyboard focus.
pub(crate) const DOCK_SELECTED_COLOR: u32 = 0xaeccec;

/// The single keyboard-focus ring of the Dock: the approved focus-outline width
/// always, so acquiring focus changes colour only and never layout.
pub(crate) fn dock_focus_ring(tokens: &DesignTokens, focused: bool) -> (Pixels, u32) {
    (
        focus_outline(tokens),
        if focused {
            DOCK_FOCUS_COLOR
        } else {
            DOCK_STRUCTURE_COLOR
        },
    )
}

/// Structural border of one dock row: one physical pixel, never the focus width.
pub(crate) fn dock_row_border(
    tokens: &DesignTokens,
    scale_factor: f32,
) -> Result<Pixels, TokenError> {
    separator(tokens, scale_factor)
}

/// Row colour: selection only. Keyboard focus is drawn by [`dock_focus_ring`].
pub(crate) fn dock_row_color(selected: bool) -> u32 {
    if selected {
        DOCK_SELECTED_COLOR
    } else {
        DOCK_STRUCTURE_COLOR
    }
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
        let (focused_width, focused_color) = dock_focus_ring(&tokens, true);
        let (unfocused_width, unfocused_color) = dock_focus_ring(&tokens, false);
        assert_eq!(focused_width, focus_outline(&tokens));
        assert_eq!(focused_width, px(2.0));
        assert_eq!(
            focused_width, unfocused_width,
            "acquiring focus must change colour only, never layout width"
        );
        assert_ne!(focused_color, unfocused_color);
        assert_eq!(unfocused_color, DOCK_STRUCTURE_COLOR);
    }

    #[test]
    fn a_focus_outline_override_reaches_the_dock_ring() {
        let tokens = DesignTokensOverride {
            focus_outline_dip: Some(3.0),
            ..DesignTokensOverride::default()
        }
        .resolve_accepted()
        .expect("a valid override resolves");
        assert_eq!(dock_focus_ring(&tokens, true).0, px(3.0));
        assert_eq!(dock_focus_ring(&tokens, false).0, px(3.0));
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
        assert_eq!(dock_row_color(true), DOCK_SELECTED_COLOR);
        assert_eq!(dock_row_color(false), DOCK_STRUCTURE_COLOR);
        assert_ne!(
            dock_row_color(true),
            DOCK_FOCUS_COLOR,
            "selection and focus are distinct"
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
}
