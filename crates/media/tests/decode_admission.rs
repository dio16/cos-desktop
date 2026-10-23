//! MP-IMG2 decoder-agnostic preflight: declared dimensions/bit depth/scratch are
//! checked before any allocation. No decoder, codec capability or GPU behaviour is
//! exercised or simulated.
use media::{Cancellation, DecodeLimits, DecodeRequest, MediaError, PixelSize, admit_decode};

fn limits() -> DecodeLimits {
    DecodeLimits {
        max_dimension: 8192,
        max_pixels: 16_000_000,
        max_bit_depth: 16,
        max_decoded_bytes: 64_000_000,
        max_scratch_bytes: 32_000_000,
    }
}

fn request(width: u32, height: u32, bit_depth: u8, channels: u8, scratch: u64) -> DecodeRequest {
    DecodeRequest {
        size: PixelSize::new(width, height).unwrap(),
        bit_depth,
        channels,
        declared_scratch_bytes: scratch,
    }
}

#[test]
fn declared_budget_is_computed_with_checked_arithmetic() {
    let admitted = admit_decode(
        request(32, 32, 8, 4, 1024),
        limits(),
        &Cancellation::default(),
    )
    .unwrap();
    assert_eq!(
        admitted,
        media::DecodeAdmission {
            decoded_bytes: 4096,
            scratch_bytes: 1024,
            total_bytes: 5120,
        }
    );
    // 16-bit RGBA uses two bytes per channel with no rounding surprise.
    let wide = admit_decode(
        request(1000, 1000, 16, 4, 0),
        limits(),
        &Cancellation::default(),
    )
    .unwrap();
    assert_eq!(wide.decoded_bytes, 8_000_000);
    // 1-bit single-channel rounds up to a whole byte.
    let packed = admit_decode(request(3, 1, 1, 1, 0), limits(), &Cancellation::default()).unwrap();
    assert_eq!(packed.decoded_bytes, 1);
}

#[test]
fn impossible_dimensions_and_declarations_are_rejected_before_arithmetic() {
    for (width, height, bit_depth, channels, expected) in [
        (8193, 32, 8, 4, MediaError::TooLarge),
        (32, 8193, 8, 4, MediaError::TooLarge),
        (5000, 4000, 8, 4, MediaError::TooLarge), // 20M pixels over the ceiling
        (0, 32, 8, 4, MediaError::InvalidMetadata),
        (32, 32, 0, 4, MediaError::InvalidMetadata),
        (32, 32, 17, 4, MediaError::InvalidMetadata),
        (32, 32, 8, 0, MediaError::InvalidMetadata),
        (32, 32, 8, 5, MediaError::InvalidMetadata),
    ] {
        let result = if width == 0 {
            // PixelSize itself refuses zero dimensions.
            PixelSize::new(width, height).err()
        } else {
            admit_decode(
                request(width, height, bit_depth, channels, 0),
                limits(),
                &Cancellation::default(),
            )
            .err()
        };
        assert_eq!(
            result,
            Some(expected),
            "{width}x{height} {bit_depth}bpp {channels}ch"
        );
    }
}

#[test]
fn budgets_are_inclusive_and_one_byte_over_is_rejected() {
    let exact = request(100, 100, 8, 4, 0); // 40,000 decoded bytes
    let exact_limits = DecodeLimits {
        max_decoded_bytes: 40_000,
        ..limits()
    };
    assert_eq!(
        admit_decode(exact, exact_limits, &Cancellation::default())
            .unwrap()
            .decoded_bytes,
        40_000
    );
    let tight = DecodeLimits {
        max_decoded_bytes: 39_999,
        ..limits()
    };
    assert_eq!(
        admit_decode(exact, tight, &Cancellation::default()).err(),
        Some(MediaError::TooLarge)
    );
    let scratch_limits = DecodeLimits {
        max_scratch_bytes: 4096,
        ..limits()
    };
    assert_eq!(
        admit_decode(
            request(32, 32, 8, 4, 4097),
            scratch_limits,
            &Cancellation::default()
        )
        .err(),
        Some(MediaError::TooLarge)
    );
}

#[test]
fn invalid_limits_and_cancellation_fail_closed() {
    for bad in [
        DecodeLimits {
            max_dimension: 0,
            ..limits()
        },
        DecodeLimits {
            max_pixels: 0,
            ..limits()
        },
        DecodeLimits {
            max_bit_depth: 0,
            ..limits()
        },
        DecodeLimits {
            max_bit_depth: 17,
            ..limits()
        },
        DecodeLimits {
            max_decoded_bytes: 0,
            ..limits()
        },
    ] {
        assert_eq!(
            admit_decode(request(32, 32, 8, 4, 0), bad, &Cancellation::default()).err(),
            Some(MediaError::InvalidLimits)
        );
    }
    let cancel = Cancellation::default();
    cancel.cancel();
    assert_eq!(
        admit_decode(request(32, 32, 8, 4, 0), limits(), &cancel).err(),
        Some(MediaError::Cancelled)
    );
}
