//! MP-IMG2 ratified **declared provisional** decode budgets: defaults, per-field overrides
//! and the closed rejection of unusable replacements.
//!
//! Every number asserted here is the owner-ratified declared value on an **unmeasured
//! basis**; no measurement, decoder call or allocation is exercised or implied.
use media::{
    Cancellation, DecodeBudget, DecodeLimits, DecodeLimitsOverride, DecodeRequest, MediaError,
    PixelSize, admit_decode,
};

fn request(width: u32, height: u32, bit_depth: u8, channels: u8, scratch: u64) -> DecodeRequest {
    DecodeRequest {
        size: PixelSize::new(width, height).unwrap(),
        bit_depth,
        channels,
        declared_scratch_bytes: scratch,
    }
}

#[test]
fn ratified_defaults_are_the_declared_numbers() {
    let budget = DecodeBudget::ratified_default();
    assert_eq!(budget.max_dimension, 16_384);
    assert_eq!(budget.max_pixels, 268_435_456);
    assert_eq!(budget.max_bit_depth, 16);
    assert_eq!(budget.max_decoded_bytes, 268_435_456);
    assert_eq!(budget.max_scratch_bytes, 268_435_456);

    // The declared relations the numbers are derived from, asserted separately so a change
    // to one constant without the other fails loudly.
    assert_eq!(
        u64::from(budget.max_dimension) * u64::from(budget.max_dimension),
        budget.max_pixels,
        "the pixel ceiling is the declared stress edge squared"
    );
    assert_eq!(budget.max_decoded_bytes, 256 * 1024 * 1024);
    assert_eq!(budget.max_scratch_bytes, 256 * 1024 * 1024);
    assert_eq!(budget.max_bit_depth, media::MAX_SUPPORTED_BIT_DEPTH);

    // Named constants, the constructor and `Default` are one bundle.
    assert_eq!(budget, DecodeBudget::default());
    assert_eq!(DecodeBudget::MAX_DIMENSION, budget.max_dimension);
    assert_eq!(DecodeBudget::MAX_PIXELS, budget.max_pixels);
    assert_eq!(DecodeBudget::MAX_BIT_DEPTH, budget.max_bit_depth);
    assert_eq!(DecodeBudget::MAX_DECODED_BYTES, budget.max_decoded_bytes);
    assert_eq!(DecodeBudget::MAX_SCRATCH_BYTES, budget.max_scratch_bytes);

    // The bundle validates and resolves to the caller-facing limits unchanged.
    assert_eq!(budget.validate().unwrap(), budget);
    assert_eq!(DecodeLimits::from(budget), budget.limits().unwrap());
    assert_eq!(
        DecodeLimitsOverride::none().resolve().unwrap(),
        budget.limits().unwrap()
    );
    assert_eq!(
        DecodeLimitsOverride::default().resolve().unwrap(),
        budget.limits().unwrap()
    );
}

#[test]
fn every_field_override_is_honoured() {
    let base = DecodeBudget::ratified_default();
    let cases = [
        DecodeLimitsOverride {
            max_dimension: Some(8_192),
            ..DecodeLimitsOverride::none()
        },
        DecodeLimitsOverride {
            max_pixels: Some(4_000_000),
            ..DecodeLimitsOverride::none()
        },
        DecodeLimitsOverride {
            max_bit_depth: Some(8),
            ..DecodeLimitsOverride::none()
        },
        DecodeLimitsOverride {
            max_decoded_bytes: Some(64_000_000),
            ..DecodeLimitsOverride::none()
        },
        DecodeLimitsOverride {
            max_scratch_bytes: Some(1),
            ..DecodeLimitsOverride::none()
        },
    ];
    for over in cases {
        let resolved = over.apply(base);
        // Exactly one field differs, and it is the field the override named.
        let mut expected = base;
        if let Some(value) = over.max_dimension {
            expected.max_dimension = value;
        }
        if let Some(value) = over.max_pixels {
            expected.max_pixels = value;
        }
        if let Some(value) = over.max_bit_depth {
            expected.max_bit_depth = value;
        }
        if let Some(value) = over.max_decoded_bytes {
            expected.max_decoded_bytes = value;
        }
        if let Some(value) = over.max_scratch_bytes {
            expected.max_scratch_bytes = value;
        }
        assert_eq!(resolved, expected, "{over:?}");
        assert_eq!(
            over.resolve().unwrap(),
            DecodeLimits::from(expected),
            "{over:?}"
        );
    }

    // A ceiling may be raised as well as lowered by an explicit override; resolution does
    // not clamp in either direction.
    let raised = DecodeLimitsOverride {
        max_dimension: Some(32_768),
        max_pixels: Some(1_073_741_824),
        ..DecodeLimitsOverride::none()
    }
    .resolve()
    .unwrap();
    assert_eq!(raised.max_dimension, 32_768);
    assert_eq!(raised.max_pixels, 1_073_741_824);
    assert_eq!(raised.max_decoded_bytes, base.max_decoded_bytes);
}

#[test]
fn invalid_overrides_are_rejected_with_the_closed_error() {
    let zero_overrides = [
        DecodeLimitsOverride {
            max_dimension: Some(0),
            ..DecodeLimitsOverride::none()
        },
        DecodeLimitsOverride {
            max_pixels: Some(0),
            ..DecodeLimitsOverride::none()
        },
        DecodeLimitsOverride {
            max_bit_depth: Some(0),
            ..DecodeLimitsOverride::none()
        },
        DecodeLimitsOverride {
            max_decoded_bytes: Some(0),
            ..DecodeLimitsOverride::none()
        },
        DecodeLimitsOverride {
            max_scratch_bytes: Some(0),
            ..DecodeLimitsOverride::none()
        },
    ];
    for over in zero_overrides {
        assert_eq!(
            over.resolve().err(),
            Some(MediaError::InvalidLimits),
            "{over:?}"
        );
        assert_eq!(
            over.resolve_from(DecodeBudget::ratified_default()).err(),
            Some(MediaError::InvalidLimits),
            "{over:?}"
        );
    }

    // Out of the arithmetic's range in both directions.
    for depth in [0_u8, media::MAX_SUPPORTED_BIT_DEPTH + 1, u8::MAX] {
        let over = DecodeLimitsOverride {
            max_bit_depth: Some(depth),
            ..DecodeLimitsOverride::none()
        };
        assert_eq!(
            over.resolve().err(),
            Some(MediaError::InvalidLimits),
            "bit depth {depth} must be refused, never clamped"
        );
    }

    // The bundle-level validator refuses the same values.
    let mut broken = DecodeBudget::ratified_default();
    broken.max_scratch_bytes = 0;
    assert_eq!(broken.validate().err(), Some(MediaError::InvalidLimits));
    assert_eq!(broken.limits().err(), Some(MediaError::InvalidLimits));
}

#[test]
fn resolution_is_pure_and_order_independent() {
    let base = DecodeBudget::ratified_default();
    let first = DecodeLimitsOverride {
        max_dimension: Some(8_192),
        max_bit_depth: Some(8),
        ..DecodeLimitsOverride::none()
    };
    let second = DecodeLimitsOverride {
        max_pixels: Some(16_000_000),
        max_decoded_bytes: Some(64_000_000),
        ..DecodeLimitsOverride::none()
    };

    // Purity: resolving twice gives the same answer and never mutates the base.
    assert_eq!(first.apply(base), first.apply(base));
    assert_eq!(base, DecodeBudget::ratified_default());

    // Two overrides on disjoint fields resolve identically in either order.
    let forward = first.merge(second).apply(base);
    let backward = second.merge(first).apply(base);
    assert_eq!(forward, backward);
    assert_eq!(
        first.merge(second).resolve().unwrap(),
        second.merge(first).resolve().unwrap()
    );
    assert_eq!(forward.max_dimension, 8_192);
    assert_eq!(forward.max_bit_depth, 8);
    assert_eq!(forward.max_pixels, 16_000_000);
    assert_eq!(forward.max_decoded_bytes, 64_000_000);
    assert_eq!(forward.max_scratch_bytes, base.max_scratch_bytes);

    // Collisions are documented, not silent: the later set wins per field.
    let lower = DecodeLimitsOverride {
        max_pixels: Some(1_000),
        ..DecodeLimitsOverride::none()
    };
    let higher = DecodeLimitsOverride {
        max_pixels: Some(2_000),
        ..DecodeLimitsOverride::none()
    };
    assert_eq!(lower.merge(higher).apply(base).max_pixels, 2_000);
    assert_eq!(higher.merge(lower).apply(base).max_pixels, 1_000);
}

#[test]
fn budget_limits_pass_the_existing_validation_and_admit_the_declared_ceiling() {
    let limits = DecodeBudget::ratified_default().limits().unwrap();
    assert_eq!(limits, DecodeLimits::from(DecodeBudget::ratified_default()));

    // The declared stress edge at 8 bits single channel is exactly both ceilings
    // (16_384² pixels and 268_435_456 decoded bytes), and `admit_decode` accepts it.
    let admitted = admit_decode(
        request(16_384, 16_384, 8, 1, 0),
        limits,
        &Cancellation::default(),
    )
    .unwrap();
    assert_eq!(admitted.decoded_bytes, 268_435_456);
    assert_eq!(admitted.scratch_bytes, 0);
    assert_eq!(admitted.total_bytes, 268_435_456);

    // The declared scratch ceiling is inclusive, and one byte over is refused.
    let scratch_exact = admit_decode(
        request(1, 1, 8, 1, 268_435_456),
        limits,
        &Cancellation::default(),
    )
    .unwrap();
    assert_eq!(scratch_exact.scratch_bytes, 268_435_456);
    assert_eq!(
        admit_decode(
            request(1, 1, 8, 1, 268_435_456 + 1),
            limits,
            &Cancellation::default()
        )
        .err(),
        Some(MediaError::TooLarge)
    );

    // The decoded-byte ceiling bites independently of the pixel/edge ceilings: four
    // channels at the stress edge exceed 256 MiB.
    assert_eq!(
        admit_decode(
            request(16_384, 16_384, 8, 4, 0),
            limits,
            &Cancellation::default()
        )
        .err(),
        Some(MediaError::TooLarge)
    );

    // One edge pixel past the declared edge is refused.
    assert_eq!(
        admit_decode(
            request(16_385, 1, 8, 1, 0),
            limits,
            &Cancellation::default()
        )
        .err(),
        Some(MediaError::TooLarge)
    );

    // A lowered override reaches admission: the same request is now too large.
    let lowered = DecodeLimitsOverride {
        max_decoded_bytes: Some(1_000),
        ..DecodeLimitsOverride::none()
    }
    .resolve()
    .unwrap();
    assert_eq!(
        admit_decode(
            request(16_384, 16_384, 8, 1, 0),
            lowered,
            &Cancellation::default()
        )
        .err(),
        Some(MediaError::TooLarge)
    );
}
