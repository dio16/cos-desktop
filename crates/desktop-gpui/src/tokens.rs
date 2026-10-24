//! Accepted typography and geometry tokens for the native shell.
//!
//! These are the rendering §9.1 and §9.2 values ratified as the initial
//! adopted scale by ADR-0008 §5 (DD-04a/DD-04b). The module is plain data: it
//! bundles and downloads no font, implements no shaping engine, owns no GPUI
//! resource and changes no dependency. A consumer resolves a
//! [`DesignTokensOverride`] and reads the accepted value from the resolved
//! set instead of writing a literal at its use site.
//!
//! [`DesignTokens::accepted`] returns the documented values verbatim; every
//! leaf is a named constant (`TYPE_XS_SIZE_DIP`, `SPACING_4_DIP`, ...) and
//! every leaf is replaceable through [`DesignTokensOverride`], the only
//! supported configuration path. Resolution is pure and refuses an unworkable
//! set with [`TokenError`] instead of clamping it.
//!
//! A font family is a declaration, not a resolved resource: nothing here
//! loads fonts, probes installed families or reorders the stack. Validation
//! only refuses a name no platform could consume as a family — empty or
//! whitespace-only, padded with leading/trailing whitespace, or containing
//! control characters — and the platform shaping layer still resolves the
//! primary family and then its fallback.
//!
//! Scope boundaries: this is a **token foundation**, not MP-TYP1-wide
//! completion. Native DPI evidence at 100/125/150/175/200 % is `NOT_MEASURED`
//! here: this environment cannot drive native window scales or capture
//! screenshots, so mixed-script shaping, font rasterisation, DPI invalidation
//! and focus/scroll preservation remain open and no native or DPI PASS is
//! claimed. Colour, motion, icons and depth (§9.3/§9.4) are MP-TYP2 and are
//! not declared here. §9.1 asks for kerning/letter-spacing, baseline and
//! wrapping tokens without proposing numeric values, so none is invented here
//! (ADR-0008 §5 leaves unanswered questions open).
//!
//! The structural separator is declared in **physical pixels** and converted
//! per monitor scale: [`Geometry::separator_dip`] returns
//! `physical_pixels / scale_factor`, so a 200 % monitor gets a 0.5-DIP
//! hairline instead of a 1-DIP line. The keyboard focus outline is a separate
//! accepted DIP token and is never derived from that hairline.

use std::borrow::Cow;

// Accepted typography tokens (rendering §9.1; DD-04a, ADR-0008 §5).

/// Accepted Latin UI family; first member of the ratified stack
/// (rendering §9.1, ADR-0008 §5).
pub const LATIN_UI_FAMILY: &str = "Segoe UI Variable";

/// Accepted Latin UI fallback used when the first family is unavailable.
pub const LATIN_UI_FALLBACK_FAMILY: &str = "Segoe UI";

/// Accepted Japanese UI family; first native fallback candidate
/// (rendering §9.1).
pub const JAPANESE_UI_FAMILY: &str = "Yu Gothic UI";

/// Accepted Japanese UI fallback family. Installed Noto CJK stays an
/// optional platform candidate and is never bundled.
pub const JAPANESE_UI_FALLBACK_FAMILY: &str = "Meiryo";

/// Accepted monospace family for code and diagnostics (rendering §9.1).
pub const MONOSPACE_FAMILY: &str = "Cascadia Mono";

/// Accepted monospace fallback family.
pub const MONOSPACE_FALLBACK_FAMILY: &str = "Consolas";

/// Keep tabular numerals for aligned numeric values where the installed
/// face provides them (rendering §9.1).
pub const TABULAR_NUMERALS: bool = true;

/// `type-xs` size in DIP (rendering §9.1).
pub const TYPE_XS_SIZE_DIP: f32 = 12.0;

/// `type-xs` lightest accepted CSS-like weight (rendering §9.1).
pub const TYPE_XS_WEIGHT_MIN: u16 = 400;

/// `type-xs` heaviest accepted CSS-like weight (rendering §9.1).
pub const TYPE_XS_WEIGHT_MAX: u16 = 500;

/// `type-xs` line height in DIP (rendering §9.1).
pub const TYPE_XS_LINE_HEIGHT_DIP: f32 = 16.0;

/// `type-xs` Japanese fallback line height in DIP (rendering §9.1,
/// 1.5x the size rounded to a whole DIP).
pub const TYPE_XS_JAPANESE_LINE_HEIGHT_DIP: f32 = 18.0;

/// `type-sm` size in DIP (rendering §9.1).
pub const TYPE_SM_SIZE_DIP: f32 = 13.0;

/// `type-sm` lightest accepted CSS-like weight (rendering §9.1).
pub const TYPE_SM_WEIGHT_MIN: u16 = 400;

/// `type-sm` heaviest accepted CSS-like weight (rendering §9.1).
pub const TYPE_SM_WEIGHT_MAX: u16 = 500;

/// `type-sm` line height in DIP (rendering §9.1).
pub const TYPE_SM_LINE_HEIGHT_DIP: f32 = 18.0;

/// `type-sm` Japanese fallback line height in DIP (rendering §9.1,
/// 1.5x the size rounded to a whole DIP).
pub const TYPE_SM_JAPANESE_LINE_HEIGHT_DIP: f32 = 20.0;

/// `type-body` size in DIP (rendering §9.1).
pub const TYPE_BODY_SIZE_DIP: f32 = 14.0;

/// `type-body` lightest accepted CSS-like weight (rendering §9.1).
pub const TYPE_BODY_WEIGHT_MIN: u16 = 400;

/// `type-body` heaviest accepted CSS-like weight (rendering §9.1).
pub const TYPE_BODY_WEIGHT_MAX: u16 = 400;

/// `type-body` line height in DIP (rendering §9.1).
pub const TYPE_BODY_LINE_HEIGHT_DIP: f32 = 20.0;

/// `type-body` Japanese fallback line height in DIP (rendering §9.1,
/// 1.5x the size rounded to a whole DIP).
pub const TYPE_BODY_JAPANESE_LINE_HEIGHT_DIP: f32 = 21.0;

/// `type-body-strong` size in DIP (rendering §9.1).
pub const TYPE_BODY_STRONG_SIZE_DIP: f32 = 14.0;

/// `type-body-strong` lightest accepted CSS-like weight (rendering §9.1).
pub const TYPE_BODY_STRONG_WEIGHT_MIN: u16 = 600;

/// `type-body-strong` heaviest accepted CSS-like weight (rendering §9.1).
pub const TYPE_BODY_STRONG_WEIGHT_MAX: u16 = 600;

/// `type-body-strong` line height in DIP (rendering §9.1).
pub const TYPE_BODY_STRONG_LINE_HEIGHT_DIP: f32 = 20.0;

/// `type-body-strong` Japanese fallback line height in DIP (rendering §9.1,
/// 1.5x the size rounded to a whole DIP).
pub const TYPE_BODY_STRONG_JAPANESE_LINE_HEIGHT_DIP: f32 = 21.0;

/// `type-control` size in DIP (rendering §9.1).
pub const TYPE_CONTROL_SIZE_DIP: f32 = 16.0;

/// `type-control` lightest accepted CSS-like weight (rendering §9.1).
pub const TYPE_CONTROL_WEIGHT_MIN: u16 = 400;

/// `type-control` heaviest accepted CSS-like weight (rendering §9.1).
pub const TYPE_CONTROL_WEIGHT_MAX: u16 = 500;

/// `type-control` line height in DIP (rendering §9.1).
pub const TYPE_CONTROL_LINE_HEIGHT_DIP: f32 = 24.0;

/// `type-control` Japanese fallback line height in DIP (rendering §9.1,
/// 1.5x the size rounded to a whole DIP).
pub const TYPE_CONTROL_JAPANESE_LINE_HEIGHT_DIP: f32 = 24.0;

/// `type-title` size in DIP (rendering §9.1).
pub const TYPE_TITLE_SIZE_DIP: f32 = 20.0;

/// `type-title` lightest accepted CSS-like weight (rendering §9.1).
pub const TYPE_TITLE_WEIGHT_MIN: u16 = 600;

/// `type-title` heaviest accepted CSS-like weight (rendering §9.1).
pub const TYPE_TITLE_WEIGHT_MAX: u16 = 600;

/// `type-title` line height in DIP (rendering §9.1).
pub const TYPE_TITLE_LINE_HEIGHT_DIP: f32 = 28.0;

/// `type-title` Japanese fallback line height in DIP (rendering §9.1,
/// 1.5x the size rounded to a whole DIP).
pub const TYPE_TITLE_JAPANESE_LINE_HEIGHT_DIP: f32 = 30.0;

/// `type-display` size in DIP (rendering §9.1).
pub const TYPE_DISPLAY_SIZE_DIP: f32 = 24.0;

/// `type-display` lightest accepted CSS-like weight (rendering §9.1).
pub const TYPE_DISPLAY_WEIGHT_MIN: u16 = 600;

/// `type-display` heaviest accepted CSS-like weight (rendering §9.1).
pub const TYPE_DISPLAY_WEIGHT_MAX: u16 = 600;

/// `type-display` line height in DIP (rendering §9.1).
pub const TYPE_DISPLAY_LINE_HEIGHT_DIP: f32 = 32.0;

/// `type-display` Japanese fallback line height in DIP (rendering §9.1,
/// 1.5x the size rounded to a whole DIP).
pub const TYPE_DISPLAY_JAPANESE_LINE_HEIGHT_DIP: f32 = 36.0;

// Accepted geometry tokens (rendering §9.2; DD-04b, ADR-0008 §5).

/// 4-DIP spacing scale step 4 DIP (rendering §9.2).
pub const SPACING_4_DIP: f32 = 4.0;

/// 4-DIP spacing scale step 8 DIP (rendering §9.2).
pub const SPACING_8_DIP: f32 = 8.0;

/// 4-DIP spacing scale step 12 DIP (rendering §9.2).
pub const SPACING_12_DIP: f32 = 12.0;

/// 4-DIP spacing scale step 16 DIP (rendering §9.2).
pub const SPACING_16_DIP: f32 = 16.0;

/// 4-DIP spacing scale step 20 DIP (rendering §9.2).
pub const SPACING_20_DIP: f32 = 20.0;

/// 4-DIP spacing scale step 24 DIP (rendering §9.2).
pub const SPACING_24_DIP: f32 = 24.0;

/// 4-DIP spacing scale step 32 DIP (rendering §9.2).
pub const SPACING_32_DIP: f32 = 32.0;

/// 4-DIP spacing scale step 40 DIP (rendering §9.2).
pub const SPACING_40_DIP: f32 = 40.0;

/// 4-DIP spacing scale step 48 DIP (rendering §9.2).
pub const SPACING_48_DIP: f32 = 48.0;

/// Dense list row height in DIP (rendering §9.2).
pub const ROW_DENSE_DIP: f32 = 32.0;

/// Comfortable list row height in DIP (rendering §9.2).
pub const ROW_COMFORTABLE_DIP: f32 = 40.0;

/// Title bar height in DIP (rendering §9.2).
pub const TITLE_BAR_DIP: f32 = 40.0;

/// Task rail width in DIP (rendering §9.2).
pub const TASK_RAIL_DIP: f32 = 240.0;

/// Inspector width in DIP (rendering §9.2).
pub const INSPECTOR_DIP: f32 = 320.0;

/// Thumbnail size in DIP (rendering §9.2).
pub const THUMBNAIL_DIP: f32 = 72.0;

/// High-resolution preview size in DIP (rendering §9.2).
pub const PREVIEW_DIP: f32 = 96.0;

/// Minimum icon/control target in DIP inside a larger accessible hit
/// region (rendering §9.2). The accepted value is the documented floor.
pub const MIN_CONTROL_TARGET_DIP: f32 = 24.0;

/// Square (no radius) corner token in DIP (rendering §9.2).
pub const RADIUS_NONE_DIP: f32 = 0.0;

/// 4-DIP corner radius token (rendering §9.2).
pub const RADIUS_SMALL_DIP: f32 = 4.0;

/// 8-DIP corner radius token (rendering §9.2).
pub const RADIUS_LARGE_DIP: f32 = 8.0;

/// Structural separator thickness in **physical pixels**; convert it with
/// [`Geometry::separator_dip`] instead of declaring a 1-DIP line
/// (rendering §9.2).
pub const SEPARATOR_PHYSICAL_PIXELS: f32 = 1.0;

/// Keyboard focus outline width in DIP (rendering §9.2). It is never
/// derived from the structural hairline, and the accepted value is the
/// documented floor.
pub const FOCUS_OUTLINE_DIP: f32 = 2.0;

/// One accepted type scale entry named by rendering §9.1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextStyle {
    Xs,
    Sm,
    Body,
    BodyStrong,
    Control,
    Title,
    Display,
}

impl TextStyle {
    /// Every declared type scale entry, in ascending size order.
    pub const ALL: [Self; 7] = [
        Self::Xs,
        Self::Sm,
        Self::Body,
        Self::BodyStrong,
        Self::Control,
        Self::Title,
        Self::Display,
    ];

    /// The rendering §9.1 token name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Xs => "type-xs",
            Self::Sm => "type-sm",
            Self::Body => "type-body",
            Self::BodyStrong => "type-body-strong",
            Self::Control => "type-control",
            Self::Title => "type-title",
            Self::Display => "type-display",
        }
    }
}

impl std::fmt::Display for TextStyle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.name())
    }
}

/// One accepted geometry token named by rendering §9.2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dimension {
    RowDense,
    RowComfortable,
    TitleBar,
    TaskRail,
    Inspector,
    Thumbnail,
    Preview,
    MinControlTarget,
    Spacing4,
    Spacing8,
    Spacing12,
    Spacing16,
    Spacing20,
    Spacing24,
    Spacing32,
    Spacing40,
    Spacing48,
    SeparatorPhysicalPixels,
    RadiusNone,
    RadiusSmall,
    RadiusLarge,
}

impl Dimension {
    /// The rendering §9.2 token name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::RowDense => "row-dense",
            Self::RowComfortable => "row-comfortable",
            Self::TitleBar => "title-bar",
            Self::TaskRail => "task-rail",
            Self::Inspector => "inspector",
            Self::Thumbnail => "thumbnail",
            Self::Preview => "preview",
            Self::MinControlTarget => "min-control-target",
            Self::Spacing4 => "spacing-4",
            Self::Spacing8 => "spacing-8",
            Self::Spacing12 => "spacing-12",
            Self::Spacing16 => "spacing-16",
            Self::Spacing20 => "spacing-20",
            Self::Spacing24 => "spacing-24",
            Self::Spacing32 => "spacing-32",
            Self::Spacing40 => "spacing-40",
            Self::Spacing48 => "spacing-48",
            Self::SeparatorPhysicalPixels => "separator",
            Self::RadiusNone => "radius-none",
            Self::RadiusSmall => "radius-small",
            Self::RadiusLarge => "radius-large",
        }
    }
}

impl std::fmt::Display for Dimension {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.name())
    }
}

/// One accepted font-family declaration named by rendering §9.1.
///
/// A family is a declaration only: this module neither loads fonts nor probes
/// installed families, and the platform shaping layer still resolves the
/// primary family and then its fallback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FamilyToken {
    LatinUi,
    LatinUiFallback,
    JapaneseUi,
    JapaneseUiFallback,
    Monospace,
    MonospaceFallback,
}

impl FamilyToken {
    /// The rendering §9.1 token name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::LatinUi => "latin-ui",
            Self::LatinUiFallback => "latin-ui-fallback",
            Self::JapaneseUi => "japanese-ui",
            Self::JapaneseUiFallback => "japanese-ui-fallback",
            Self::Monospace => "monospace",
            Self::MonospaceFallback => "monospace-fallback",
        }
    }

    /// Refuse a declaration that no platform can consume as a family name.
    ///
    /// The rule is exact: a usable name is non-empty after trimming (so empty
    /// and whitespace-only names are refused), is identical to its trimmed
    /// form (so leading/trailing padding is refused) and contains no Unicode
    /// control character.
    fn unusable(self, name: &str) -> Option<TokenError> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Some(TokenError::FamilyNameEmpty { family: self });
        }
        if trimmed != name {
            return Some(TokenError::FamilyNameUntrimmed { family: self });
        }
        if name.chars().any(char::is_control) {
            return Some(TokenError::FamilyNameContainsControl { family: self });
        }
        None
    }
}

impl std::fmt::Display for FamilyToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.name())
    }
}

/// Resolved metrics of one [`TextStyle`] entry (rendering §9.1).
///
/// Produced only by [`Typography::metrics`] from a resolved token set, so a
/// consumer never restates a size, weight or line height at its use site.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TypeScale {
    /// Type size in DIP.
    pub size_dip: f32,
    /// Lightest accepted CSS-like weight.
    pub weight_min: u16,
    /// Heaviest accepted CSS-like weight.
    pub weight_max: u16,
    /// Latin line height in DIP.
    pub line_height_dip: f32,
    /// Japanese fallback line height in DIP.
    pub japanese_line_height_dip: f32,
}

/// Accepted typography tokens: the ratified font stack and the type scale
/// (rendering §9.1).
///
/// Every field is a named constant above and is overridable through
/// [`DesignTokensOverride`]; no consumer writes one of these values itself.
#[derive(Clone, Debug, PartialEq)]
pub struct Typography {
    /// Accepted Latin UI family; first member of the ratified stack
    /// (rendering §9.1, ADR-0008 §5).
    pub latin_ui: Cow<'static, str>,
    /// Accepted Latin UI fallback used when the first family is unavailable.
    pub latin_ui_fallback: Cow<'static, str>,
    /// Accepted Japanese UI family; first native fallback candidate
    /// (rendering §9.1).
    pub japanese_ui: Cow<'static, str>,
    /// Accepted Japanese UI fallback family. Installed Noto CJK stays an
    /// optional platform candidate and is never bundled.
    pub japanese_ui_fallback: Cow<'static, str>,
    /// Accepted monospace family for code and diagnostics (rendering §9.1).
    pub monospace: Cow<'static, str>,
    /// Accepted monospace fallback family.
    pub monospace_fallback: Cow<'static, str>,
    /// Keep tabular numerals for aligned numeric values where the installed
    /// face provides them (rendering §9.1).
    pub tabular_numerals: bool,
    /// `type-xs` size in DIP (rendering §9.1).
    pub xs_size_dip: f32,
    /// `type-xs` lightest accepted CSS-like weight (rendering §9.1).
    pub xs_weight_min: u16,
    /// `type-xs` heaviest accepted CSS-like weight (rendering §9.1).
    pub xs_weight_max: u16,
    /// `type-xs` line height in DIP (rendering §9.1).
    pub xs_line_height_dip: f32,
    /// `type-xs` Japanese fallback line height in DIP (rendering §9.1,
    /// 1.5x the size rounded to a whole DIP).
    pub xs_japanese_line_height_dip: f32,
    /// `type-sm` size in DIP (rendering §9.1).
    pub sm_size_dip: f32,
    /// `type-sm` lightest accepted CSS-like weight (rendering §9.1).
    pub sm_weight_min: u16,
    /// `type-sm` heaviest accepted CSS-like weight (rendering §9.1).
    pub sm_weight_max: u16,
    /// `type-sm` line height in DIP (rendering §9.1).
    pub sm_line_height_dip: f32,
    /// `type-sm` Japanese fallback line height in DIP (rendering §9.1,
    /// 1.5x the size rounded to a whole DIP).
    pub sm_japanese_line_height_dip: f32,
    /// `type-body` size in DIP (rendering §9.1).
    pub body_size_dip: f32,
    /// `type-body` lightest accepted CSS-like weight (rendering §9.1).
    pub body_weight_min: u16,
    /// `type-body` heaviest accepted CSS-like weight (rendering §9.1).
    pub body_weight_max: u16,
    /// `type-body` line height in DIP (rendering §9.1).
    pub body_line_height_dip: f32,
    /// `type-body` Japanese fallback line height in DIP (rendering §9.1,
    /// 1.5x the size rounded to a whole DIP).
    pub body_japanese_line_height_dip: f32,
    /// `type-body-strong` size in DIP (rendering §9.1).
    pub body_strong_size_dip: f32,
    /// `type-body-strong` lightest accepted CSS-like weight (rendering §9.1).
    pub body_strong_weight_min: u16,
    /// `type-body-strong` heaviest accepted CSS-like weight (rendering §9.1).
    pub body_strong_weight_max: u16,
    /// `type-body-strong` line height in DIP (rendering §9.1).
    pub body_strong_line_height_dip: f32,
    /// `type-body-strong` Japanese fallback line height in DIP (rendering §9.1,
    /// 1.5x the size rounded to a whole DIP).
    pub body_strong_japanese_line_height_dip: f32,
    /// `type-control` size in DIP (rendering §9.1).
    pub control_size_dip: f32,
    /// `type-control` lightest accepted CSS-like weight (rendering §9.1).
    pub control_weight_min: u16,
    /// `type-control` heaviest accepted CSS-like weight (rendering §9.1).
    pub control_weight_max: u16,
    /// `type-control` line height in DIP (rendering §9.1).
    pub control_line_height_dip: f32,
    /// `type-control` Japanese fallback line height in DIP (rendering §9.1,
    /// 1.5x the size rounded to a whole DIP).
    pub control_japanese_line_height_dip: f32,
    /// `type-title` size in DIP (rendering §9.1).
    pub title_size_dip: f32,
    /// `type-title` lightest accepted CSS-like weight (rendering §9.1).
    pub title_weight_min: u16,
    /// `type-title` heaviest accepted CSS-like weight (rendering §9.1).
    pub title_weight_max: u16,
    /// `type-title` line height in DIP (rendering §9.1).
    pub title_line_height_dip: f32,
    /// `type-title` Japanese fallback line height in DIP (rendering §9.1,
    /// 1.5x the size rounded to a whole DIP).
    pub title_japanese_line_height_dip: f32,
    /// `type-display` size in DIP (rendering §9.1).
    pub display_size_dip: f32,
    /// `type-display` lightest accepted CSS-like weight (rendering §9.1).
    pub display_weight_min: u16,
    /// `type-display` heaviest accepted CSS-like weight (rendering §9.1).
    pub display_weight_max: u16,
    /// `type-display` line height in DIP (rendering §9.1).
    pub display_line_height_dip: f32,
    /// `type-display` Japanese fallback line height in DIP (rendering §9.1,
    /// 1.5x the size rounded to a whole DIP).
    pub display_japanese_line_height_dip: f32,
}

impl Typography {
    /// Resolved metrics for one accepted type scale entry.
    #[must_use]
    pub fn metrics(&self, style: TextStyle) -> TypeScale {
        match style {
            TextStyle::Xs => TypeScale {
                size_dip: self.xs_size_dip,
                weight_min: self.xs_weight_min,
                weight_max: self.xs_weight_max,
                line_height_dip: self.xs_line_height_dip,
                japanese_line_height_dip: self.xs_japanese_line_height_dip,
            },
            TextStyle::Sm => TypeScale {
                size_dip: self.sm_size_dip,
                weight_min: self.sm_weight_min,
                weight_max: self.sm_weight_max,
                line_height_dip: self.sm_line_height_dip,
                japanese_line_height_dip: self.sm_japanese_line_height_dip,
            },
            TextStyle::Body => TypeScale {
                size_dip: self.body_size_dip,
                weight_min: self.body_weight_min,
                weight_max: self.body_weight_max,
                line_height_dip: self.body_line_height_dip,
                japanese_line_height_dip: self.body_japanese_line_height_dip,
            },
            TextStyle::BodyStrong => TypeScale {
                size_dip: self.body_strong_size_dip,
                weight_min: self.body_strong_weight_min,
                weight_max: self.body_strong_weight_max,
                line_height_dip: self.body_strong_line_height_dip,
                japanese_line_height_dip: self.body_strong_japanese_line_height_dip,
            },
            TextStyle::Control => TypeScale {
                size_dip: self.control_size_dip,
                weight_min: self.control_weight_min,
                weight_max: self.control_weight_max,
                line_height_dip: self.control_line_height_dip,
                japanese_line_height_dip: self.control_japanese_line_height_dip,
            },
            TextStyle::Title => TypeScale {
                size_dip: self.title_size_dip,
                weight_min: self.title_weight_min,
                weight_max: self.title_weight_max,
                line_height_dip: self.title_line_height_dip,
                japanese_line_height_dip: self.title_japanese_line_height_dip,
            },
            TextStyle::Display => TypeScale {
                size_dip: self.display_size_dip,
                weight_min: self.display_weight_min,
                weight_max: self.display_weight_max,
                line_height_dip: self.display_line_height_dip,
                japanese_line_height_dip: self.display_japanese_line_height_dip,
            },
        }
    }
}

/// Accepted geometry tokens: the 4-DIP spacing scale, row heights, chrome
/// widths, thumbnail sizes, radii, the physical-pixel separator and the
/// focus outline (rendering §9.2).
#[derive(Clone, Debug, PartialEq)]
pub struct Geometry {
    /// 4-DIP spacing scale step 4 DIP (rendering §9.2).
    pub spacing_4_dip: f32,
    /// 4-DIP spacing scale step 8 DIP (rendering §9.2).
    pub spacing_8_dip: f32,
    /// 4-DIP spacing scale step 12 DIP (rendering §9.2).
    pub spacing_12_dip: f32,
    /// 4-DIP spacing scale step 16 DIP (rendering §9.2).
    pub spacing_16_dip: f32,
    /// 4-DIP spacing scale step 20 DIP (rendering §9.2).
    pub spacing_20_dip: f32,
    /// 4-DIP spacing scale step 24 DIP (rendering §9.2).
    pub spacing_24_dip: f32,
    /// 4-DIP spacing scale step 32 DIP (rendering §9.2).
    pub spacing_32_dip: f32,
    /// 4-DIP spacing scale step 40 DIP (rendering §9.2).
    pub spacing_40_dip: f32,
    /// 4-DIP spacing scale step 48 DIP (rendering §9.2).
    pub spacing_48_dip: f32,
    /// Dense list row height in DIP (rendering §9.2).
    pub row_dense_dip: f32,
    /// Comfortable list row height in DIP (rendering §9.2).
    pub row_comfortable_dip: f32,
    /// Title bar height in DIP (rendering §9.2).
    pub title_bar_dip: f32,
    /// Task rail width in DIP (rendering §9.2).
    pub task_rail_dip: f32,
    /// Inspector width in DIP (rendering §9.2).
    pub inspector_dip: f32,
    /// Thumbnail size in DIP (rendering §9.2).
    pub thumbnail_dip: f32,
    /// High-resolution preview size in DIP (rendering §9.2).
    pub preview_dip: f32,
    /// Minimum icon/control target in DIP inside a larger accessible hit
    /// region (rendering §9.2). The accepted value is the documented floor.
    pub min_control_target_dip: f32,
    /// Square (no radius) corner token in DIP (rendering §9.2).
    pub radius_none_dip: f32,
    /// 4-DIP corner radius token (rendering §9.2).
    pub radius_small_dip: f32,
    /// 8-DIP corner radius token (rendering §9.2).
    pub radius_large_dip: f32,
    /// Structural separator thickness in **physical pixels**; convert it with
    /// [`Geometry::separator_dip`] instead of declaring a 1-DIP line
    /// (rendering §9.2).
    pub separator_physical_pixels: f32,
    /// Keyboard focus outline width in DIP (rendering §9.2). It is never
    /// derived from the structural hairline, and the accepted value is the
    /// documented floor.
    pub focus_outline_dip: f32,
}

impl Geometry {
    /// The structural separator in DIP for one physical pixel at
    /// `scale_factor`.
    ///
    /// Rendering §9.2 uses a 1-physical-pixel divider (`1 / scale_factor`
    /// DIP) rather than a 1-DIP line at fractional DPI, so the divider stays
    /// one physical pixel at every monitor scale. A non-positive or
    /// non-finite scale factor is refused instead of producing a negative or
    /// infinite width.
    pub fn separator_dip(&self, scale_factor: f32) -> Result<f32, TokenError> {
        if !scale_factor.is_finite() || scale_factor <= 0.0 {
            return Err(TokenError::NonPositiveScaleFactor { scale_factor });
        }
        Ok(self.separator_physical_pixels / scale_factor)
    }

    fn validate(&self) -> Result<(), TokenError> {
        for (dimension, value) in [
            (Dimension::Spacing4, self.spacing_4_dip),
            (Dimension::Spacing8, self.spacing_8_dip),
            (Dimension::Spacing12, self.spacing_12_dip),
            (Dimension::Spacing16, self.spacing_16_dip),
            (Dimension::Spacing20, self.spacing_20_dip),
            (Dimension::Spacing24, self.spacing_24_dip),
            (Dimension::Spacing32, self.spacing_32_dip),
            (Dimension::Spacing40, self.spacing_40_dip),
            (Dimension::Spacing48, self.spacing_48_dip),
            (Dimension::RowDense, self.row_dense_dip),
            (Dimension::RowComfortable, self.row_comfortable_dip),
            (Dimension::TitleBar, self.title_bar_dip),
            (Dimension::TaskRail, self.task_rail_dip),
            (Dimension::Inspector, self.inspector_dip),
            (Dimension::Thumbnail, self.thumbnail_dip),
            (Dimension::Preview, self.preview_dip),
            (Dimension::MinControlTarget, self.min_control_target_dip),
            (
                Dimension::SeparatorPhysicalPixels,
                self.separator_physical_pixels,
            ),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(TokenError::NonPositiveDimension { dimension });
            }
        }
        for (dimension, value) in [
            (Dimension::RadiusNone, self.radius_none_dip),
            (Dimension::RadiusSmall, self.radius_small_dip),
            (Dimension::RadiusLarge, self.radius_large_dip),
        ] {
            if !value.is_finite() || value < 0.0 {
                return Err(TokenError::NegativeRadius { dimension });
            }
        }
        if self.radius_none_dip > self.radius_small_dip
            || self.radius_small_dip > self.radius_large_dip
        {
            return Err(TokenError::RadiiNotAscending);
        }
        let spacing = [
            self.spacing_4_dip,
            self.spacing_8_dip,
            self.spacing_12_dip,
            self.spacing_16_dip,
            self.spacing_20_dip,
            self.spacing_24_dip,
            self.spacing_32_dip,
            self.spacing_40_dip,
            self.spacing_48_dip,
        ];
        if spacing.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(TokenError::SpacingNotAscending);
        }
        if self.row_dense_dip > self.row_comfortable_dip {
            return Err(TokenError::RowHeightsInverted);
        }
        if self.preview_dip < self.thumbnail_dip {
            return Err(TokenError::PreviewSmallerThanThumbnail);
        }
        if !self.focus_outline_dip.is_finite() || self.focus_outline_dip < FOCUS_OUTLINE_DIP {
            return Err(TokenError::FocusOutlineBelowMinimum {
                width_dip: self.focus_outline_dip,
            });
        }
        if self.min_control_target_dip < MIN_CONTROL_TARGET_DIP {
            return Err(TokenError::ControlTargetBelowMinimum {
                width_dip: self.min_control_target_dip,
            });
        }
        Ok(())
    }
}

/// One resolved typography and geometry token set for the native shell.
///
/// A value from [`Self::accepted`] or [`DesignTokensOverride::resolve`] has
/// already passed [`Self::validate`], which is the single refusal point; a
/// caller that assembles a set field by field must validate it before use.
#[derive(Clone, Debug, PartialEq)]
pub struct DesignTokens {
    pub typography: Typography,
    pub geometry: Geometry,
}

impl DesignTokens {
    /// The accepted token set: rendering §9.1 and §9.2 verbatim.
    #[must_use]
    pub const fn accepted() -> Self {
        Self {
            typography: Typography {
                latin_ui: Cow::Borrowed(LATIN_UI_FAMILY),
                latin_ui_fallback: Cow::Borrowed(LATIN_UI_FALLBACK_FAMILY),
                japanese_ui: Cow::Borrowed(JAPANESE_UI_FAMILY),
                japanese_ui_fallback: Cow::Borrowed(JAPANESE_UI_FALLBACK_FAMILY),
                monospace: Cow::Borrowed(MONOSPACE_FAMILY),
                monospace_fallback: Cow::Borrowed(MONOSPACE_FALLBACK_FAMILY),
                tabular_numerals: TABULAR_NUMERALS,
                xs_size_dip: TYPE_XS_SIZE_DIP,
                xs_weight_min: TYPE_XS_WEIGHT_MIN,
                xs_weight_max: TYPE_XS_WEIGHT_MAX,
                xs_line_height_dip: TYPE_XS_LINE_HEIGHT_DIP,
                xs_japanese_line_height_dip: TYPE_XS_JAPANESE_LINE_HEIGHT_DIP,
                sm_size_dip: TYPE_SM_SIZE_DIP,
                sm_weight_min: TYPE_SM_WEIGHT_MIN,
                sm_weight_max: TYPE_SM_WEIGHT_MAX,
                sm_line_height_dip: TYPE_SM_LINE_HEIGHT_DIP,
                sm_japanese_line_height_dip: TYPE_SM_JAPANESE_LINE_HEIGHT_DIP,
                body_size_dip: TYPE_BODY_SIZE_DIP,
                body_weight_min: TYPE_BODY_WEIGHT_MIN,
                body_weight_max: TYPE_BODY_WEIGHT_MAX,
                body_line_height_dip: TYPE_BODY_LINE_HEIGHT_DIP,
                body_japanese_line_height_dip: TYPE_BODY_JAPANESE_LINE_HEIGHT_DIP,
                body_strong_size_dip: TYPE_BODY_STRONG_SIZE_DIP,
                body_strong_weight_min: TYPE_BODY_STRONG_WEIGHT_MIN,
                body_strong_weight_max: TYPE_BODY_STRONG_WEIGHT_MAX,
                body_strong_line_height_dip: TYPE_BODY_STRONG_LINE_HEIGHT_DIP,
                body_strong_japanese_line_height_dip: TYPE_BODY_STRONG_JAPANESE_LINE_HEIGHT_DIP,
                control_size_dip: TYPE_CONTROL_SIZE_DIP,
                control_weight_min: TYPE_CONTROL_WEIGHT_MIN,
                control_weight_max: TYPE_CONTROL_WEIGHT_MAX,
                control_line_height_dip: TYPE_CONTROL_LINE_HEIGHT_DIP,
                control_japanese_line_height_dip: TYPE_CONTROL_JAPANESE_LINE_HEIGHT_DIP,
                title_size_dip: TYPE_TITLE_SIZE_DIP,
                title_weight_min: TYPE_TITLE_WEIGHT_MIN,
                title_weight_max: TYPE_TITLE_WEIGHT_MAX,
                title_line_height_dip: TYPE_TITLE_LINE_HEIGHT_DIP,
                title_japanese_line_height_dip: TYPE_TITLE_JAPANESE_LINE_HEIGHT_DIP,
                display_size_dip: TYPE_DISPLAY_SIZE_DIP,
                display_weight_min: TYPE_DISPLAY_WEIGHT_MIN,
                display_weight_max: TYPE_DISPLAY_WEIGHT_MAX,
                display_line_height_dip: TYPE_DISPLAY_LINE_HEIGHT_DIP,
                display_japanese_line_height_dip: TYPE_DISPLAY_JAPANESE_LINE_HEIGHT_DIP,
            },
            geometry: Geometry {
                spacing_4_dip: SPACING_4_DIP,
                spacing_8_dip: SPACING_8_DIP,
                spacing_12_dip: SPACING_12_DIP,
                spacing_16_dip: SPACING_16_DIP,
                spacing_20_dip: SPACING_20_DIP,
                spacing_24_dip: SPACING_24_DIP,
                spacing_32_dip: SPACING_32_DIP,
                spacing_40_dip: SPACING_40_DIP,
                spacing_48_dip: SPACING_48_DIP,
                row_dense_dip: ROW_DENSE_DIP,
                row_comfortable_dip: ROW_COMFORTABLE_DIP,
                title_bar_dip: TITLE_BAR_DIP,
                task_rail_dip: TASK_RAIL_DIP,
                inspector_dip: INSPECTOR_DIP,
                thumbnail_dip: THUMBNAIL_DIP,
                preview_dip: PREVIEW_DIP,
                min_control_target_dip: MIN_CONTROL_TARGET_DIP,
                radius_none_dip: RADIUS_NONE_DIP,
                radius_small_dip: RADIUS_SMALL_DIP,
                radius_large_dip: RADIUS_LARGE_DIP,
                separator_physical_pixels: SEPARATOR_PHYSICAL_PIXELS,
                focus_outline_dip: FOCUS_OUTLINE_DIP,
            },
        }
    }

    /// Refuse a token set that cannot render faithfully.
    ///
    /// Zero, negative, non-finite or mutually contradictory values, and a
    /// font family declaration no platform could consume, are errors, never
    /// clamped values. The documented floors (the 2-DIP focus outline and the
    /// 24-DIP control target) are the accepted values, so an override cannot
    /// drop below them.
    pub fn validate(&self) -> Result<(), TokenError> {
        for (family, name) in [
            (FamilyToken::LatinUi, self.typography.latin_ui.as_ref()),
            (
                FamilyToken::LatinUiFallback,
                self.typography.latin_ui_fallback.as_ref(),
            ),
            (
                FamilyToken::JapaneseUi,
                self.typography.japanese_ui.as_ref(),
            ),
            (
                FamilyToken::JapaneseUiFallback,
                self.typography.japanese_ui_fallback.as_ref(),
            ),
            (FamilyToken::Monospace, self.typography.monospace.as_ref()),
            (
                FamilyToken::MonospaceFallback,
                self.typography.monospace_fallback.as_ref(),
            ),
        ] {
            if let Some(error) = family.unusable(name) {
                return Err(error);
            }
        }
        for style in TextStyle::ALL {
            let metrics = self.typography.metrics(style);
            if !metrics.size_dip.is_finite() || metrics.size_dip <= 0.0 {
                return Err(TokenError::TypeSizeNotPositive { style });
            }
            for weight in [metrics.weight_min, metrics.weight_max] {
                if !(1..=1000).contains(&weight) {
                    return Err(TokenError::WeightOutOfRange { style, weight });
                }
            }
            if metrics.weight_min > metrics.weight_max {
                return Err(TokenError::WeightRangeInverted { style });
            }
            if !metrics.line_height_dip.is_finite() || metrics.line_height_dip < metrics.size_dip {
                return Err(TokenError::LineHeightBelowTypeSize { style });
            }
            if !metrics.japanese_line_height_dip.is_finite()
                || metrics.japanese_line_height_dip < metrics.line_height_dip
            {
                return Err(TokenError::JapaneseLineHeightBelowLineHeight { style });
            }
        }
        self.geometry.validate()
    }
}

impl Default for DesignTokens {
    fn default() -> Self {
        Self::accepted()
    }
}

/// Field-wise override of [`DesignTokens`]; `None` keeps the base value.
///
/// A caller or a future config source sets only the tokens it changes and
/// resolves them over the accepted set (or over another explicit base), so
/// no consumer has to edit a constant or restate a value.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DesignTokensOverride {
    pub latin_ui: Option<Cow<'static, str>>,
    pub latin_ui_fallback: Option<Cow<'static, str>>,
    pub japanese_ui: Option<Cow<'static, str>>,
    pub japanese_ui_fallback: Option<Cow<'static, str>>,
    pub monospace: Option<Cow<'static, str>>,
    pub monospace_fallback: Option<Cow<'static, str>>,
    pub tabular_numerals: Option<bool>,
    pub xs_size_dip: Option<f32>,
    pub xs_weight_min: Option<u16>,
    pub xs_weight_max: Option<u16>,
    pub xs_line_height_dip: Option<f32>,
    pub xs_japanese_line_height_dip: Option<f32>,
    pub sm_size_dip: Option<f32>,
    pub sm_weight_min: Option<u16>,
    pub sm_weight_max: Option<u16>,
    pub sm_line_height_dip: Option<f32>,
    pub sm_japanese_line_height_dip: Option<f32>,
    pub body_size_dip: Option<f32>,
    pub body_weight_min: Option<u16>,
    pub body_weight_max: Option<u16>,
    pub body_line_height_dip: Option<f32>,
    pub body_japanese_line_height_dip: Option<f32>,
    pub body_strong_size_dip: Option<f32>,
    pub body_strong_weight_min: Option<u16>,
    pub body_strong_weight_max: Option<u16>,
    pub body_strong_line_height_dip: Option<f32>,
    pub body_strong_japanese_line_height_dip: Option<f32>,
    pub control_size_dip: Option<f32>,
    pub control_weight_min: Option<u16>,
    pub control_weight_max: Option<u16>,
    pub control_line_height_dip: Option<f32>,
    pub control_japanese_line_height_dip: Option<f32>,
    pub title_size_dip: Option<f32>,
    pub title_weight_min: Option<u16>,
    pub title_weight_max: Option<u16>,
    pub title_line_height_dip: Option<f32>,
    pub title_japanese_line_height_dip: Option<f32>,
    pub display_size_dip: Option<f32>,
    pub display_weight_min: Option<u16>,
    pub display_weight_max: Option<u16>,
    pub display_line_height_dip: Option<f32>,
    pub display_japanese_line_height_dip: Option<f32>,
    pub spacing_4_dip: Option<f32>,
    pub spacing_8_dip: Option<f32>,
    pub spacing_12_dip: Option<f32>,
    pub spacing_16_dip: Option<f32>,
    pub spacing_20_dip: Option<f32>,
    pub spacing_24_dip: Option<f32>,
    pub spacing_32_dip: Option<f32>,
    pub spacing_40_dip: Option<f32>,
    pub spacing_48_dip: Option<f32>,
    pub row_dense_dip: Option<f32>,
    pub row_comfortable_dip: Option<f32>,
    pub title_bar_dip: Option<f32>,
    pub task_rail_dip: Option<f32>,
    pub inspector_dip: Option<f32>,
    pub thumbnail_dip: Option<f32>,
    pub preview_dip: Option<f32>,
    pub min_control_target_dip: Option<f32>,
    pub radius_none_dip: Option<f32>,
    pub radius_small_dip: Option<f32>,
    pub radius_large_dip: Option<f32>,
    pub separator_physical_pixels: Option<f32>,
    pub focus_outline_dip: Option<f32>,
}

impl DesignTokensOverride {
    /// Resolve this override over `base` without mutating either side.
    ///
    /// Pure: the result depends only on these two values. An unworkable
    /// resolved set is refused with [`TokenError`], never clamped.
    pub fn resolve(self, base: DesignTokens) -> Result<DesignTokens, TokenError> {
        let DesignTokens {
            typography,
            geometry,
        } = base;
        let resolved = DesignTokens {
            typography: Typography {
                latin_ui: self.latin_ui.unwrap_or(typography.latin_ui),
                latin_ui_fallback: self
                    .latin_ui_fallback
                    .unwrap_or(typography.latin_ui_fallback),
                japanese_ui: self.japanese_ui.unwrap_or(typography.japanese_ui),
                japanese_ui_fallback: self
                    .japanese_ui_fallback
                    .unwrap_or(typography.japanese_ui_fallback),
                monospace: self.monospace.unwrap_or(typography.monospace),
                monospace_fallback: self
                    .monospace_fallback
                    .unwrap_or(typography.monospace_fallback),
                tabular_numerals: self.tabular_numerals.unwrap_or(typography.tabular_numerals),
                xs_size_dip: self.xs_size_dip.unwrap_or(typography.xs_size_dip),
                xs_weight_min: self.xs_weight_min.unwrap_or(typography.xs_weight_min),
                xs_weight_max: self.xs_weight_max.unwrap_or(typography.xs_weight_max),
                xs_line_height_dip: self
                    .xs_line_height_dip
                    .unwrap_or(typography.xs_line_height_dip),
                xs_japanese_line_height_dip: self
                    .xs_japanese_line_height_dip
                    .unwrap_or(typography.xs_japanese_line_height_dip),
                sm_size_dip: self.sm_size_dip.unwrap_or(typography.sm_size_dip),
                sm_weight_min: self.sm_weight_min.unwrap_or(typography.sm_weight_min),
                sm_weight_max: self.sm_weight_max.unwrap_or(typography.sm_weight_max),
                sm_line_height_dip: self
                    .sm_line_height_dip
                    .unwrap_or(typography.sm_line_height_dip),
                sm_japanese_line_height_dip: self
                    .sm_japanese_line_height_dip
                    .unwrap_or(typography.sm_japanese_line_height_dip),
                body_size_dip: self.body_size_dip.unwrap_or(typography.body_size_dip),
                body_weight_min: self.body_weight_min.unwrap_or(typography.body_weight_min),
                body_weight_max: self.body_weight_max.unwrap_or(typography.body_weight_max),
                body_line_height_dip: self
                    .body_line_height_dip
                    .unwrap_or(typography.body_line_height_dip),
                body_japanese_line_height_dip: self
                    .body_japanese_line_height_dip
                    .unwrap_or(typography.body_japanese_line_height_dip),
                body_strong_size_dip: self
                    .body_strong_size_dip
                    .unwrap_or(typography.body_strong_size_dip),
                body_strong_weight_min: self
                    .body_strong_weight_min
                    .unwrap_or(typography.body_strong_weight_min),
                body_strong_weight_max: self
                    .body_strong_weight_max
                    .unwrap_or(typography.body_strong_weight_max),
                body_strong_line_height_dip: self
                    .body_strong_line_height_dip
                    .unwrap_or(typography.body_strong_line_height_dip),
                body_strong_japanese_line_height_dip: self
                    .body_strong_japanese_line_height_dip
                    .unwrap_or(typography.body_strong_japanese_line_height_dip),
                control_size_dip: self.control_size_dip.unwrap_or(typography.control_size_dip),
                control_weight_min: self
                    .control_weight_min
                    .unwrap_or(typography.control_weight_min),
                control_weight_max: self
                    .control_weight_max
                    .unwrap_or(typography.control_weight_max),
                control_line_height_dip: self
                    .control_line_height_dip
                    .unwrap_or(typography.control_line_height_dip),
                control_japanese_line_height_dip: self
                    .control_japanese_line_height_dip
                    .unwrap_or(typography.control_japanese_line_height_dip),
                title_size_dip: self.title_size_dip.unwrap_or(typography.title_size_dip),
                title_weight_min: self.title_weight_min.unwrap_or(typography.title_weight_min),
                title_weight_max: self.title_weight_max.unwrap_or(typography.title_weight_max),
                title_line_height_dip: self
                    .title_line_height_dip
                    .unwrap_or(typography.title_line_height_dip),
                title_japanese_line_height_dip: self
                    .title_japanese_line_height_dip
                    .unwrap_or(typography.title_japanese_line_height_dip),
                display_size_dip: self.display_size_dip.unwrap_or(typography.display_size_dip),
                display_weight_min: self
                    .display_weight_min
                    .unwrap_or(typography.display_weight_min),
                display_weight_max: self
                    .display_weight_max
                    .unwrap_or(typography.display_weight_max),
                display_line_height_dip: self
                    .display_line_height_dip
                    .unwrap_or(typography.display_line_height_dip),
                display_japanese_line_height_dip: self
                    .display_japanese_line_height_dip
                    .unwrap_or(typography.display_japanese_line_height_dip),
            },
            geometry: Geometry {
                spacing_4_dip: self.spacing_4_dip.unwrap_or(geometry.spacing_4_dip),
                spacing_8_dip: self.spacing_8_dip.unwrap_or(geometry.spacing_8_dip),
                spacing_12_dip: self.spacing_12_dip.unwrap_or(geometry.spacing_12_dip),
                spacing_16_dip: self.spacing_16_dip.unwrap_or(geometry.spacing_16_dip),
                spacing_20_dip: self.spacing_20_dip.unwrap_or(geometry.spacing_20_dip),
                spacing_24_dip: self.spacing_24_dip.unwrap_or(geometry.spacing_24_dip),
                spacing_32_dip: self.spacing_32_dip.unwrap_or(geometry.spacing_32_dip),
                spacing_40_dip: self.spacing_40_dip.unwrap_or(geometry.spacing_40_dip),
                spacing_48_dip: self.spacing_48_dip.unwrap_or(geometry.spacing_48_dip),
                row_dense_dip: self.row_dense_dip.unwrap_or(geometry.row_dense_dip),
                row_comfortable_dip: self
                    .row_comfortable_dip
                    .unwrap_or(geometry.row_comfortable_dip),
                title_bar_dip: self.title_bar_dip.unwrap_or(geometry.title_bar_dip),
                task_rail_dip: self.task_rail_dip.unwrap_or(geometry.task_rail_dip),
                inspector_dip: self.inspector_dip.unwrap_or(geometry.inspector_dip),
                thumbnail_dip: self.thumbnail_dip.unwrap_or(geometry.thumbnail_dip),
                preview_dip: self.preview_dip.unwrap_or(geometry.preview_dip),
                min_control_target_dip: self
                    .min_control_target_dip
                    .unwrap_or(geometry.min_control_target_dip),
                radius_none_dip: self.radius_none_dip.unwrap_or(geometry.radius_none_dip),
                radius_small_dip: self.radius_small_dip.unwrap_or(geometry.radius_small_dip),
                radius_large_dip: self.radius_large_dip.unwrap_or(geometry.radius_large_dip),
                separator_physical_pixels: self
                    .separator_physical_pixels
                    .unwrap_or(geometry.separator_physical_pixels),
                focus_outline_dip: self.focus_outline_dip.unwrap_or(geometry.focus_outline_dip),
            },
        };
        resolved.validate()?;
        Ok(resolved)
    }

    /// Resolve this override over [`DesignTokens::accepted`].
    pub fn resolve_accepted(self) -> Result<DesignTokens, TokenError> {
        self.resolve(DesignTokens::accepted())
    }
}

/// Closed refusal for an unworkable token set or scale factor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TokenError {
    TypeSizeNotPositive { style: TextStyle },
    WeightOutOfRange { style: TextStyle, weight: u16 },
    WeightRangeInverted { style: TextStyle },
    LineHeightBelowTypeSize { style: TextStyle },
    JapaneseLineHeightBelowLineHeight { style: TextStyle },
    FamilyNameEmpty { family: FamilyToken },
    FamilyNameUntrimmed { family: FamilyToken },
    FamilyNameContainsControl { family: FamilyToken },
    NonPositiveDimension { dimension: Dimension },
    NegativeRadius { dimension: Dimension },
    RadiiNotAscending,
    SpacingNotAscending,
    RowHeightsInverted,
    PreviewSmallerThanThumbnail,
    FocusOutlineBelowMinimum { width_dip: f32 },
    ControlTargetBelowMinimum { width_dip: f32 },
    NonPositiveScaleFactor { scale_factor: f32 },
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TypeSizeNotPositive { style } => write!(
                formatter,
                "the {style} size must be a positive finite DIP value"
            ),
            Self::WeightOutOfRange { style, weight } => write!(
                formatter,
                "the {style} weight {weight} is outside the CSS-like weight range 1..=1000"
            ),
            Self::WeightRangeInverted { style } => {
                write!(formatter, "the {style} weight range is inverted")
            }
            Self::LineHeightBelowTypeSize { style } => write!(
                formatter,
                "the {style} line height is below its own size and would clip"
            ),
            Self::JapaneseLineHeightBelowLineHeight { style } => write!(
                formatter,
                "the {style} Japanese fallback line height is below its Latin line height"
            ),
            Self::FamilyNameEmpty { family } => write!(
                formatter,
                "the {family} font family declaration must not be empty or whitespace-only"
            ),
            Self::FamilyNameUntrimmed { family } => write!(
                formatter,
                "the {family} font family declaration must not carry leading or trailing whitespace"
            ),
            Self::FamilyNameContainsControl { family } => write!(
                formatter,
                "the {family} font family declaration must not contain control characters"
            ),
            Self::NonPositiveDimension { dimension } => write!(
                formatter,
                "the {dimension} token must be a positive finite DIP value"
            ),
            Self::NegativeRadius { dimension } => {
                write!(formatter, "the {dimension} token must not be negative")
            }
            Self::RadiiNotAscending => {
                formatter.write_str("the radius scale must be non-negative and non-decreasing")
            }
            Self::SpacingNotAscending => {
                formatter.write_str("the 4-DIP spacing scale must be strictly ascending")
            }
            Self::RowHeightsInverted => formatter
                .write_str("the dense row height must not exceed the comfortable row height"),
            Self::PreviewSmallerThanThumbnail => formatter
                .write_str("the high-resolution preview must not be smaller than a thumbnail"),
            Self::FocusOutlineBelowMinimum { width_dip } => write!(
                formatter,
                "the {width_dip} DIP focus outline is thinner than the {FOCUS_OUTLINE_DIP} DIP minimum; the structural hairline is not a focus indicator"
            ),
            Self::ControlTargetBelowMinimum { width_dip } => write!(
                formatter,
                "the {width_dip} DIP control target is below the {MIN_CONTROL_TARGET_DIP} DIP accessible minimum"
            ),
            Self::NonPositiveScaleFactor { scale_factor } => write!(
                formatter,
                "the monitor scale factor {scale_factor} must be positive and finite"
            ),
        }
    }
}

impl std::error::Error for TokenError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// One override setter plus the leaf it must move.
    type OverrideCase = (&'static str, fn(&mut DesignTokensOverride));

    /// One override setter plus the refusal it must produce.
    type RefusalCase = (&'static str, fn(&mut DesignTokensOverride), TokenError);

    /// Every leaf whose resolved value differs from `left` to `right`.
    fn differing_leaves(left: &DesignTokens, right: &DesignTokens) -> Vec<&'static str> {
        let mut differing = Vec::new();
        if left.typography.latin_ui != right.typography.latin_ui {
            differing.push("typography.latin_ui");
        }
        if left.typography.latin_ui_fallback != right.typography.latin_ui_fallback {
            differing.push("typography.latin_ui_fallback");
        }
        if left.typography.japanese_ui != right.typography.japanese_ui {
            differing.push("typography.japanese_ui");
        }
        if left.typography.japanese_ui_fallback != right.typography.japanese_ui_fallback {
            differing.push("typography.japanese_ui_fallback");
        }
        if left.typography.monospace != right.typography.monospace {
            differing.push("typography.monospace");
        }
        if left.typography.monospace_fallback != right.typography.monospace_fallback {
            differing.push("typography.monospace_fallback");
        }
        if left.typography.tabular_numerals != right.typography.tabular_numerals {
            differing.push("typography.tabular_numerals");
        }
        if left.typography.xs_size_dip != right.typography.xs_size_dip {
            differing.push("typography.xs_size_dip");
        }
        if left.typography.xs_weight_min != right.typography.xs_weight_min {
            differing.push("typography.xs_weight_min");
        }
        if left.typography.xs_weight_max != right.typography.xs_weight_max {
            differing.push("typography.xs_weight_max");
        }
        if left.typography.xs_line_height_dip != right.typography.xs_line_height_dip {
            differing.push("typography.xs_line_height_dip");
        }
        if left.typography.xs_japanese_line_height_dip
            != right.typography.xs_japanese_line_height_dip
        {
            differing.push("typography.xs_japanese_line_height_dip");
        }
        if left.typography.sm_size_dip != right.typography.sm_size_dip {
            differing.push("typography.sm_size_dip");
        }
        if left.typography.sm_weight_min != right.typography.sm_weight_min {
            differing.push("typography.sm_weight_min");
        }
        if left.typography.sm_weight_max != right.typography.sm_weight_max {
            differing.push("typography.sm_weight_max");
        }
        if left.typography.sm_line_height_dip != right.typography.sm_line_height_dip {
            differing.push("typography.sm_line_height_dip");
        }
        if left.typography.sm_japanese_line_height_dip
            != right.typography.sm_japanese_line_height_dip
        {
            differing.push("typography.sm_japanese_line_height_dip");
        }
        if left.typography.body_size_dip != right.typography.body_size_dip {
            differing.push("typography.body_size_dip");
        }
        if left.typography.body_weight_min != right.typography.body_weight_min {
            differing.push("typography.body_weight_min");
        }
        if left.typography.body_weight_max != right.typography.body_weight_max {
            differing.push("typography.body_weight_max");
        }
        if left.typography.body_line_height_dip != right.typography.body_line_height_dip {
            differing.push("typography.body_line_height_dip");
        }
        if left.typography.body_japanese_line_height_dip
            != right.typography.body_japanese_line_height_dip
        {
            differing.push("typography.body_japanese_line_height_dip");
        }
        if left.typography.body_strong_size_dip != right.typography.body_strong_size_dip {
            differing.push("typography.body_strong_size_dip");
        }
        if left.typography.body_strong_weight_min != right.typography.body_strong_weight_min {
            differing.push("typography.body_strong_weight_min");
        }
        if left.typography.body_strong_weight_max != right.typography.body_strong_weight_max {
            differing.push("typography.body_strong_weight_max");
        }
        if left.typography.body_strong_line_height_dip
            != right.typography.body_strong_line_height_dip
        {
            differing.push("typography.body_strong_line_height_dip");
        }
        if left.typography.body_strong_japanese_line_height_dip
            != right.typography.body_strong_japanese_line_height_dip
        {
            differing.push("typography.body_strong_japanese_line_height_dip");
        }
        if left.typography.control_size_dip != right.typography.control_size_dip {
            differing.push("typography.control_size_dip");
        }
        if left.typography.control_weight_min != right.typography.control_weight_min {
            differing.push("typography.control_weight_min");
        }
        if left.typography.control_weight_max != right.typography.control_weight_max {
            differing.push("typography.control_weight_max");
        }
        if left.typography.control_line_height_dip != right.typography.control_line_height_dip {
            differing.push("typography.control_line_height_dip");
        }
        if left.typography.control_japanese_line_height_dip
            != right.typography.control_japanese_line_height_dip
        {
            differing.push("typography.control_japanese_line_height_dip");
        }
        if left.typography.title_size_dip != right.typography.title_size_dip {
            differing.push("typography.title_size_dip");
        }
        if left.typography.title_weight_min != right.typography.title_weight_min {
            differing.push("typography.title_weight_min");
        }
        if left.typography.title_weight_max != right.typography.title_weight_max {
            differing.push("typography.title_weight_max");
        }
        if left.typography.title_line_height_dip != right.typography.title_line_height_dip {
            differing.push("typography.title_line_height_dip");
        }
        if left.typography.title_japanese_line_height_dip
            != right.typography.title_japanese_line_height_dip
        {
            differing.push("typography.title_japanese_line_height_dip");
        }
        if left.typography.display_size_dip != right.typography.display_size_dip {
            differing.push("typography.display_size_dip");
        }
        if left.typography.display_weight_min != right.typography.display_weight_min {
            differing.push("typography.display_weight_min");
        }
        if left.typography.display_weight_max != right.typography.display_weight_max {
            differing.push("typography.display_weight_max");
        }
        if left.typography.display_line_height_dip != right.typography.display_line_height_dip {
            differing.push("typography.display_line_height_dip");
        }
        if left.typography.display_japanese_line_height_dip
            != right.typography.display_japanese_line_height_dip
        {
            differing.push("typography.display_japanese_line_height_dip");
        }
        if left.geometry.spacing_4_dip != right.geometry.spacing_4_dip {
            differing.push("geometry.spacing_4_dip");
        }
        if left.geometry.spacing_8_dip != right.geometry.spacing_8_dip {
            differing.push("geometry.spacing_8_dip");
        }
        if left.geometry.spacing_12_dip != right.geometry.spacing_12_dip {
            differing.push("geometry.spacing_12_dip");
        }
        if left.geometry.spacing_16_dip != right.geometry.spacing_16_dip {
            differing.push("geometry.spacing_16_dip");
        }
        if left.geometry.spacing_20_dip != right.geometry.spacing_20_dip {
            differing.push("geometry.spacing_20_dip");
        }
        if left.geometry.spacing_24_dip != right.geometry.spacing_24_dip {
            differing.push("geometry.spacing_24_dip");
        }
        if left.geometry.spacing_32_dip != right.geometry.spacing_32_dip {
            differing.push("geometry.spacing_32_dip");
        }
        if left.geometry.spacing_40_dip != right.geometry.spacing_40_dip {
            differing.push("geometry.spacing_40_dip");
        }
        if left.geometry.spacing_48_dip != right.geometry.spacing_48_dip {
            differing.push("geometry.spacing_48_dip");
        }
        if left.geometry.row_dense_dip != right.geometry.row_dense_dip {
            differing.push("geometry.row_dense_dip");
        }
        if left.geometry.row_comfortable_dip != right.geometry.row_comfortable_dip {
            differing.push("geometry.row_comfortable_dip");
        }
        if left.geometry.title_bar_dip != right.geometry.title_bar_dip {
            differing.push("geometry.title_bar_dip");
        }
        if left.geometry.task_rail_dip != right.geometry.task_rail_dip {
            differing.push("geometry.task_rail_dip");
        }
        if left.geometry.inspector_dip != right.geometry.inspector_dip {
            differing.push("geometry.inspector_dip");
        }
        if left.geometry.thumbnail_dip != right.geometry.thumbnail_dip {
            differing.push("geometry.thumbnail_dip");
        }
        if left.geometry.preview_dip != right.geometry.preview_dip {
            differing.push("geometry.preview_dip");
        }
        if left.geometry.min_control_target_dip != right.geometry.min_control_target_dip {
            differing.push("geometry.min_control_target_dip");
        }
        if left.geometry.radius_none_dip != right.geometry.radius_none_dip {
            differing.push("geometry.radius_none_dip");
        }
        if left.geometry.radius_small_dip != right.geometry.radius_small_dip {
            differing.push("geometry.radius_small_dip");
        }
        if left.geometry.radius_large_dip != right.geometry.radius_large_dip {
            differing.push("geometry.radius_large_dip");
        }
        if left.geometry.separator_physical_pixels != right.geometry.separator_physical_pixels {
            differing.push("geometry.separator_physical_pixels");
        }
        if left.geometry.focus_outline_dip != right.geometry.focus_outline_dip {
            differing.push("geometry.focus_outline_dip");
        }
        differing
    }

    fn override_cases() -> Vec<OverrideCase> {
        vec![
            ("typography.latin_ui", |overrides| {
                overrides.latin_ui = Some(Cow::Borrowed("Test UI Family"))
            }),
            ("typography.latin_ui_fallback", |overrides| {
                overrides.latin_ui_fallback = Some(Cow::Borrowed("Test UI Family"))
            }),
            ("typography.japanese_ui", |overrides| {
                overrides.japanese_ui = Some(Cow::Borrowed("Test UI Family"))
            }),
            ("typography.japanese_ui_fallback", |overrides| {
                overrides.japanese_ui_fallback = Some(Cow::Borrowed("Test UI Family"))
            }),
            ("typography.monospace", |overrides| {
                overrides.monospace = Some(Cow::Borrowed("Test UI Family"))
            }),
            ("typography.monospace_fallback", |overrides| {
                overrides.monospace_fallback = Some(Cow::Borrowed("Test UI Family"))
            }),
            ("typography.tabular_numerals", |overrides| {
                overrides.tabular_numerals = Some(false)
            }),
            ("typography.xs_size_dip", |overrides| {
                overrides.xs_size_dip = Some(11.0)
            }),
            ("typography.xs_weight_min", |overrides| {
                overrides.xs_weight_min = Some(300)
            }),
            ("typography.xs_weight_max", |overrides| {
                overrides.xs_weight_max = Some(600)
            }),
            ("typography.xs_line_height_dip", |overrides| {
                overrides.xs_line_height_dip = Some(15.0)
            }),
            ("typography.xs_japanese_line_height_dip", |overrides| {
                overrides.xs_japanese_line_height_dip = Some(19.0)
            }),
            ("typography.sm_size_dip", |overrides| {
                overrides.sm_size_dip = Some(12.0)
            }),
            ("typography.sm_weight_min", |overrides| {
                overrides.sm_weight_min = Some(300)
            }),
            ("typography.sm_weight_max", |overrides| {
                overrides.sm_weight_max = Some(600)
            }),
            ("typography.sm_line_height_dip", |overrides| {
                overrides.sm_line_height_dip = Some(17.0)
            }),
            ("typography.sm_japanese_line_height_dip", |overrides| {
                overrides.sm_japanese_line_height_dip = Some(21.0)
            }),
            ("typography.body_size_dip", |overrides| {
                overrides.body_size_dip = Some(13.0)
            }),
            ("typography.body_weight_min", |overrides| {
                overrides.body_weight_min = Some(300)
            }),
            ("typography.body_weight_max", |overrides| {
                overrides.body_weight_max = Some(500)
            }),
            ("typography.body_line_height_dip", |overrides| {
                overrides.body_line_height_dip = Some(19.0)
            }),
            ("typography.body_japanese_line_height_dip", |overrides| {
                overrides.body_japanese_line_height_dip = Some(22.0)
            }),
            ("typography.body_strong_size_dip", |overrides| {
                overrides.body_strong_size_dip = Some(13.0)
            }),
            ("typography.body_strong_weight_min", |overrides| {
                overrides.body_strong_weight_min = Some(500)
            }),
            ("typography.body_strong_weight_max", |overrides| {
                overrides.body_strong_weight_max = Some(700)
            }),
            ("typography.body_strong_line_height_dip", |overrides| {
                overrides.body_strong_line_height_dip = Some(19.0)
            }),
            (
                "typography.body_strong_japanese_line_height_dip",
                |overrides| overrides.body_strong_japanese_line_height_dip = Some(22.0),
            ),
            ("typography.control_size_dip", |overrides| {
                overrides.control_size_dip = Some(15.0)
            }),
            ("typography.control_weight_min", |overrides| {
                overrides.control_weight_min = Some(300)
            }),
            ("typography.control_weight_max", |overrides| {
                overrides.control_weight_max = Some(600)
            }),
            ("typography.control_line_height_dip", |overrides| {
                overrides.control_line_height_dip = Some(23.0)
            }),
            ("typography.control_japanese_line_height_dip", |overrides| {
                overrides.control_japanese_line_height_dip = Some(25.0)
            }),
            ("typography.title_size_dip", |overrides| {
                overrides.title_size_dip = Some(19.0)
            }),
            ("typography.title_weight_min", |overrides| {
                overrides.title_weight_min = Some(500)
            }),
            ("typography.title_weight_max", |overrides| {
                overrides.title_weight_max = Some(700)
            }),
            ("typography.title_line_height_dip", |overrides| {
                overrides.title_line_height_dip = Some(27.0)
            }),
            ("typography.title_japanese_line_height_dip", |overrides| {
                overrides.title_japanese_line_height_dip = Some(31.0)
            }),
            ("typography.display_size_dip", |overrides| {
                overrides.display_size_dip = Some(23.0)
            }),
            ("typography.display_weight_min", |overrides| {
                overrides.display_weight_min = Some(500)
            }),
            ("typography.display_weight_max", |overrides| {
                overrides.display_weight_max = Some(700)
            }),
            ("typography.display_line_height_dip", |overrides| {
                overrides.display_line_height_dip = Some(31.0)
            }),
            ("typography.display_japanese_line_height_dip", |overrides| {
                overrides.display_japanese_line_height_dip = Some(37.0)
            }),
            ("geometry.spacing_4_dip", |overrides| {
                overrides.spacing_4_dip = Some(4.5)
            }),
            ("geometry.spacing_8_dip", |overrides| {
                overrides.spacing_8_dip = Some(8.5)
            }),
            ("geometry.spacing_12_dip", |overrides| {
                overrides.spacing_12_dip = Some(12.5)
            }),
            ("geometry.spacing_16_dip", |overrides| {
                overrides.spacing_16_dip = Some(16.5)
            }),
            ("geometry.spacing_20_dip", |overrides| {
                overrides.spacing_20_dip = Some(20.5)
            }),
            ("geometry.spacing_24_dip", |overrides| {
                overrides.spacing_24_dip = Some(24.5)
            }),
            ("geometry.spacing_32_dip", |overrides| {
                overrides.spacing_32_dip = Some(32.5)
            }),
            ("geometry.spacing_40_dip", |overrides| {
                overrides.spacing_40_dip = Some(40.5)
            }),
            ("geometry.spacing_48_dip", |overrides| {
                overrides.spacing_48_dip = Some(48.5)
            }),
            ("geometry.row_dense_dip", |overrides| {
                overrides.row_dense_dip = Some(33.0)
            }),
            ("geometry.row_comfortable_dip", |overrides| {
                overrides.row_comfortable_dip = Some(41.0)
            }),
            ("geometry.title_bar_dip", |overrides| {
                overrides.title_bar_dip = Some(41.0)
            }),
            ("geometry.task_rail_dip", |overrides| {
                overrides.task_rail_dip = Some(241.0)
            }),
            ("geometry.inspector_dip", |overrides| {
                overrides.inspector_dip = Some(321.0)
            }),
            ("geometry.thumbnail_dip", |overrides| {
                overrides.thumbnail_dip = Some(73.0)
            }),
            ("geometry.preview_dip", |overrides| {
                overrides.preview_dip = Some(97.0)
            }),
            ("geometry.min_control_target_dip", |overrides| {
                overrides.min_control_target_dip = Some(25.0)
            }),
            ("geometry.radius_none_dip", |overrides| {
                overrides.radius_none_dip = Some(1.0)
            }),
            ("geometry.radius_small_dip", |overrides| {
                overrides.radius_small_dip = Some(5.0)
            }),
            ("geometry.radius_large_dip", |overrides| {
                overrides.radius_large_dip = Some(9.0)
            }),
            ("geometry.separator_physical_pixels", |overrides| {
                overrides.separator_physical_pixels = Some(2.0)
            }),
            ("geometry.focus_outline_dip", |overrides| {
                overrides.focus_outline_dip = Some(3.0)
            }),
        ]
    }

    #[test]
    fn accepted_tokens_resolve_to_the_ratified_values() {
        let tokens = DesignTokens::accepted();
        assert_eq!(tokens, DesignTokens::default());
        assert!(tokens.validate().is_ok());

        // Font stack: Segoe UI Variable -> Segoe UI, Yu Gothic UI -> Meiryo,
        // Cascadia Mono -> Consolas (rendering §9.1).
        assert_eq!(tokens.typography.latin_ui.as_ref(), "Segoe UI Variable");
        assert_eq!(tokens.typography.latin_ui_fallback.as_ref(), "Segoe UI");
        assert_eq!(tokens.typography.japanese_ui.as_ref(), "Yu Gothic UI");
        assert_eq!(tokens.typography.japanese_ui_fallback.as_ref(), "Meiryo");
        assert_eq!(tokens.typography.monospace.as_ref(), "Cascadia Mono");
        assert_eq!(tokens.typography.monospace_fallback.as_ref(), "Consolas");
        assert!(tokens.typography.tabular_numerals);

        // Type scale: size, weight range, line height, Japanese line height.
        let type_scale = [
            (TextStyle::Xs, "type-xs", 12.0, 400, 500, 16.0, 18.0),
            (TextStyle::Sm, "type-sm", 13.0, 400, 500, 18.0, 20.0),
            (TextStyle::Body, "type-body", 14.0, 400, 400, 20.0, 21.0),
            (
                TextStyle::BodyStrong,
                "type-body-strong",
                14.0,
                600,
                600,
                20.0,
                21.0,
            ),
            (
                TextStyle::Control,
                "type-control",
                16.0,
                400,
                500,
                24.0,
                24.0,
            ),
            (TextStyle::Title, "type-title", 20.0, 600, 600, 28.0, 30.0),
            (
                TextStyle::Display,
                "type-display",
                24.0,
                600,
                600,
                32.0,
                36.0,
            ),
        ];
        for (style, name, size, weight_min, weight_max, line_height, japanese) in type_scale {
            let metrics = tokens.typography.metrics(style);
            assert_eq!(metrics.size_dip, size, "{name} size");
            assert_eq!(metrics.weight_min, weight_min, "{name} lightest weight");
            assert_eq!(metrics.weight_max, weight_max, "{name} heaviest weight");
            assert_eq!(metrics.line_height_dip, line_height, "{name} line height");
            assert_eq!(
                metrics.japanese_line_height_dip, japanese,
                "{name} Japanese line height"
            );
        }

        // 4-DIP spacing scale (rendering §9.2).
        let geometry = &tokens.geometry;
        assert_eq!(
            [
                geometry.spacing_4_dip,
                geometry.spacing_8_dip,
                geometry.spacing_12_dip,
                geometry.spacing_16_dip,
                geometry.spacing_20_dip,
                geometry.spacing_24_dip,
                geometry.spacing_32_dip,
                geometry.spacing_40_dip,
                geometry.spacing_48_dip,
            ],
            [4.0, 8.0, 12.0, 16.0, 20.0, 24.0, 32.0, 40.0, 48.0]
        );
        assert_eq!(
            [geometry.row_dense_dip, geometry.row_comfortable_dip],
            [32.0, 40.0]
        );
        assert_eq!(geometry.title_bar_dip, 40.0);
        assert_eq!(geometry.task_rail_dip, 240.0);
        assert_eq!(geometry.inspector_dip, 320.0);
        assert_eq!([geometry.thumbnail_dip, geometry.preview_dip], [72.0, 96.0]);
        assert_eq!(geometry.min_control_target_dip, 24.0);
        assert_eq!(
            [
                geometry.radius_none_dip,
                geometry.radius_small_dip,
                geometry.radius_large_dip,
            ],
            [0.0, 4.0, 8.0]
        );
        assert_eq!(geometry.separator_physical_pixels, 1.0);
        assert_eq!(geometry.focus_outline_dip, FOCUS_OUTLINE_DIP);
        assert_eq!(geometry.focus_outline_dip, 2.0);
    }

    #[test]
    fn every_override_is_honoured_and_touches_exactly_one_leaf() {
        let base = DesignTokens::accepted();
        for (leaf, set) in override_cases() {
            let mut overrides = DesignTokensOverride::default();
            set(&mut overrides);
            let resolved = overrides
                .resolve(base.clone())
                .expect("a single valid override resolves");
            assert_eq!(differing_leaves(&base, &resolved), [leaf], "{leaf}");
        }
        // An empty override is the accepted set, so no leaf moves.
        let accepted_again = DesignTokensOverride::default()
            .resolve_accepted()
            .expect("the accepted set validates");
        assert!(differing_leaves(&base, &accepted_again).is_empty());
    }

    #[test]
    fn unworkable_token_values_are_refused_with_a_closed_error() {
        let base = DesignTokens::accepted();
        let cases: [RefusalCase; 21] = [
            (
                "zero xs size",
                |overrides| overrides.xs_size_dip = Some(0.0),
                TokenError::TypeSizeNotPositive {
                    style: TextStyle::Xs,
                },
            ),
            (
                "negative display size",
                |overrides| overrides.display_size_dip = Some(-1.0),
                TokenError::TypeSizeNotPositive {
                    style: TextStyle::Display,
                },
            ),
            (
                "non-finite title size",
                |overrides| overrides.title_size_dip = Some(f32::NAN),
                TokenError::TypeSizeNotPositive {
                    style: TextStyle::Title,
                },
            ),
            (
                "weight below the CSS-like range",
                |overrides| overrides.body_weight_min = Some(0),
                TokenError::WeightOutOfRange {
                    style: TextStyle::Body,
                    weight: 0,
                },
            ),
            (
                "weight above the CSS-like range",
                |overrides| overrides.body_weight_max = Some(1001),
                TokenError::WeightOutOfRange {
                    style: TextStyle::Body,
                    weight: 1001,
                },
            ),
            (
                "inverted weight range",
                |overrides| overrides.body_weight_min = Some(500),
                TokenError::WeightRangeInverted {
                    style: TextStyle::Body,
                },
            ),
            (
                "line height below the type size",
                |overrides| overrides.body_line_height_dip = Some(13.0),
                TokenError::LineHeightBelowTypeSize {
                    style: TextStyle::Body,
                },
            ),
            (
                "Japanese line height below the Latin line height",
                |overrides| overrides.body_japanese_line_height_dip = Some(19.0),
                TokenError::JapaneseLineHeightBelowLineHeight {
                    style: TextStyle::Body,
                },
            ),
            (
                "zero dense row height",
                |overrides| overrides.row_dense_dip = Some(0.0),
                TokenError::NonPositiveDimension {
                    dimension: Dimension::RowDense,
                },
            ),
            (
                "negative spacing step",
                |overrides| overrides.spacing_8_dip = Some(-4.0),
                TokenError::NonPositiveDimension {
                    dimension: Dimension::Spacing8,
                },
            ),
            (
                "collapsed spacing scale",
                |overrides| overrides.spacing_12_dip = Some(8.0),
                TokenError::SpacingNotAscending,
            ),
            (
                "inverted row heights",
                |overrides| overrides.row_comfortable_dip = Some(31.0),
                TokenError::RowHeightsInverted,
            ),
            (
                "preview smaller than the thumbnail",
                |overrides| overrides.preview_dip = Some(70.0),
                TokenError::PreviewSmallerThanThumbnail,
            ),
            (
                "negative radius",
                |overrides| overrides.radius_small_dip = Some(-1.0),
                TokenError::NegativeRadius {
                    dimension: Dimension::RadiusSmall,
                },
            ),
            (
                "collapsed radius scale",
                |overrides| overrides.radius_large_dip = Some(2.0),
                TokenError::RadiiNotAscending,
            ),
            (
                "focus outline below the 2-DIP minimum",
                |overrides| overrides.focus_outline_dip = Some(1.5),
                TokenError::FocusOutlineBelowMinimum { width_dip: 1.5 },
            ),
            (
                "hairline-sized focus outline",
                |overrides| overrides.focus_outline_dip = Some(0.5),
                TokenError::FocusOutlineBelowMinimum { width_dip: 0.5 },
            ),
            (
                "control target below the 24-DIP minimum",
                |overrides| overrides.min_control_target_dip = Some(23.0),
                TokenError::ControlTargetBelowMinimum { width_dip: 23.0 },
            ),
            (
                "zero inspector width",
                |overrides| overrides.inspector_dip = Some(0.0),
                TokenError::NonPositiveDimension {
                    dimension: Dimension::Inspector,
                },
            ),
            (
                "non-finite title bar height",
                |overrides| overrides.title_bar_dip = Some(f32::NAN),
                TokenError::NonPositiveDimension {
                    dimension: Dimension::TitleBar,
                },
            ),
            (
                "zero physical separator",
                |overrides| overrides.separator_physical_pixels = Some(0.0),
                TokenError::NonPositiveDimension {
                    dimension: Dimension::SeparatorPhysicalPixels,
                },
            ),
        ];
        for (case, set, expected) in cases {
            let mut overrides = DesignTokensOverride::default();
            set(&mut overrides);
            assert_eq!(overrides.resolve(base.clone()), Err(expected), "{case}");
        }
        // The documented floors are exactly representable, so the boundary is
        // accepted rather than refused.
        let at_the_floor = DesignTokensOverride {
            focus_outline_dip: Some(2.0),
            min_control_target_dip: Some(24.0),
            ..DesignTokensOverride::default()
        }
        .resolve(base)
        .expect("the documented floors are valid");
        assert_eq!(at_the_floor.geometry.focus_outline_dip, 2.0);
        assert_eq!(at_the_floor.geometry.min_control_target_dip, 24.0);
    }

    #[test]
    fn one_physical_pixel_separator_is_a_reciprocal_scale_not_a_one_dip_line() {
        let geometry = DesignTokens::accepted().geometry;
        for scale_factor in [1.0_f32, 1.25, 1.5, 1.75, 2.0] {
            let dip = geometry
                .separator_dip(scale_factor)
                .expect("a positive finite scale factor is valid");
            assert_eq!(dip, geometry.separator_physical_pixels / scale_factor);
            if scale_factor > 1.0 {
                assert!(
                    dip < 1.0,
                    "a physical hairline is not a 1-DIP line at {scale_factor}"
                );
                assert_ne!(dip, 1.0);
            }
        }
        assert_eq!(geometry.separator_dip(1.0), Ok(1.0));
        assert_eq!(geometry.separator_dip(1.25), Ok(0.8));
        assert_eq!(geometry.separator_dip(1.5), Ok(1.0 / 1.5));
        assert_eq!(geometry.separator_dip(2.0), Ok(0.5));
        // A wider declared separator stays reciprocal instead of becoming a
        // fixed DIP line.
        let widened = DesignTokensOverride {
            separator_physical_pixels: Some(2.0),
            ..DesignTokensOverride::default()
        }
        .resolve_accepted()
        .expect("two physical pixels is a valid separator");
        assert_eq!(widened.geometry.separator_dip(2.0), Ok(1.0));
        assert_eq!(widened.geometry.separator_dip(1.5), Ok(2.0 / 1.5));
    }

    #[test]
    fn invalid_scale_factors_are_refused_rather_than_producing_a_broken_hairline() {
        let geometry = DesignTokens::accepted().geometry;
        for scale_factor in [0.0_f32, -1.0, f32::INFINITY] {
            assert_eq!(
                geometry.separator_dip(scale_factor),
                Err(TokenError::NonPositiveScaleFactor { scale_factor })
            );
        }
        assert!(matches!(
            geometry.separator_dip(f32::NAN),
            Err(TokenError::NonPositiveScaleFactor { .. })
        ));
    }

    #[test]
    fn the_focus_outline_is_independent_of_the_structural_hairline() {
        let tokens = DesignTokens::accepted();
        assert_eq!(tokens.geometry.focus_outline_dip, FOCUS_OUTLINE_DIP);
        // The hairline shrinks with the monitor scale; the focus outline does
        // not, so focus indication never collapses into a structural line.
        for scale_factor in [1.0_f32, 1.25, 1.5, 1.75, 2.0] {
            let hairline = tokens
                .geometry
                .separator_dip(scale_factor)
                .expect("a positive finite scale factor is valid");
            assert!(
                tokens.geometry.focus_outline_dip > hairline,
                "focus outline stays distinct at {scale_factor}"
            );
        }
        // Widening the separator never moves the focus outline.
        let widened = DesignTokensOverride {
            separator_physical_pixels: Some(2.0),
            ..DesignTokensOverride::default()
        }
        .resolve_accepted()
        .expect("two physical pixels is a valid separator");
        assert_eq!(widened.geometry.separator_dip(2.0), Ok(1.0));
        assert_eq!(
            widened.geometry.focus_outline_dip,
            tokens.geometry.focus_outline_dip
        );
        // A hairline-sized outline (0.5 DIP, the 200 % separator) is not a
        // focus indicator.
        assert_eq!(
            DesignTokensOverride {
                focus_outline_dip: Some(0.5),
                ..DesignTokensOverride::default()
            }
            .resolve_accepted(),
            Err(TokenError::FocusOutlineBelowMinimum { width_dip: 0.5 })
        );
    }

    #[test]
    fn golden_row_height_and_spacing_layout_expectations() {
        let tokens = DesignTokens::accepted();
        let geometry = &tokens.geometry;
        // The accepted scale, pinned exactly and in order.
        let scale = [
            geometry.spacing_4_dip,
            geometry.spacing_8_dip,
            geometry.spacing_12_dip,
            geometry.spacing_16_dip,
            geometry.spacing_20_dip,
            geometry.spacing_24_dip,
            geometry.spacing_32_dip,
            geometry.spacing_40_dip,
            geometry.spacing_48_dip,
        ];
        assert_eq!(scale, [4.0, 8.0, 12.0, 16.0, 20.0, 24.0, 32.0, 40.0, 48.0]);
        assert!(scale.windows(2).all(|pair| pair[0] < pair[1]));

        // Row heights and the documented relationships they express with the
        // scale. These are golden expectations of the accepted set, not
        // separately enforced invariants.
        assert_eq!(
            [geometry.row_dense_dip, geometry.row_comfortable_dip],
            [32.0, 40.0]
        );
        assert_eq!(
            geometry.row_comfortable_dip - geometry.row_dense_dip,
            geometry.spacing_8_dip
        );
        assert_eq!(geometry.title_bar_dip, geometry.row_comfortable_dip);
        assert_eq!(
            geometry.preview_dip - geometry.thumbnail_dip,
            geometry.spacing_24_dip
        );
        assert_eq!(geometry.task_rail_dip, geometry.spacing_48_dip * 5.0);
        assert_eq!(
            geometry.inspector_dip,
            geometry.spacing_48_dip * 6.0 + geometry.spacing_32_dip
        );

        // A deterministic stacked-row expectation computed only from the row
        // height and the spacing scale (no GPU, no window): three dense rows
        // separated by the smallest scale step are 32 * 3 + 4 * 2 = 104 DIP.
        let rows = 3.0_f32;
        let stacked = rows * geometry.row_dense_dip + (rows - 1.0) * geometry.spacing_4_dip;
        assert_eq!(stacked, 104.0);
        // The same expression with the comfortable row and a 12-DIP step.
        let comfortable =
            rows * geometry.row_comfortable_dip + (rows - 1.0) * geometry.spacing_12_dip;
        assert_eq!(comfortable, 144.0);
    }

    /// One family override setter paired with the token it addresses.
    type FamilySetter = (
        FamilyToken,
        fn(&mut DesignTokensOverride, Cow<'static, str>),
    );

    /// One unusable declaration paired with the error it must produce.
    type FamilyDefect = (Cow<'static, str>, fn(FamilyToken) -> TokenError);

    #[test]
    fn unusable_font_family_declarations_are_refused_for_every_field() {
        let accepted = DesignTokens::accepted();
        assert!(accepted.validate().is_ok());
        // The one-line resolution path still returns the accepted tokens.
        assert_eq!(
            DesignTokensOverride::default().resolve_accepted(),
            Ok(accepted.clone())
        );

        let setters: [FamilySetter; 6] = [
            (FamilyToken::LatinUi, |overrides, value| {
                overrides.latin_ui = Some(value);
            }),
            (FamilyToken::LatinUiFallback, |overrides, value| {
                overrides.latin_ui_fallback = Some(value);
            }),
            (FamilyToken::JapaneseUi, |overrides, value| {
                overrides.japanese_ui = Some(value);
            }),
            (FamilyToken::JapaneseUiFallback, |overrides, value| {
                overrides.japanese_ui_fallback = Some(value);
            }),
            (FamilyToken::Monospace, |overrides, value| {
                overrides.monospace = Some(value);
            }),
            (FamilyToken::MonospaceFallback, |overrides, value| {
                overrides.monospace_fallback = Some(value);
            }),
        ];
        let defects: [FamilyDefect; 6] = [
            (Cow::Borrowed(""), |family| TokenError::FamilyNameEmpty {
                family,
            }),
            (Cow::Borrowed("   "), |family| TokenError::FamilyNameEmpty {
                family,
            }),
            (Cow::Borrowed("\t\r\n"), |family| {
                TokenError::FamilyNameEmpty { family }
            }),
            (Cow::Borrowed(" Segoe UI"), |family| {
                TokenError::FamilyNameUntrimmed { family }
            }),
            (Cow::Borrowed("Segoe UI "), |family| {
                TokenError::FamilyNameUntrimmed { family }
            }),
            (Cow::Borrowed("Segoe\u{0}UI"), |family| {
                TokenError::FamilyNameContainsControl { family }
            }),
        ];
        for (family, set) in setters {
            for (value, expected) in defects.clone() {
                let mut overrides = DesignTokensOverride::default();
                set(&mut overrides, value);
                assert_eq!(
                    overrides.resolve_accepted(),
                    Err(expected(family)),
                    "{family}"
                );
            }
        }
        // A family is a declaration, never resolved here: a well-formed name
        // that happens not to be installed is accepted, and the platform
        // shaping layer owns the primary-then-fallback walk.
        let declared = DesignTokensOverride {
            japanese_ui: Some(Cow::Borrowed("Not Installed CJK Candidate")),
            ..DesignTokensOverride::default()
        }
        .resolve_accepted()
        .expect("a well-formed declaration is not resolved against installed fonts");
        assert_eq!(
            declared.typography.japanese_ui.as_ref(),
            "Not Installed CJK Candidate"
        );
    }
}
