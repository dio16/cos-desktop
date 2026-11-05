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

/// The whole binary's measurements share one lock, so timings do not perturb each other.
fn heavy_lock() -> std::sync::MutexGuard<'static, ()> {
    static HEAVY: std::sync::Mutex<()> = std::sync::Mutex::new(());
    HEAVY.lock().unwrap_or_else(|poison| poison.into_inner())
}

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
    let _guard = heavy_lock();
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
    let _guard = heavy_lock();
    const WIDTH: u32 = 3000;
    const HEIGHT: u32 = 3000;
    /// Measurement threshold only: far above one row's work and far below a full decode of this
    /// image, so a loop that stopped checking between rows would fail.
    const BOUND_US: u128 = 250_000;

    let bytes = png_rgb(WIDTH, HEIGHT);
    let cancel = Cancellation::default();
    let cancelled_at: Arc<std::sync::Mutex<Option<Instant>>> =
        Arc::new(std::sync::Mutex::new(None));
    // One lock guards both the row counter and the moment of cancellation, so the baseline cannot
    // be inflated by rows the decode finishes between the flag being set and the count being read.
    // Reading the count and setting the flag happen under the same lock the row loop needs.
    let progress: Arc<std::sync::Mutex<u64>> = Arc::new(std::sync::Mutex::new(0));
    let rows_at_flag = Arc::new(AtomicU64::new(0));

    let helper_cancel = cancel.clone();
    let helper_stamp = Arc::clone(&cancelled_at);
    let helper_progress = Arc::clone(&progress);
    let helper_rows = Arc::clone(&rows_at_flag);
    let helper = std::thread::spawn(move || {
        // Wait for the decode to actually start before cancelling. A fixed sleep raced with the
        // first row under load: the flag could be set before row one, and the test then failed its
        // own "landed mid-image" assertion even though the code was right. Waiting for a counted
        // row makes the starting point an event; the cancel still comes from another thread.
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let rows = helper_progress.lock().expect("progress");
            if *rows > 0 {
                // Snapshot first, then set the flag, both while holding the lock the loop needs to
                // count its next row: no row can be counted between these two statements.
                helper_rows.store(*rows, Ordering::SeqCst);
                *helper_stamp.lock().expect("stamp") = Some(Instant::now());
                helper_cancel.cancel();
                drop(rows);
                return;
            }
            drop(rows);
            assert!(
                Instant::now() < deadline,
                "the decode never counted a row, so this measurement cannot start"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    });

    let loop_progress = Arc::clone(&progress);
    let result = media::decode_raster_with_row_hook(&bytes, limits(), &cancel, &mut |rows| {
        let mut counter = loop_progress.lock().expect("progress");
        *counter = u64::from(rows);
    });

    let returned = Instant::now();
    helper.join().expect("helper thread");
    let cancelled_moment = cancelled_at.lock().expect("stamp").expect("stamp set");
    let micros = returned.duration_since(cancelled_moment).as_micros();
    let stopped_at = *progress.lock().expect("progress");
    let baseline = rows_at_flag.load(Ordering::SeqCst);

    eprintln!(
        "external cancel mid-decode: {baseline} rows counted when the flag was set, stopped after \
         row {stopped_at} of {HEIGHT}, returned {micros} us after the flag was set"
    );
    assert_eq!(result.err(), Some(MediaError::Cancelled));
    assert!(
        stopped_at > 0 && stopped_at < u64::from(HEIGHT),
        "the cancellation must have landed mid-image, not before the first row or after the last"
    );
    // The row in progress when the flag was set may still finish, so one row of slack is the
    // expected allowance; anything beyond that means the loop kept decoding past the check.
    assert!(
        stopped_at <= baseline.saturating_add(1),
        "the loop stopped at row {stopped_at} although only {baseline} rows had been counted when \
         the flag was set: it kept decoding past the cancellation check"
    );
    assert!(
        micros <= BOUND_US,
        "returned {micros} us after cancellation, beyond this test's {BOUND_US} us threshold"
    );
}

/// A cancellation requested **before** the decode starts must stop every class at the door.
///
/// The row decoder can also stop mid-decode, but that is a different property: this measures the
/// pre-decode boundary that every class shares, so a cancelled request does not pay for a decode it
/// will throw away.
#[test]
fn a_pre_cancelled_request_stops_before_the_decoder_in_every_class() {
    let _guard = heavy_lock();
    let edge = 1024u32;
    let cases = [
        ("PNG", png_rgb(edge, edge)),
        ("JPEG", encoded_rgb(edge, edge, ImageFormat::Jpeg)),
        ("WebP", encoded_rgb(edge, edge, ImageFormat::WebP)),
        ("GIF", encoded_rgb(edge, edge, ImageFormat::Gif)),
    ];
    for (label, bytes) in cases {
        let cancel = Cancellation::default();
        cancel.cancel();
        let started = Instant::now();
        let result = media::decode_raster(&bytes, limits(), &cancel);
        let micros = started.elapsed().as_micros();
        eprintln!("pre-cancelled {label} {edge}x{edge}: {result:?} in {micros} us");
        assert_eq!(
            result.err(),
            Some(MediaError::Cancelled),
            "{label}: a request cancelled before the decode must report Cancelled"
        );
        // A decode of this frame would take milliseconds; the admission check returns far sooner.
        assert!(
            micros < 10_000,
            "{label}: a pre-cancelled request took {micros} us, which is decode-scale"
        );
    }
}

/// The pre-decode cancellation must be the **first** thing the decode does.
///
/// A timing bound alone would pass if the format sniff, the dimension read or the reservation moved
/// above the check: the work is small next to a decode. This reads the source and requires the
/// order `cancel.check()` -> format -> dimensions -> reservation -> decoder, so a reordering fails
/// here rather than showing up as a slightly slower cancelled call.
#[test]
fn the_pre_decode_cancellation_check_comes_before_any_work() {
    let source = include_str!("../src/raster.rs");
    let body_start = source
        .find("fn decode_raster_inner(")
        .expect("the decode entry point is present");
    // The range must be **this function only**: everything after its opening brace up to the brace
    // that closes it. Searching to the end of the file would let a later function satisfy a check
    // the decode path no longer performs.
    let after_signature = &source[body_start..];
    let open = after_signature
        .find('{')
        .expect("the decode entry point has a body");
    let mut depth = 0usize;
    let mut close = None;
    for (offset, ch) in after_signature[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + offset);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close.expect("the decode entry point has a closing brace");
    let body = &after_signature[..=close];
    // The extraction is pinned directly: the range must end at this function's closing brace, so a
    // later function in the same file cannot satisfy a check the decode path no longer performs.
    assert!(
        body.ends_with('}'),
        "the extracted range does not end at the function's closing brace"
    );
    assert!(
        !body.contains("fn decode_raster_with_hook"),
        "the extracted range ran past the decode entry point into the next function"
    );
    let opened = body.matches('{').count();
    let closed = body.matches('}').count();
    assert_eq!(
        opened, closed,
        "the extracted range is not brace-balanced, so it is not one function body"
    );
    let cancel = body
        .find("cancel.check()?;")
        .expect("the first cancellation check is present");
    // Every step must be **found** as well as ordered: a guard that skipped a missing needle would
    // pass silently after the code was renamed or the step moved into another function.
    for (label, needle) in [
        ("the format sniff", "AcceptedFormat::from_bytes"),
        ("the dimension read", "into_dimensions"),
        ("the reservation", "reserve_in_flight"),
        ("the decoder call", "decode_png_rows_inner"),
    ] {
        let position = body.find(needle).unwrap_or_else(|| {
            panic!("{label} is no longer in the decode path, so this guard is stale")
        });
        assert!(
            cancel < position,
            "{label} appears before the first cancellation check"
        );
    }
}

fn encoded_rgb(width: u32, height: u32, format: ImageFormat) -> Vec<u8> {
    let image = RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x % 251) as u8, (y % 241) as u8, 90])
    });
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image)
        .write_to(&mut bytes, format)
        .expect("encode the class");
    bytes.into_inner()
}
