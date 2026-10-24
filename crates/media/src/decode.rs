//! MP-IMG2 decode preflight arithmetic shared by decoder adapters.
//!
//! This admits declared dimensions/bit depth/scratch against caller budgets before a
//! decoder allocates. It selects no decoder, decodes nothing, owns no GPU object and
//! makes no format-support, profile, region or capability claim; those remain the
//! decoder owner's admission inputs.
//!
//! Budgets are configuration owned by this module, never a numeric literal at a use site.
//! [`DecodeBudget::ratified_default`] names the owner-ratified **declared provisional**
//! decode budget: every number in it is a declared ceiling on an **unmeasured basis** (the
//! Caller ratified "assumed appropriate values" on 2026-10-24), so it is a declared
//! assumption rather than a measurement, a peak-memory fact or an SLO-attainment claim.
//! Every field is replaceable through [`DecodeLimitsOverride`], so no caller has to edit a
//! constant, and an unusable replacement is refused with [`MediaError::InvalidLimits`]
//! instead of being clamped.

use crate::{Cancellation, MediaError, PixelSize};

/// Widest per-channel bit depth the preflight arithmetic models.
///
/// This is an arithmetic bound, **not** a decode budget: overriding
/// [`DecodeBudget::max_bit_depth`] narrows what is admitted but can never widen the
/// arithmetic beyond this value.
pub const MAX_SUPPORTED_BIT_DEPTH: u8 = 16;

/// Most channels the preflight arithmetic models (RGBA).
///
/// An arithmetic bound, not a decode budget; it is not overridable.
pub const MAX_SUPPORTED_CHANNELS: u8 = 4;

/// Caller-supplied decode budgets. No numeric SLO is implied; values are configuration,
/// conventionally resolved from [`DecodeBudget::ratified_default`] through
/// [`DecodeLimitsOverride`] rather than written inline at a call site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecodeLimits {
    pub max_dimension: u32,
    pub max_pixels: u64,
    pub max_bit_depth: u8,
    pub max_decoded_bytes: u64,
    pub max_scratch_bytes: u64,
}

impl DecodeLimits {
    fn validate(self) -> Result<Self, MediaError> {
        if self.max_dimension == 0
            || self.max_pixels == 0
            || !(1..=MAX_SUPPORTED_BIT_DEPTH).contains(&self.max_bit_depth)
            || self.max_decoded_bytes == 0
        {
            return Err(MediaError::InvalidLimits);
        }
        Ok(self)
    }
}

/// Owner-ratified **declared provisional** decode budget.
///
/// Every field is a declared ceiling on an **unmeasured basis**: the Caller ratified these
/// assumed values on 2026-10-24 ("assume appropriate values"), so they are not measured
/// budgets, not SLO attainment and not evidence of decoder behaviour. They exist so that no
/// consumer has to hardcode a limit: read [`Self::ratified_default`] (or the named
/// constants), and replace what you must through [`DecodeLimitsOverride`].
///
/// The pixel ceiling is the declared stress-input edge squared, the byte ceilings are
/// aligned to the ratified 256 MiB CPU decoded-image cache, and the bit depth is the widest
/// depth the preflight arithmetic models.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecodeBudget {
    pub max_dimension: u32,
    pub max_pixels: u64,
    pub max_bit_depth: u8,
    pub max_decoded_bytes: u64,
    pub max_scratch_bytes: u64,
}

impl DecodeBudget {
    /// Declared maximum edge, `16_384` pixels.
    pub const MAX_DIMENSION: u32 = 16_384;
    /// Declared pixel ceiling, `268_435_456` = `16_384²` (the declared stress input).
    pub const MAX_PIXELS: u64 = 268_435_456;
    /// Declared bit-depth ceiling, `16` bits per channel (the arithmetic bound).
    pub const MAX_BIT_DEPTH: u8 = MAX_SUPPORTED_BIT_DEPTH;
    /// Declared decoded-byte ceiling, `268_435_456` B (256 MiB), aligned with the ratified
    /// CPU decoded-image cache size.
    pub const MAX_DECODED_BYTES: u64 = 268_435_456;
    /// Declared scratch ceiling, `268_435_456` B (256 MiB).
    pub const MAX_SCRATCH_BYTES: u64 = 268_435_456;

    /// The owner-ratified declared provisional bundle (declared on an unmeasured basis).
    pub const fn ratified_default() -> Self {
        Self {
            max_dimension: Self::MAX_DIMENSION,
            max_pixels: Self::MAX_PIXELS,
            max_bit_depth: Self::MAX_BIT_DEPTH,
            max_decoded_bytes: Self::MAX_DECODED_BYTES,
            max_scratch_bytes: Self::MAX_SCRATCH_BYTES,
        }
    }

    /// Refuse a bundle the preflight arithmetic cannot honour, with the closed
    /// [`MediaError::InvalidLimits`]; nothing is clamped.
    ///
    /// This is stricter than [`DecodeLimits::validate`] on one field by design: a declared
    /// budget with `max_scratch_bytes == 0` would refuse every decoder scratch request, so
    /// it is treated as an unusable *override* rather than as a ceiling. The raw
    /// [`DecodeLimits`] type keeps its historical meaning (zero scratch is representable
    /// there) for callers that construct it directly.
    pub fn validate(self) -> Result<Self, MediaError> {
        if self.max_scratch_bytes == 0 {
            return Err(MediaError::InvalidLimits);
        }
        DecodeLimits::from(self).validate()?;
        Ok(self)
    }

    /// The caller-facing budgets this bundle declares, validated.
    pub fn limits(self) -> Result<DecodeLimits, MediaError> {
        self.validate()?;
        Ok(DecodeLimits::from(self))
    }
}

impl Default for DecodeBudget {
    /// The owner-ratified declared provisional bundle; the same value as
    /// [`DecodeBudget::ratified_default`].
    fn default() -> Self {
        Self::ratified_default()
    }
}

impl From<DecodeBudget> for DecodeLimits {
    /// Raw conversion; use [`DecodeBudget::limits`] when the value must be validated too.
    fn from(budget: DecodeBudget) -> Self {
        Self {
            max_dimension: budget.max_dimension,
            max_pixels: budget.max_pixels,
            max_bit_depth: budget.max_bit_depth,
            max_decoded_bytes: budget.max_decoded_bytes,
            max_scratch_bytes: budget.max_scratch_bytes,
        }
    }
}

/// Per-field replacement of a [`DecodeBudget`]; `None` keeps the base value.
///
/// This is the only sanctioned way to change a declared limit: resolution is pure (the base
/// bundle is never modified), order-independent for disjoint fields, and an unusable value
/// (zero, or a bit depth outside `1..=16`) is refused with [`MediaError::InvalidLimits`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DecodeLimitsOverride {
    pub max_dimension: Option<u32>,
    pub max_pixels: Option<u64>,
    pub max_bit_depth: Option<u8>,
    pub max_decoded_bytes: Option<u64>,
    pub max_scratch_bytes: Option<u64>,
}

impl DecodeLimitsOverride {
    /// No replacement: resolving keeps every base value.
    pub const fn none() -> Self {
        Self {
            max_dimension: None,
            max_pixels: None,
            max_bit_depth: None,
            max_decoded_bytes: None,
            max_scratch_bytes: None,
        }
    }

    /// Pure per-field replacement: every `Some` replaces exactly its field.
    pub const fn apply(self, base: DecodeBudget) -> DecodeBudget {
        DecodeBudget {
            max_dimension: match self.max_dimension {
                Some(value) => value,
                None => base.max_dimension,
            },
            max_pixels: match self.max_pixels {
                Some(value) => value,
                None => base.max_pixels,
            },
            max_bit_depth: match self.max_bit_depth {
                Some(value) => value,
                None => base.max_bit_depth,
            },
            max_decoded_bytes: match self.max_decoded_bytes {
                Some(value) => value,
                None => base.max_decoded_bytes,
            },
            max_scratch_bytes: match self.max_scratch_bytes {
                Some(value) => value,
                None => base.max_scratch_bytes,
            },
        }
    }

    /// Stack two replacement sets. A field set in `later` replaces the same field set in
    /// `self`, so applying `first.merge(second)` is "second wins on collision"; two sets
    /// that touch disjoint fields therefore resolve identically in either order.
    pub const fn merge(self, later: Self) -> Self {
        Self {
            max_dimension: match later.max_dimension {
                Some(value) => Some(value),
                None => self.max_dimension,
            },
            max_pixels: match later.max_pixels {
                Some(value) => Some(value),
                None => self.max_pixels,
            },
            max_bit_depth: match later.max_bit_depth {
                Some(value) => Some(value),
                None => self.max_bit_depth,
            },
            max_decoded_bytes: match later.max_decoded_bytes {
                Some(value) => Some(value),
                None => self.max_decoded_bytes,
            },
            max_scratch_bytes: match later.max_scratch_bytes {
                Some(value) => Some(value),
                None => self.max_scratch_bytes,
            },
        }
    }

    /// Resolve against the owner-ratified declared defaults and validate the result.
    pub fn resolve(self) -> Result<DecodeLimits, MediaError> {
        self.resolve_from(DecodeBudget::ratified_default())
    }

    /// Resolve against an explicit declared base bundle and validate the result.
    pub fn resolve_from(self, base: DecodeBudget) -> Result<DecodeLimits, MediaError> {
        self.apply(base).limits()
    }
}

/// Declared decode request. `declared_scratch_bytes` is the decoder adapter's
/// conservative peak claim; it is checked, never allocated here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecodeRequest {
    pub size: PixelSize,
    pub bit_depth: u8,
    pub channels: u8,
    pub declared_scratch_bytes: u64,
}

/// Bounded preflight result for a decoder adapter to honour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecodeAdmission {
    pub decoded_bytes: u64,
    pub scratch_bytes: u64,
    pub total_bytes: u64,
}

/// Admit declared decode work before any allocation or decoder call.
///
/// Rejects impossible dimensions, unsupported bit depth/channel declarations,
/// decoded-byte or scratch budgets above the caller ceiling, and cancellation.
/// Arithmetic is checked; overflow is `TooLarge`, never a wrapped value.
pub fn admit_decode(
    request: DecodeRequest,
    limits: DecodeLimits,
    cancel: &Cancellation,
) -> Result<DecodeAdmission, MediaError> {
    cancel.check()?;
    let limits = limits.validate()?;
    if request.size.width() > limits.max_dimension || request.size.height() > limits.max_dimension {
        return Err(MediaError::TooLarge);
    }
    let area = request.size.checked_area()?;
    if area > limits.max_pixels {
        return Err(MediaError::TooLarge);
    }
    if !(1..=MAX_SUPPORTED_BIT_DEPTH).contains(&request.bit_depth)
        || request.bit_depth > limits.max_bit_depth
    {
        return Err(MediaError::InvalidMetadata);
    }
    if !(1..=MAX_SUPPORTED_CHANNELS).contains(&request.channels) {
        return Err(MediaError::InvalidMetadata);
    }
    let bits = area
        .checked_mul(u64::from(request.channels))
        .and_then(|value| value.checked_mul(u64::from(request.bit_depth)))
        .ok_or(MediaError::TooLarge)?;
    let decoded_bytes = bits.checked_add(7).ok_or(MediaError::TooLarge)? / 8;
    if decoded_bytes > limits.max_decoded_bytes {
        return Err(MediaError::TooLarge);
    }
    if request.declared_scratch_bytes > limits.max_scratch_bytes {
        return Err(MediaError::TooLarge);
    }
    let total_bytes = decoded_bytes
        .checked_add(request.declared_scratch_bytes)
        .ok_or(MediaError::TooLarge)?;
    cancel.check()?;
    Ok(DecodeAdmission {
        decoded_bytes,
        scratch_bytes: request.declared_scratch_bytes,
        total_bytes,
    })
}
