//! MP-IMG2 decode preflight arithmetic shared by decoder adapters.
//!
//! This admits declared dimensions/bit depth/scratch against caller budgets before a
//! decoder allocates. It selects no decoder, decodes nothing, owns no GPU object and
//! makes no format-support, profile, region or capability claim; those remain the
//! decoder owner's admission inputs.

use crate::{Cancellation, MediaError, PixelSize};

/// Caller-supplied decode budgets. No numeric SLO is implied; values come from the
/// owning experiment/configuration, not from this module.
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
            || !(1..=16).contains(&self.max_bit_depth)
            || self.max_decoded_bytes == 0
        {
            return Err(MediaError::InvalidLimits);
        }
        Ok(self)
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
    if !(1..=16).contains(&request.bit_depth) || request.bit_depth > limits.max_bit_depth {
        return Err(MediaError::InvalidMetadata);
    }
    if !(1..=4).contains(&request.channels) {
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
