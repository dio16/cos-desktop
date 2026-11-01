//! MP-IMG2 measurement probe: one known-size static 8-bit RGB PNG decode.
//!
//! `scripts/measure-decode-peak.ps1` starts this binary, waits for the ready file,
//! samples the process working set (baseline), writes the go file, then samples the
//! working set until the process exits. The fixture is generated in-process so the
//! measurement binary is self-contained; the encoded bytes stay live during the decode
//! (the baseline is sampled after generation and includes them).
//!
//! Usage: `peak_probe <width> <height> <ready-path> <go-path> [<decode-start> <decode-done>]`.
//! The optional marker paths bracket the decode call so the harness can count samples
//! taken while the decode is running.

use image::{DynamicImage, ImageFormat, RgbImage};
use media::{Cancellation, DecodeBudget, decode_raster};
use std::io::{Cursor, Write};
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
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

    println!("fixture-ready bytes={}", encoded.len());
    std::io::stdout().flush()?;
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
    println!("decode-start");
    std::io::stdout().flush()?;
    let raster = decode_raster(&encoded, limits, &Cancellation::default())?;
    if let Some(marker) = done_marker.as_ref() {
        std::fs::write(marker, b"done")?;
    }
    println!("decode-done rgba-bytes={}", raster.rgba.len());
    Ok(())
}
