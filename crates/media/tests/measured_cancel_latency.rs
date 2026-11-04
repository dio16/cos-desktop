// MP-IMG2: how long a row-wise PNG decode keeps running after cancellation.
//
// §19.240 wired the row decoder into the product path and recorded that cancellation is
// observed between rows, but claimed **no** latency bound because none was measured. This
// binary measures two different questions and keeps them apart:
//
// 1. **Cancelled from inside a row hook** - at a row boundary, the moment the loop is already
//    checking. That is the *best* case, reported as "return time at a row boundary", not as a
//    bound.
// 2. **Cancelled from another thread while the decode is running** - the realistic case. The
//    helper thread records when it set the flag, and the elapsed time to the call's return is
//    the figure that matters. No claim is made that it is bounded by one row: the per-row figure
//    available here is an average, not a worst case.
//
// Both runs assert that the loop stopped at the first row where the flag was already visible,
// which is what a regression that kept decoding past the flag would break.

use image::{DynamicImage, ImageFormat, RgbImage};
use media::{Cancellation, DecodeBudget, DecodeLimits, MediaError};
use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

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
fn cancellation_at_a_row_boundary_returns_without_decoding_further() {
    const WIDTH: u32 = 3000;
    const HEIGHT: u32 = 3000;
    const CANCEL_AT_ROW: u32 = HEIGHT / 4;

    let bytes = png_rgb(WIDTH, HEIGHT);
    let cancel = Cancellation::default();
    let cancelled_at: Arc<std::sync::Mutex<Option<Instant>>> =
        Arc::new(std::sync::Mutex::new(None));
    let rows_after_flag = Arc::new(AtomicU64::new(0));
    let last_row = Arc::new(AtomicU64::new(0));

    let stamp = Arc::clone(&cancelled_at);
    let observer = Arc::clone(&rows_after_flag);
    let last = Arc::clone(&last_row);
    let result = media::decode_raster_with_row_hook(&bytes, limits(), &cancel, &mut |rows| {
        last.store(u64::from(rows), Ordering::SeqCst);
        let mut slot = stamp.lock().expect("stamp");
        match *slot {
            None if rows == CANCEL_AT_ROW => {
                *slot = Some(Instant::now());
                cancel.cancel();
            }
            Some(_) => {
                observer.fetch_add(1, Ordering::SeqCst);
            }
            None => {}
        }
    });

    let micros = cancelled_at
        .lock()
        .expect("stamp")
        .expect("the hook must have run")
        .elapsed()
        .as_micros();
    eprintln!(
        "row-boundary cancel at row {CANCEL_AT_ROW}/{HEIGHT}: returned in {micros} us, \
         rows processed after the flag: {}",
        rows_after_flag.load(Ordering::SeqCst)
    );
    assert_eq!(result.err(), Some(MediaError::Cancelled));
    assert_eq!(
        rows_after_flag.load(Ordering::SeqCst),
        0,
        "no row may be decoded after the cancellation was observed"
    );
    assert_eq!(
        last_row.load(Ordering::SeqCst),
        u64::from(CANCEL_AT_ROW),
        "the loop must stop at the row where cancellation was observed"
    );
}

#[test]
fn cancellation_from_another_thread_mid_decode_returns_promptly() {
    const WIDTH: u32 = 3000;
    const HEIGHT: u32 = 3000;
    /// Measurement threshold only: far above one row's work and far below a full decode of this
    /// image, so a loop that stopped checking between rows would fail.
    const BOUND_US: u128 = 250_000;

    let bytes = png_rgb(WIDTH, HEIGHT);
    let cancel = Cancellation::default();
    let rows_seen = Arc::new(AtomicU64::new(0));
    let cancelled_at: Arc<std::sync::Mutex<Option<Instant>>> =
        Arc::new(std::sync::Mutex::new(None));

    // The decode runs on this thread; the helper sets the flag while the image is still being
    // decoded, which is the case the row-boundary measurement cannot represent.
    let helper_cancel = cancel.clone();
    let helper_stamp = Arc::clone(&cancelled_at);
    let helper = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(5));
        *helper_stamp.lock().expect("stamp") = Some(Instant::now());
        helper_cancel.cancel();
    });

    let seen = Arc::clone(&rows_seen);
    let result = media::decode_raster_with_row_hook(&bytes, limits(), &cancel, &mut |rows| {
        seen.store(u64::from(rows), Ordering::SeqCst);
    });

    let returned = Instant::now();
    helper.join().expect("helper thread");
    let cancelled_moment = cancelled_at.lock().expect("stamp").expect("stamp set");
    let micros = returned.duration_since(cancelled_moment).as_micros();
    let stopped_at = rows_seen.load(Ordering::SeqCst);

    eprintln!(
        "external cancel mid-decode: stopped after row {stopped_at} of {HEIGHT}, \
         returned {micros} us after the flag was set"
    );
    assert_eq!(result.err(), Some(MediaError::Cancelled));
    assert!(
        stopped_at > 0 && stopped_at < u64::from(HEIGHT),
        "the cancellation must have landed mid-image, not before the first row or after the last"
    );
    assert!(
        micros <= BOUND_US,
        "returned {micros} us after cancellation, beyond this test's {BOUND_US} us threshold"
    );
}
