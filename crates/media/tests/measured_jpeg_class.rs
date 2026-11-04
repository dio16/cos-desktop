//! MP-IMG2 second class: static 8-bit JPEG, functional case (64×64).
//!
//! The assigned measurement class in §19.119 is a **static 8-bit RGB PNG at
//! 2048×2048**. This binary does **not** re-measure that class; it proves that the
//! contracts the slice established are **not PNG-specific** by running the same
//! admission, refusal and cancellation conditions against a JPEG:
//!
//! - the declared ceiling is computed from the pixel class, so it matches the PNG
//!   figure for the same geometry;
//! - a JPEG in the class decodes to a complete raster;
//! - a cancelled decode is typed `Cancelled` and publishes nothing;
//! - an over-limit JPEG is typed `TooLarge` before decode and publishes nothing;
//! - a truncated JPEG is typed `InvalidMetadata` and publishes nothing.
//!
//! **Verified here:** the class ceiling for the same geometry, complete-raster
//! decoding for JPEG, static WebP and GIF, cancellation for JPEG, the dimension-edge
//! refusal for JPEG and GIF, the byte-budget refusal **for WebP only**, and the
//! truncated-input refusal for JPEG.
//!
//! **Not claimed here:** Working Set peak for these classes (that measurement is the
//! PNG class in `measured_png_class.rs` plus `examples/peak_probe.rs`), internal
//! allocator peak and rejection allocation flatness (HOLD, `unsafe_code = "forbid"`),
//! bounded cancellation of a *running* decoder (NOT_VERIFIED, the backend call is not
//! interruptible), cancellation for WebP/GIF, the dimension-edge refusal for WebP (its
//! encoder refuses such dimensions, so the case cannot be constructed), and other bit
//! depths.

use image::{DynamicImage, ImageFormat, RgbImage};
use media::{
    Cancellation, DecodeBudget, DecodeLimits, DecodeLimitsOverride, MediaError, PixelSize,
    decode_raster_joined,
};
use std::io::Cursor;

/// Serialises these tests so their publication assertions cannot observe each other.
static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn test_guard() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().expect("test lock")
}

fn limits() -> DecodeLimits {
    DecodeBudget::ratified_default()
        .limits()
        .expect("ratified budget resolves")
}

/// A static 8-bit RGB JPEG of the requested dimensions.
fn jpeg_rgb(width: u32, height: u32) -> Vec<u8> {
    let image = RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x % 251) as u8, (y % 241) as u8, 90])
    });
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image)
        .write_to(&mut bytes, ImageFormat::Jpeg)
        .expect("encode an 8-bit RGB JPEG");
    bytes.into_inner()
}

#[test]
fn the_ceiling_is_a_property_of_the_pixel_class_not_the_format() {
    let _guard = test_guard();
    // Same geometry as the PNG functional case: the ceiling may not depend on how the
    // pixels were encoded.
    let size = PixelSize::new(64, 64).expect("size");
    assert_eq!(
        limits().peak_rgba8_bytes(size).expect("peak"),
        1_081_344,
        "the ceiling follows the pixel class, so a JPEG of the same size shares it"
    );
}

#[test]
fn a_jpeg_in_the_class_decodes_to_a_complete_raster() {
    let _guard = test_guard();
    let bytes = jpeg_rgb(64, 64);
    let raster = decode_raster_joined(&bytes, limits(), &Cancellation::default())
        .expect("the class decodes");
    assert_eq!(raster.size, PixelSize::new(64, 64).expect("size"));
    assert_eq!(
        raster.rgba.len() as u64,
        raster.size.checked_rgba_bytes().expect("rgba length"),
        "the returned raster is complete, never partial"
    );
}

#[test]
fn jpeg_cancellation_is_typed_and_publishes_nothing() {
    let _guard = test_guard();
    let cancel = Cancellation::default();
    cancel.cancel();
    match decode_raster_joined(&jpeg_rgb(64, 64), limits(), &cancel) {
        Err(error) => assert_eq!(error, MediaError::Cancelled),
        Ok(_) => panic!("a cancelled decode must not return a raster"),
    }
}

#[test]
fn an_over_limit_jpeg_is_refused_typed_and_publishes_nothing() {
    let _guard = test_guard();
    // 20,000 px is above the declared 16,384 px edge; admission refuses it before decode.
    let bytes = jpeg_rgb(20_000, 1);
    match decode_raster_joined(&bytes, limits(), &Cancellation::default()) {
        Err(error) => assert_eq!(error, MediaError::TooLarge),
        Ok(_) => panic!("a refused input must not publish a raster"),
    }
}

#[test]
fn a_truncated_jpeg_is_refused_typed_and_publishes_nothing() {
    let _guard = test_guard();
    let bytes = jpeg_rgb(64, 64);
    let truncated = &bytes[..bytes.len() / 3];
    match decode_raster_joined(truncated, limits(), &Cancellation::default()) {
        Err(error) => assert_eq!(
            error,
            MediaError::InvalidMetadata,
            "a truncated JPEG is a metadata/decoder failure, not a size failure and not a raster"
        ),
        Ok(_) => panic!("a truncated input must not publish a raster"),
    }
}

/// A static 8-bit RGB image encoded in `format`.
fn encoded_rgb(width: u32, height: u32, format: ImageFormat) -> Vec<u8> {
    let image = RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x % 251) as u8, (y % 241) as u8, 90])
    });
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image)
        .write_to(&mut bytes, format)
        .expect("encode the requested class");
    bytes.into_inner()
}

#[test]
fn the_other_advertised_classes_decode_to_complete_rasters() {
    let _guard = test_guard();
    for format in [ImageFormat::WebP, ImageFormat::Gif] {
        let bytes = encoded_rgb(64, 64, format);
        let raster = decode_raster_joined(&bytes, limits(), &Cancellation::default())
            .unwrap_or_else(|error| {
                panic!("{format:?} should decode in this class, got {error:?}")
            });
        assert_eq!(
            raster.size,
            PixelSize::new(64, 64).expect("size"),
            "{format:?}"
        );
        assert_eq!(
            raster.rgba.len() as u64,
            raster.size.checked_rgba_bytes().expect("rgba length"),
            "{format:?} must return a complete raster"
        );
    }
}

#[test]
fn an_over_limit_gif_is_refused_typed_at_the_dimension_edge() {
    let _guard = test_guard();
    // GIF stores dimensions as 16-bit values, so the edge case is reachable for this class and the
    // refusal must come from admission rather than from the encoder.
    let bytes = encoded_rgb(20_000, 1, ImageFormat::Gif);
    match decode_raster_joined(&bytes, limits(), &Cancellation::default()) {
        Err(error) => assert_eq!(error, MediaError::TooLarge),
        Ok(_) => panic!("a refused input must not publish a raster"),
    }
}

#[test]
fn webp_is_refused_typed_under_a_tight_byte_budget() {
    let _guard = test_guard();
    // The WebP encoder refuses dimensions past its own limit (`InvalidDimensions`), so an oversized
    // WebP cannot be built to exercise the edge; the byte ceiling is the refusal this class can
    // actually reach.
    let tight = DecodeLimitsOverride {
        max_decoded_bytes: Some(1024),
        ..DecodeLimitsOverride::none()
    }
    .resolve()
    .expect("tight budget resolves");
    let bytes = encoded_rgb(64, 64, ImageFormat::WebP);
    match decode_raster_joined(&bytes, tight, &Cancellation::default()) {
        Err(error) => assert_eq!(error, MediaError::TooLarge),
        Ok(_) => panic!("a refused input must not publish a raster"),
    }
}
