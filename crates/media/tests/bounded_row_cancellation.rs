// MP-IMG2: bounded cancellation of a *running* PNG decode.
//
// §19.118 recorded bounded cancellation of a running decoder as HOLD because the
// `image` backend decodes in one uninterruptible call. These tests cover the narrower
// path that does exist: a PNG decode that reads **one row at a time** and checks the
// caller's `Cancellation` between rows, so a cancelled decode stops early, returns a
// typed `Cancelled`, and publishes nothing.
//
// Scope, stated plainly: this covers **PNG only**. JPEG, WebP and GIF still go through
// the uninterruptible backend and remain HOLD for bounded cancellation.

use image::{DynamicImage, ImageFormat, RgbImage};
use media::{Cancellation, DecodeBudget, DecodeLimits, MediaError, PixelSize};
use std::io::Cursor;

fn limits() -> DecodeLimits {
    DecodeBudget::ratified_default()
        .limits()
        .expect("ratified budget resolves")
}

fn png_rgb(width: u32, height: u32) -> Vec<u8> {
    let image = RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x % 251) as u8, (y % 241) as u8, 90])
    });
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image)
        .write_to(&mut bytes, ImageFormat::Png)
        .expect("encode an 8-bit RGB PNG");
    bytes.into_inner()
}

#[test]
fn a_row_wise_png_decode_returns_a_complete_raster() {
    let bytes = png_rgb(256, 128);
    let raster = media::decode_png_rows(&bytes, limits(), &Cancellation::default())
        .expect("the PNG decodes row by row");
    assert_eq!(raster.size, PixelSize::new(256, 128).expect("size"));
    assert_eq!(
        raster.rgba.len() as u64,
        raster.size.checked_rgba_bytes().expect("rgba length"),
        "the row-wise path must produce a complete raster"
    );
}

#[test]
fn cancellation_between_rows_stops_the_decode_early() {
    let bytes = png_rgb(256, 512);
    let cancel = Cancellation::default();
    // Cancel from inside the decoder's per-row hook, after a known number of rows.
    let rows_seen = std::cell::Cell::new(0u32);
    let result = media::decode_png_rows_with_hook(&bytes, limits(), &cancel, &mut |rows| {
        rows_seen.set(rows);
        if rows == 16 {
            cancel.cancel();
        }
    });
    assert_eq!(
        result.err(),
        Some(MediaError::Cancelled),
        "a decode cancelled between rows must be typed Cancelled"
    );
    assert_eq!(
        rows_seen.get(),
        16,
        "the decode must stop at the row where cancellation was observed, not run to completion"
    );
}

#[test]
fn a_cancelled_row_wise_decode_publishes_nothing() {
    let bytes = png_rgb(64, 64);
    let cancel = Cancellation::default();
    cancel.cancel();
    match media::decode_png_rows(&bytes, limits(), &cancel) {
        Err(error) => assert_eq!(error, MediaError::Cancelled),
        Ok(_) => panic!("a cancelled decode must not return a raster"),
    }
}

#[test]
fn the_row_wise_path_refuses_an_over_limit_input_before_decoding() {
    let bytes = png_rgb(20_000, 1);
    match media::decode_png_rows(&bytes, limits(), &Cancellation::default()) {
        Err(error) => assert_eq!(error, MediaError::TooLarge),
        Ok(_) => panic!("a refused input must not publish a raster"),
    }
}

#[test]
fn a_non_png_input_is_refused_by_the_row_wise_path() {
    let image = RgbImage::from_fn(32, 32, |x, y| image::Rgb([(x % 7) as u8, (y % 5) as u8, 3]));
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image)
        .write_to(&mut bytes, ImageFormat::Jpeg)
        .expect("encode a JPEG");
    let jpeg = bytes.into_inner();
    match media::decode_png_rows(&jpeg, limits(), &Cancellation::default()) {
        Err(error) => assert_eq!(error, MediaError::UnsupportedContainer),
        Ok(_) => panic!("the row-wise path is PNG-only"),
    }
}
