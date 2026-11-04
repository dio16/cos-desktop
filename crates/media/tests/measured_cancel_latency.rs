// MP-IMG2: how long a row-wise PNG decode keeps running after cancellation is observed.
//
// §19.240 wired the row decoder into the product path and recorded that cancellation is
// observed between rows, but explicitly did **not** claim a latency bound because none was
// measured. This binary measures it: cancel from inside the row hook and time how long the
// call takes to return afterwards.
//
// The bound asserted here is a **measurement threshold chosen when writing the test**, not a
// product SLO: it exists so a regression that stopped checking between rows (and therefore ran
// to completion) fails instead of quietly passing. The measured figures are printed for the
// record.

use image::{DynamicImage, ImageFormat, RgbImage};
use media::{Cancellation, DecodeBudget, DecodeLimits, MediaError};
use std::io::Cursor;
use std::time::Instant;

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
fn cancellation_is_observed_and_the_call_returns_promptly() {
    // 3000×3000 is large enough that a decode which ignored the cancellation would keep running
    // long enough to be obvious in the timing, and small enough to keep this test quick.
    const WIDTH: u32 = 3000;
    const HEIGHT: u32 = 3000;
    const CANCEL_AT_ROW: u32 = HEIGHT / 4;
    /// Measurement threshold, not an SLO.
    const BOUND_MS: u128 = 250;

    let bytes = png_rgb(WIDTH, HEIGHT);
    let mut samples = Vec::new();
    for run in 0..3 {
        let cancel = Cancellation::default();
        let mut cancelled_at: Option<Instant> = None;
        let mut rows_when_cancelled = 0u32;
        let result = media::decode_raster_with_row_hook(&bytes, limits(), &cancel, &mut |rows| {
            if rows == CANCEL_AT_ROW {
                rows_when_cancelled = rows;
                cancelled_at = Some(Instant::now());
                cancel.cancel();
            }
        });
        let elapsed_ms = cancelled_at
            .expect("the hook must have run before the call returned")
            .elapsed()
            .as_millis();
        eprintln!(
            "run {run}: cancelled at row {rows_when_cancelled} of {HEIGHT}, returned {elapsed_ms} ms later"
        );
        assert_eq!(
            result.err(),
            Some(MediaError::Cancelled),
            "run {run}: a cancelled decode must be typed Cancelled"
        );
        assert_eq!(
            rows_when_cancelled, CANCEL_AT_ROW,
            "run {run}: cancellation must be observed at the requested row"
        );
        samples.push(elapsed_ms);
    }

    // What the 0 ms figure means: cancellation is checked at the top of each row iteration, so
    // the wait is at most one row's work. Measure that row to give the number a scale.
    let full_start = Instant::now();
    let _ = media::decode_raster(&bytes, limits(), &Cancellation::default())
        .expect("the same image decodes when not cancelled");
    let full_ms = full_start.elapsed().as_millis();
    let per_row_us = (full_start.elapsed().as_micros()) / u128::from(HEIGHT);
    eprintln!("full decode {full_ms} ms for {HEIGHT} rows, about {per_row_us} us per row");

    let worst = *samples.iter().max().expect("three samples");
    eprintln!("worst return latency after cancellation: {worst} ms");
    assert!(
        worst <= BOUND_MS,
        "returning took {worst} ms after cancellation, beyond this test's {BOUND_MS} ms threshold"
    );
}
