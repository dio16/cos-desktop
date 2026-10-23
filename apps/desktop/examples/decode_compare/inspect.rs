//! Bounded, dependency-free inspector for the bytes a candidate wrote (MP-IMG2 Phase 2).
//!
//! The comparison harness must not trust a candidate's exit code or its self-report,
//! so it parses the output file itself and records only what it can actually read:
//! PNG (signature, IHDR width/height/bit depth/colour type, bounded chunk walk with
//! `IEND` detection and `iCCP`/`sRGB`/`gAMA` presence), BMP (`BM` file header plus
//! `BITMAPINFOHEADER`/V4/V5 width/height/bit count) and JPEG (`SOI`, `SOF0`/`SOF2`
//! precision, dimensions and component count).
//!
//! Every read is bounded by [`MAX_INSPECT_BYTES`], every chunk/segment walk by
//! [`MAX_INSPECT_STEPS`], and payloads the inspector does not need are skipped with
//! seeks rather than read, so a multi-megabyte PNG costs only its chunk headers.
//! Anything the inspector cannot recognise or cannot read stays `None`, which the
//! record renders as `not_measured`: this module never guesses, never repairs and
//! never validates pixel content, colour management or image data.

use std::io::{Read, Seek, SeekFrom};

/// Total bytes the inspector may read from one output file.
pub const MAX_INSPECT_BYTES: u64 = 64 * 1024;
/// Chunk/segment steps the inspector may walk in one output file.
pub const MAX_INSPECT_STEPS: usize = 4096;
/// Largest stated dimension the inspector accepts before treating a file as garbage.
const MAX_DIMENSION: u32 = 1 << 20;
/// Largest PNG chunk length the inspector is willing to skip over.
const MAX_PNG_CHUNK: u64 = 1 << 31;

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// Container the inspector recognised from the output signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Format {
    Png,
    Bmp,
    Jpeg,
    NotMeasured,
}

impl Format {
    /// Stable record token.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Bmp => "bmp",
            Self::Jpeg => "jpeg",
            Self::NotMeasured => "not_measured",
        }
    }
}

/// Colour type as far as the container states it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ColourType {
    Grey,
    Rgb,
    Palette,
    GreyAlpha,
    Rgba,
    NotMeasured,
}

impl ColourType {
    /// Stable record token.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Grey => "grey",
            Self::Rgb => "rgb",
            Self::Palette => "palette",
            Self::GreyAlpha => "grey_alpha",
            Self::Rgba => "rgba",
            Self::NotMeasured => "not_measured",
        }
    }
}

/// Colour-management markers the inspector actually saw in the output.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ColourMarker {
    Icc,
    Srgb,
    Gamma,
}

impl ColourMarker {
    /// Stable record token.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Icc => "icc",
            Self::Srgb => "srgb",
            Self::Gamma => "gamma",
        }
    }
}

/// What the inspector read from one output file. `None` means "not measured".
#[derive(Clone, Debug, Default)]
pub struct Inspection {
    /// Container recognised from the signature, or `None` when nothing matched.
    pub format: Option<Format>,
    /// Container terminator observed (`IEND` for PNG, `EOI`/`SOS` for JPEG, the whole
    /// header for BMP).
    pub complete: bool,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bit_depth: Option<u32>,
    pub channels: Option<u32>,
    pub colour_type: Option<ColourType>,
    /// Sorted, de-duplicated colour-management markers seen while walking.
    pub colour_markers: Vec<ColourMarker>,
}

impl Inspection {
    fn record(&mut self, marker: ColourMarker) {
        if !self.colour_markers.contains(&marker) {
            self.colour_markers.push(marker);
            self.colour_markers.sort();
        }
    }

    /// Dimensions are the minimum evidence needed to check an expectation.
    pub fn dimensions(&self) -> Option<(u32, u32)> {
        Some((self.width?, self.height?))
    }
}

/// Inspect one output file. The reader is consumed from its current position; the
/// caller passes a fresh handle opened at the start of the file.
pub fn inspect<R: Read + Seek>(reader: &mut R) -> Inspection {
    let Ok(length) = reader.seek(SeekFrom::End(0)) else {
        return Inspection::default();
    };
    if reader.seek(SeekFrom::Start(0)).is_err() {
        return Inspection::default();
    }
    let mut budget = MAX_INSPECT_BYTES;
    let Some(head) = read_bytes(reader, 8, &mut budget) else {
        return Inspection::default();
    };
    if head == PNG_SIGNATURE {
        // The reader is positioned on the first chunk header.
        return inspect_png(reader, budget);
    }
    if head.starts_with(b"BM") {
        // The reader is positioned after `BM` and six file-header bytes.
        return inspect_bmp(reader, budget, length);
    }
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        // Restart at SOI so the segment walk has one fixed starting point.
        if reader.seek(SeekFrom::Start(0)).is_err() {
            return Inspection::default();
        }
        return inspect_jpeg(reader, budget);
    }
    Inspection::default()
}

fn inspect_png<R: Read + Seek>(reader: &mut R, mut budget: u64) -> Inspection {
    let mut inspection = Inspection {
        format: Some(Format::Png),
        ..Inspection::default()
    };
    let mut ihdr_seen = false;
    for _ in 0..MAX_INSPECT_STEPS {
        let Some(length_bytes) = read_bytes(reader, 4, &mut budget) else {
            return inspection;
        };
        let length = u64::from(u32::from_be_bytes([
            length_bytes[0],
            length_bytes[1],
            length_bytes[2],
            length_bytes[3],
        ]));
        let Some(kind) = read_bytes(reader, 4, &mut budget) else {
            return inspection;
        };
        if length > MAX_PNG_CHUNK {
            return inspection;
        }
        // IHDR must be the first chunk and may appear exactly once (N1).
        if !ihdr_seen && kind.as_slice() != b"IHDR" {
            return inspection;
        }
        match kind.as_slice() {
            b"IHDR" => {
                if ihdr_seen || length != 13 {
                    return inspection;
                }
                let Some(payload) = read_bytes(reader, 13, &mut budget) else {
                    return inspection;
                };
                let width = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
                let height = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
                let bit_depth = u32::from(payload[8]);
                let colour_type = match payload[9] {
                    0 => Some(ColourType::Grey),
                    2 => Some(ColourType::Rgb),
                    3 => Some(ColourType::Palette),
                    4 => Some(ColourType::GreyAlpha),
                    6 => Some(ColourType::Rgba),
                    _ => None,
                };
                if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
                    return Inspection::default();
                }
                // A2: an illegal bit-depth/colour-type pair is not a PNG header.
                let Some(colour_type) = colour_type else {
                    return Inspection::default();
                };
                if !png_depth_allowed(colour_type, bit_depth) {
                    return Inspection::default();
                }
                inspection.width = Some(width);
                inspection.height = Some(height);
                inspection.bit_depth = Some(bit_depth);
                inspection.colour_type = Some(colour_type);
                inspection.channels = channels_of_colour_type(colour_type);
                ihdr_seen = true;
            }
            b"iCCP" => inspection.record(ColourMarker::Icc),
            b"sRGB" => inspection.record(ColourMarker::Srgb),
            b"gAMA" => inspection.record(ColourMarker::Gamma),
            b"IEND" => {
                // A real IEND states length 0 and terminates the image (N1).
                if length != 0 {
                    return inspection;
                }
                inspection.complete = true;
                return inspection;
            }
            _ => {}
        }
        // The IHDR payload is read above; every other chunk payload is skipped.
        if kind.as_slice() != b"IHDR" && length > 0 && skip_bytes(reader, length).is_none() {
            return inspection;
        }
        if skip_bytes(reader, 4).is_none() {
            return inspection;
        }
    }
    inspection
}

/// PNG bit depths the format allows for each colour type (A2).
fn png_depth_allowed(colour_type: ColourType, bit_depth: u32) -> bool {
    match colour_type {
        ColourType::Grey => matches!(bit_depth, 1 | 2 | 4 | 8 | 16),
        ColourType::Rgb | ColourType::GreyAlpha | ColourType::Rgba => matches!(bit_depth, 8 | 16),
        ColourType::Palette => matches!(bit_depth, 1 | 2 | 4 | 8),
        ColourType::NotMeasured => false,
    }
}

/// PNG colour type to sample channels, exactly as the format defines it.
fn channels_of_colour_type(colour_type: ColourType) -> Option<u32> {
    Some(match colour_type {
        ColourType::Grey => 1,
        ColourType::Rgb => 3,
        ColourType::Palette => 1,
        ColourType::GreyAlpha => 2,
        ColourType::Rgba => 4,
        ColourType::NotMeasured => return None,
    })
}

fn inspect_bmp<R: Read + Seek>(reader: &mut R, mut budget: u64, file_len: u64) -> Inspection {
    let mut inspection = Inspection {
        format: Some(Format::Bmp),
        ..Inspection::default()
    };
    // `BM` and six file-header bytes are already consumed; the data offset closes
    // the 14-byte file header.
    let Some(file_header) = read_bytes(reader, 6, &mut budget) else {
        return inspection;
    };
    let data_offset = u32::from_le_bytes([
        file_header[2],
        file_header[3],
        file_header[4],
        file_header[5],
    ]);
    let Some(header_size_bytes) = read_bytes(reader, 4, &mut budget) else {
        return inspection;
    };
    let header_size = u32::from_le_bytes([
        header_size_bytes[0],
        header_size_bytes[1],
        header_size_bytes[2],
        header_size_bytes[3],
    ]);
    if !matches!(header_size, 40 | 52 | 56 | 108 | 124) {
        return inspection;
    }
    // The whole declared header must be present before any field is trusted (B2).
    let header_end = u64::from(header_size).saturating_add(14);
    if file_len < header_end {
        return Inspection::default();
    }
    // width, height, planes, bit count and compression are the first 16 bytes of
    // every BITMAPINFOHEADER-compatible header.
    let Some(fields) = read_bytes(reader, 16, &mut budget) else {
        return inspection;
    };
    let width = i32::from_le_bytes([fields[0], fields[1], fields[2], fields[3]]);
    let height = i32::from_le_bytes([fields[4], fields[5], fields[6], fields[7]]);
    let bit_count = u32::from(u16::from_le_bytes([fields[10], fields[11]]));
    let compression = u32::from_le_bytes([fields[12], fields[13], fields[14], fields[15]]);
    let Ok(width) = u32::try_from(width) else {
        return inspection;
    };
    let height = height.unsigned_abs();
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return inspection;
    }
    // A2: only the bit counts the format defines may be used.
    if !matches!(bit_count, 1 | 4 | 8 | 16 | 24 | 32) {
        return Inspection::default();
    }
    // The stated pixel-data offset must sit inside the file and after the headers.
    if u64::from(data_offset) < header_end || u64::from(data_offset) > file_len {
        return Inspection::default();
    }
    inspection.width = Some(width);
    inspection.height = Some(height);
    inspection.bit_depth = Some(bit_count);
    // Uncompressed true-colour bitmaps state their channel count through the bit
    // count; paletted and compressed forms do not, so they stay not_measured.
    if compression == 0 && matches!(bit_count, 24 | 32) {
        inspection.channels = Some(bit_count / 8);
        inspection.colour_type = Some(if bit_count == 24 {
            ColourType::Rgb
        } else {
            ColourType::Rgba
        });
    }
    inspection.complete = true;
    inspection
}

fn inspect_jpeg<R: Read + Seek>(reader: &mut R, mut budget: u64) -> Inspection {
    let mut inspection = Inspection {
        format: Some(Format::Jpeg),
        ..Inspection::default()
    };
    let Some(soi) = read_bytes(reader, 2, &mut budget) else {
        return inspection;
    };
    if soi != [0xFF, 0xD8] {
        return inspection;
    }
    for _ in 0..MAX_INSPECT_STEPS {
        let Some(prefix) = read_bytes(reader, 1, &mut budget) else {
            return inspection;
        };
        if prefix[0] != 0xFF {
            return inspection;
        }
        // Marker codes may be preceded by any number of 0xFF fill bytes.
        let marker = loop {
            let Some(code) = read_bytes(reader, 1, &mut budget) else {
                return inspection;
            };
            if code[0] != 0xFF {
                break code[0];
            }
        };
        match marker {
            0x00 | 0xFF => return inspection,
            0xD8 => continue,
            0xD9 => {
                inspection.complete = true;
                return inspection;
            }
            0x01 | 0xD0..=0xD7 => continue,
            0xDA => {
                // Start of scan: entropy-coded data follows, so the walk ends here.
                inspection.complete = inspection.width.is_some();
                return inspection;
            }
            _ => {}
        }
        let Some(length_bytes) = read_bytes(reader, 2, &mut budget) else {
            return inspection;
        };
        let length = u64::from(u16::from_be_bytes([length_bytes[0], length_bytes[1]]));
        if length < 2 {
            return inspection;
        }
        let payload = length - 2;
        if matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            // The SOF payload must carry all six bytes before any of them is read,
            // and nothing past the declared segment may be consumed (B2).
            if payload < 6 {
                return Inspection::default();
            }
            let Some(header) = read_bytes(reader, 6, &mut budget) else {
                return inspection;
            };
            let precision = u32::from(header[0]);
            let height = u32::from(u16::from_be_bytes([header[1], header[2]]));
            let width = u32::from(u16::from_be_bytes([header[3], header[4]]));
            let components = u32::from(header[5]);
            if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
                return Inspection::default();
            }
            // A2: JPEG precision and component count are closed sets.
            if !matches!(precision, 8 | 12 | 16) || !(1..=4).contains(&components) {
                return Inspection::default();
            }
            inspection.width = Some(width);
            inspection.height = Some(height);
            inspection.bit_depth = Some(precision);
            inspection.channels = Some(components);
            inspection.colour_type = match components {
                1 => Some(ColourType::Grey),
                3 => Some(ColourType::Rgb),
                _ => None,
            };
            if payload > 6 && skip_bytes(reader, payload - 6).is_none() {
                return inspection;
            }
            continue;
        }
        if payload > 0 && skip_bytes(reader, payload).is_none() {
            return inspection;
        }
    }
    inspection
}

fn read_bytes<R: Read>(reader: &mut R, count: usize, budget: &mut u64) -> Option<Vec<u8>> {
    let requested = u64::try_from(count).ok()?;
    if requested > *budget {
        return None;
    }
    let mut bytes = vec![0_u8; count];
    reader.read_exact(&mut bytes).ok()?;
    *budget -= requested;
    Some(bytes)
}

fn skip_bytes<R: Seek>(reader: &mut R, count: u64) -> Option<()> {
    let distance = i64::try_from(count).ok()?;
    reader.seek(SeekFrom::Current(distance)).ok()?;
    Some(())
}
