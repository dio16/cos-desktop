//! One-way adapters from the resolved token set to GPUI shell inputs.
//!
//! Every function here takes the injected [`DesignTokens`]; nothing reads a
//! constant, keeps a default or re-derives a token set, so the composition root
//! stays the only resolution point and an override cannot be bypassed by a
//! surface. The adapters are pure: they never touch a window, GPU or texture.

use std::sync::Arc;

use gpui::{
    Div, Font, FontFallbacks, FontFeatures, FontStyle, FontWeight, Pixels, SharedString, Styled, px,
};

use crate::tokens::{DesignTokens, TextStyle};

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
    pub(crate) fn ui(tokens: &DesignTokens, style: TextStyle) -> Self {
        let metrics = tokens.typography.metrics(style);
        let weight = FontWeight(f32::from(metrics.weight_min));
        Self {
            size: px(metrics.size_dip),
            weight,
            line_height: px(metrics.line_height_dip),
            font: Font {
                family: SharedString::from(tokens.typography.latin_ui.as_ref()),
                features: FontFeatures::default(),
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

/// One physical pixel of structural separator at `scale_factor`, in DIP.
///
/// A platform scale factor that cannot be consumed (never observed in a live
/// window) yields the declared physical-pixel count in DIP instead of a
/// fabricated width; the refusal itself stays in the token layer.
pub(crate) fn separator(tokens: &DesignTokens, scale_factor: f32) -> Pixels {
    tokens
        .geometry
        .separator_dip(scale_factor)
        .map_or_else(|_| px(tokens.geometry.separator_physical_pixels), px)
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
        assert_eq!(separator(&tokens, 1.0), px(1.0));
        assert_eq!(separator(&tokens, 1.25), px(0.8));
        assert_eq!(separator(&tokens, 1.5), px(1.0 / 1.5));
        assert_eq!(separator(&tokens, 2.0), px(0.5));
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
        assert_eq!(separator(&tokens, 2.0), px(1.0));
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
}
