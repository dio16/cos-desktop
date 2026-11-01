//! MP-IMG2 measurement probe: one known-size static 8-bit RGB PNG decode.
//!
//! `scripts/measure-decode-peak.ps1` starts this binary, waits for the ready file, samples
//! the process working set, writes the go file and samples until the process exits.
//! `scripts/measure-img2-job.ps1` runs the same binary inside a Job Object process-memory
//! cap and in a cancellation mode.
//!
//! The fixture is generated in-process so the measurement binary is self-contained; the
//! encoded bytes stay live during the decode.
//!
//! Usage: `peak_probe <width> <height> <ready-path> <go-path>
//!         [<decode-start> <decode-done>] [--cancel-file <path>] [--result-file <path>]`.
//!
//! `--result-file` writes the same result lines to a file (the Job-Object harness starts
//! the process suspended without standard-output redirection); the cancel line is written
//! to `<result-file>.cancel` so two writers never share a file.
//!
//! With `--cancel-file`, a watcher thread cancels the decode when the file appears and
//! reports `cancel-requested-at-ms` plus, when the decode returns, `decode-result
//! cancelled typed=cancelled` and `decode-returned-after-cancel-ms`. The image decoder
//! itself is not interruptible; the delay is bounded by the remaining decode time.

use image::{DynamicImage, ImageFormat, RgbImage};
use media::{Cancellation, DecodeBudget, MediaError, decode_raster_joined};
use std::io::{Cursor, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut raw: Vec<String> = std::env::args().skip(1).collect();
    let mut cancel_file: Option<PathBuf> = None;
    if let Some(position) = raw.iter().position(|argument| argument == "--cancel-file") {
        if position + 1 < raw.len() {
            cancel_file = Some(PathBuf::from(raw.remove(position + 1)));
        }
        raw.remove(position);
    }
    let mut result_file: Option<PathBuf> = None;
    if let Some(position) = raw.iter().position(|argument| argument == "--result-file") {
        if position + 1 < raw.len() {
            result_file = Some(PathBuf::from(raw.remove(position + 1)));
        }
        raw.remove(position);
    }
    let cancel_line_file = result_file
        .as_ref()
        .map(|path| PathBuf::from(format!("{}.cancel", path.display())));
    let mut args = raw.into_iter();

    let width: u32 = args.next().unwrap_or_else(|| "2048".into()).parse()?;
    let height: u32 = args.next().unwrap_or_else(|| "2048".into()).parse()?;
    let ready = PathBuf::from(args.next().ok_or("missing ready path")?);
    let go = PathBuf::from(args.next().ok_or("missing go path")?);
    let start_marker = args.next().map(PathBuf::from);
    let done_marker = args.next().map(PathBuf::from);

    let image = RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x % 251) as u8, (y % 241) as u8, 90])
    });
    let mut encoded = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image).write_to(&mut encoded, ImageFormat::Png)?;
    let encoded = encoded.into_inner();

    let mut result_lines: Vec<String> = Vec::new();
    let emit = |lines: &mut Vec<String>, line: String| {
        println!("{line}");
        let _ = std::io::stdout().flush();
        lines.push(line);
    };
    emit(
        &mut result_lines,
        format!("fixture-ready bytes={}", encoded.len()),
    );
    std::fs::write(&ready, b"ready")?;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !go.exists() {
        if std::time::Instant::now() > deadline {
            return Err("the go file did not appear within 30 s".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }

    let limits = DecodeBudget::ratified_default().limits()?;
    if let Some(marker) = start_marker.as_ref() {
        std::fs::write(marker, b"start")?;
    }
    emit(&mut result_lines, "decode-start".to_string());

    let cancel = Cancellation::default();
    let requested_at: Arc<Mutex<Option<std::time::Instant>>> = Arc::new(Mutex::new(None));
    let base = std::time::Instant::now();
    let decode_started = base;
    if let Some(path) = cancel_file {
        let cancel = cancel.clone();
        let requested_at = Arc::clone(&requested_at);
        let base = decode_started;
        let cancel_line_file = cancel_line_file.clone();
        std::thread::spawn(move || {
            let deadline = base + std::time::Duration::from_secs(60);
            while !path.exists() {
                if std::time::Instant::now() > deadline {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            let at = base.elapsed();
            if let Ok(mut slot) = requested_at.lock() {
                *slot = Some(base + at);
            }
            cancel.cancel();
            let line = format!("cancel-requested-at-ms={}", at.as_millis());
            println!("{line}");
            let _ = std::io::stdout().flush();
            if let Some(path) = cancel_line_file.as_ref() {
                let _ = std::fs::write(path, line);
            }
        });
    }

    // The product path owns a scoped worker and joins it before returning, so this proves
    // cancellation and reaping of the joined worker, not of a direct decode call.
    let result = decode_raster_joined(&encoded, limits, &cancel);
    let decode_finished = std::time::Instant::now();
    let decode_ms = decode_started.elapsed().as_millis();
    if let Some(marker) = done_marker.as_ref() {
        std::fs::write(marker, b"done")?;
    }
    let requested = requested_at.lock().ok().and_then(|slot| *slot);
    let start_ms = decode_started.duration_since(base).as_millis();
    let done_ms = decode_finished.duration_since(base).as_millis();
    let cancel_ms = requested.map(|at| at.duration_since(base).as_millis());
    let cancel_within_decode = cancel_ms
        .map(|c| c >= start_ms && c <= done_ms)
        .unwrap_or(false);
    emit(&mut result_lines, "decode-path=joined".to_string());
    emit(&mut result_lines, format!("decode-start-at-ms={start_ms}"));
    emit(
        &mut result_lines,
        format!("decode-returned-at-ms={done_ms}"),
    );
    if let Some(cancel_ms) = cancel_ms {
        emit(
            &mut result_lines,
            format!("cancel-within-decode={cancel_within_decode}"),
        );
        emit(&mut result_lines, format!("cancel-at-ms={cancel_ms}"));
    }
    match result {
        Ok(raster) => {
            emit(
                &mut result_lines,
                format!("decode-result ok rgba-bytes={}", raster.rgba.len()),
            );
        }
        Err(MediaError::Cancelled) => {
            emit(
                &mut result_lines,
                "decode-result cancelled typed=cancelled".to_string(),
            );
            if let Some(at) = requested {
                emit(
                    &mut result_lines,
                    format!(
                        "decode-returned-after-cancel-ms={}",
                        at.elapsed().as_millis()
                    ),
                );
            }
        }
        Err(other) => {
            emit(&mut result_lines, format!("decode-result error {other:?}"));
            return Err(format!("the decode failed: {other:?}").into());
        }
    }
    emit(&mut result_lines, format!("decode-ms={decode_ms}"));
    if let Some(path) = result_file.as_ref() {
        std::fs::write(
            path,
            result_lines.join(
                "
",
            ),
        )?;
    }
    Ok(())
}
