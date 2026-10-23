//! Bounded, std-only PNG pixel oracle for MP-IMG2 Phase 3.
//!
//! The comparison harness verifies *decoded pixels*, not only container structure.
//! This module is the independent side of that check: it reads the bytes the
//! candidate wrote, inflates the image data with its own DEFLATE decoder (no
//! dependency, no `unsafe`), reverses the PNG scanline filters, expands the samples
//! to row-major RGBA and hashes those bytes with its own SHA-256. Nothing here
//! trusts the candidate, a platform codec or another crate.
//!
//! Supported subset — exactly what the harness itself produces and nothing else:
//! 8-bit depth, non-interlaced, compression method 0, filter method 0, and colour
//! type 0 (gray), 2 (RGB), 4 (gray+alpha) or 6 (RGBA). Anything outside that subset
//! is reported as [`PixelError::UnsupportedSubset`], so the record says
//! `not_measured`; the oracle never guesses a pixel value.
//!
//! Bounds: at most [`MAX_RGBA_BYTES`] of RGBA output (checked *before* anything is
//! allocated), at most [`MAX_IDAT_BYTES`] of compressed image data, and the inflate
//! output is capped at exactly the number of bytes the declared geometry needs, so a
//! decompression bomb stops at the cap instead of growing.
//!
//! Integrity scope (NIT-1): this oracle verifies **decoded pixel values**, not
//! checksums. PNG chunk CRCs are not recomputed and the zlib Adler-32 trailer is not
//! checked, so bytes that decode to the expected RGBA can verify even when a CRC or
//! the Adler-32 is corrupt. Palette, 16-bit and interlaced PNGs are outside the
//! supported subset and are reported as `unsupported_subset`. Every chunk walk is
//! bounded by [`MAX_CHUNKS`] and every read is checked, so a malformed or adversarial
//! file yields an error instead of a panic or an unbounded allocation.

use std::io::{Read, Seek, SeekFrom};

/// Largest RGBA buffer this oracle will allocate.
pub const MAX_RGBA_BYTES: usize = 16 * 1024 * 1024;
/// Largest concatenated image-data stream accepted.
pub const MAX_IDAT_BYTES: usize = 8 * 1024 * 1024;
/// Chunk-walk bound for one file.
pub const MAX_CHUNKS: usize = 64 * 1024;

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// Why the decoded pixels could not be produced. Each variant has a stable token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelError {
    /// The bytes are not a PNG at all.
    NotPng,
    /// No IHDR chunk before the image data.
    MissingIhdr,
    /// The file ends before its terminator or before a declared chunk payload.
    Truncated,
    /// The container is malformed (illegal IHDR, bad filter, broken stream).
    Corrupt,
    /// Valid PNG, but outside the supported subset (palette, 16-bit, interlaced…).
    UnsupportedSubset,
    /// The declared geometry exceeds the pixel or image-data cap.
    TooLarge,
    /// The compressed stream is not a valid DEFLATE stream.
    InflateCorrupt,
    /// The stream asked for more output than the declared geometry needs.
    InflateLimit,
    /// The file could not be read or seeked.
    Unreadable,
}

impl PixelError {
    /// Stable record token.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotPng => "not_png",
            Self::MissingIhdr => "missing_ihdr",
            Self::Truncated => "truncated",
            Self::Corrupt => "corrupt",
            Self::UnsupportedSubset => "unsupported_subset",
            Self::TooLarge => "too_large",
            Self::InflateCorrupt => "inflate_corrupt",
            Self::InflateLimit => "inflate_limit",
            Self::Unreadable => "unreadable",
        }
    }
}

/// Decoded pixels of one output file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    /// Row-major RGBA, four bytes per pixel, exactly `width * height * 4` long.
    pub pixels: Vec<u8>,
}

impl RgbaImage {
    /// SHA-256 over the RGBA bytes; this is the value an operator expectation names.
    pub fn digest(&self) -> [u8; 32] {
        sha256(&self.pixels)
    }
}

/// Read the decoded pixels of one PNG file, bounded and dependency-free.
pub fn read_png<R: Read + Seek>(reader: &mut R) -> Result<RgbaImage, PixelError> {
    if reader.seek(SeekFrom::Start(0)).is_err() {
        return Err(PixelError::Unreadable);
    }
    let mut signature = [0_u8; 8];
    if reader.read_exact(&mut signature).is_err() {
        return Err(PixelError::NotPng);
    }
    if signature != PNG_SIGNATURE {
        return Err(PixelError::NotPng);
    }

    let mut header: Option<Header> = None;
    let mut idat: Vec<u8> = Vec::new();
    let mut saw_end = false;
    for _ in 0..MAX_CHUNKS {
        let mut length_bytes = [0_u8; 4];
        if reader.read_exact(&mut length_bytes).is_err() {
            break;
        }
        let length = u64::from(u32::from_be_bytes(length_bytes));
        let mut kind = [0_u8; 4];
        if reader.read_exact(&mut kind).is_err() {
            break;
        }
        match &kind {
            b"IHDR" => {
                if header.is_some() || length != 13 {
                    return Err(PixelError::Corrupt);
                }
                let mut payload = [0_u8; 13];
                if reader.read_exact(&mut payload).is_err() {
                    return Err(PixelError::Truncated);
                }
                header = Some(Header::parse(&payload)?);
                if skip(reader, 4).is_none() {
                    return Err(PixelError::Truncated);
                }
            }
            b"IDAT" => {
                if header.is_none() {
                    return Err(PixelError::Corrupt);
                }
                let take = usize::try_from(length).map_err(|_| PixelError::TooLarge)?;
                if take > MAX_IDAT_BYTES || idat.len().saturating_add(take) > MAX_IDAT_BYTES {
                    return Err(PixelError::TooLarge);
                }
                let mut chunk = vec![0_u8; take];
                if reader.read_exact(&mut chunk).is_err() {
                    return Err(PixelError::Truncated);
                }
                idat.extend_from_slice(&chunk);
                if skip(reader, 4).is_none() {
                    return Err(PixelError::Truncated);
                }
            }
            b"IEND" => {
                saw_end = true;
                break;
            }
            _ => {
                if skip(reader, length.saturating_add(4)).is_none() {
                    return Err(PixelError::Truncated);
                }
            }
        }
    }

    let header = header.ok_or(PixelError::MissingIhdr)?;
    if !saw_end {
        return Err(PixelError::Truncated);
    }
    if idat.is_empty() {
        return Err(PixelError::Truncated);
    }

    let width = usize::try_from(header.width).map_err(|_| PixelError::TooLarge)?;
    let height = usize::try_from(header.height).map_err(|_| PixelError::TooLarge)?;
    let row_bytes = width
        .checked_mul(header.channels)
        .ok_or(PixelError::TooLarge)?;
    // Exactly the scanlines this geometry needs; the inflate stops here.
    let expected = row_bytes
        .checked_add(1)
        .and_then(|row| row.checked_mul(height))
        .ok_or(PixelError::TooLarge)?;
    let rgba_bytes = width
        .checked_mul(height)
        .and_then(|count| count.checked_mul(4))
        .ok_or(PixelError::TooLarge)?;
    // The cap is enforced before the RGBA buffer exists, so a huge claimed dimension
    // is refused without allocating anything.
    if rgba_bytes > MAX_RGBA_BYTES {
        return Err(PixelError::TooLarge);
    }

    let raw = inflate(&idat, expected)?;
    if raw.len() != expected {
        return Err(PixelError::Truncated);
    }
    let pixels = unfilter_and_expand(&header, &raw, rgba_bytes)?;
    Ok(RgbaImage {
        width: header.width,
        height: header.height,
        pixels,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Colour {
    Gray,
    Rgb,
    GrayAlpha,
    Rgba,
}

#[derive(Clone, Copy, Debug)]
struct Header {
    width: u32,
    height: u32,
    channels: usize,
    colour: Colour,
}

impl Header {
    fn parse(payload: &[u8; 13]) -> Result<Self, PixelError> {
        let width = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let height = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
        let bit_depth = payload[8];
        let colour_type = payload[9];
        let compression = payload[10];
        let filter = payload[11];
        let interlace = payload[12];
        if width == 0 || height == 0 {
            return Err(PixelError::Corrupt);
        }
        if compression != 0 || filter != 0 || interlace != 0 || bit_depth != 8 {
            return Err(PixelError::UnsupportedSubset);
        }
        let (colour, channels) = match colour_type {
            0 => (Colour::Gray, 1),
            2 => (Colour::Rgb, 3),
            4 => (Colour::GrayAlpha, 2),
            6 => (Colour::Rgba, 4),
            _ => return Err(PixelError::UnsupportedSubset),
        };
        Ok(Self {
            width,
            height,
            channels,
            colour,
        })
    }
}

fn skip<R: Seek>(reader: &mut R, distance: u64) -> Option<()> {
    let distance = i64::try_from(distance).ok()?;
    reader.seek(SeekFrom::Current(distance)).ok()?;
    Some(())
}

fn unfilter_and_expand(
    header: &Header,
    raw: &[u8],
    rgba_bytes: usize,
) -> Result<Vec<u8>, PixelError> {
    let row_bytes = header.width as usize * header.channels;
    let height = header.height as usize;
    let mut pixels = Vec::with_capacity(rgba_bytes);
    let mut previous = vec![0_u8; row_bytes];
    let mut current = vec![0_u8; row_bytes];
    for row in 0..height {
        let start = row * (row_bytes + 1);
        let filter = raw[start];
        let body = raw
            .get(start + 1..start + 1 + row_bytes)
            .ok_or(PixelError::Truncated)?;
        current.copy_from_slice(body);
        unfilter_row(filter, &mut current, &previous, header.channels)?;
        expand_row(header, &current, &mut pixels);
        previous.copy_from_slice(&current);
    }
    if pixels.len() != rgba_bytes {
        return Err(PixelError::Corrupt);
    }
    Ok(pixels)
}

fn unfilter_row(filter: u8, row: &mut [u8], previous: &[u8], bpp: usize) -> Result<(), PixelError> {
    match filter {
        0 => {}
        1 => {
            for index in bpp..row.len() {
                row[index] = row[index].wrapping_add(row[index - bpp]);
            }
        }
        2 => {
            for index in 0..row.len() {
                row[index] = row[index].wrapping_add(previous[index]);
            }
        }
        3 => {
            for index in 0..row.len() {
                let left = if index >= bpp {
                    u16::from(row[index - bpp])
                } else {
                    0
                };
                let up = u16::from(previous[index]);
                row[index] = row[index].wrapping_add(((left + up) / 2) as u8);
            }
        }
        4 => {
            for index in 0..row.len() {
                let left = if index >= bpp { row[index - bpp] } else { 0 };
                let up = previous[index];
                let corner = if index >= bpp {
                    previous[index - bpp]
                } else {
                    0
                };
                row[index] = row[index].wrapping_add(paeth(left, up, corner));
            }
        }
        _ => return Err(PixelError::Corrupt),
    }
    Ok(())
}

fn paeth(left: u8, up: u8, corner: u8) -> u8 {
    let left = i16::from(left);
    let up = i16::from(up);
    let corner = i16::from(corner);
    let estimate = left + up - corner;
    let to_left = (estimate - left).abs();
    let to_up = (estimate - up).abs();
    let to_corner = (estimate - corner).abs();
    if to_left <= to_up && to_left <= to_corner {
        left as u8
    } else if to_up <= to_corner {
        up as u8
    } else {
        corner as u8
    }
}

fn expand_row(header: &Header, row: &[u8], pixels: &mut Vec<u8>) {
    match header.colour {
        Colour::Gray => {
            for &value in row {
                pixels.extend_from_slice(&[value, value, value, 255]);
            }
        }
        Colour::Rgb => {
            for chunk in row.as_chunks::<3>().0 {
                pixels.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 255]);
            }
        }
        Colour::GrayAlpha => {
            for chunk in row.as_chunks::<2>().0 {
                pixels.extend_from_slice(&[chunk[0], chunk[0], chunk[0], chunk[1]]);
            }
        }
        Colour::Rgba => pixels.extend_from_slice(row),
    }
}

// ---------------------------------------------------------------------------
// SHA-256 (FIPS 180-4), std only.
// ---------------------------------------------------------------------------

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256 of `bytes` (FIPS 180-4), implemented here so the record's digest never
/// depends on an external crate.
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut message = bytes.to_vec();
    let bit_length = (bytes.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_length.to_be_bytes());

    for block in message.as_chunks::<64>().0 {
        let mut words = [0_u32; 64];
        for (index, chunk) in block.as_chunks::<4>().0.iter().enumerate() {
            words[index] = u32::from_be_bytes(*chunk);
        }
        for index in 16..64 {
            let s0 = words[index - 15].rotate_right(7)
                ^ words[index - 15].rotate_right(18)
                ^ (words[index - 15] >> 3);
            let s1 = words[index - 2].rotate_right(17)
                ^ words[index - 2].rotate_right(19)
                ^ (words[index - 2] >> 10);
            words[index] = words[index - 16]
                .wrapping_add(s0)
                .wrapping_add(words[index - 7])
                .wrapping_add(s1);
        }
        let mut working = state;
        for index in 0..64 {
            let s1 = working[4].rotate_right(6)
                ^ working[4].rotate_right(11)
                ^ working[4].rotate_right(25);
            let choose = (working[4] & working[5]) ^ (!working[4] & working[6]);
            let temp1 = working[7]
                .wrapping_add(s1)
                .wrapping_add(choose)
                .wrapping_add(SHA256_K[index])
                .wrapping_add(words[index]);
            let s0 = working[0].rotate_right(2)
                ^ working[0].rotate_right(13)
                ^ working[0].rotate_right(22);
            let majority =
                (working[0] & working[1]) ^ (working[0] & working[2]) ^ (working[1] & working[2]);
            let temp2 = s0.wrapping_add(majority);
            working[7] = working[6];
            working[6] = working[5];
            working[5] = working[4];
            working[4] = working[3].wrapping_add(temp1);
            working[3] = working[2];
            working[2] = working[1];
            working[1] = working[0];
            working[0] = temp1.wrapping_add(temp2);
        }
        for index in 0..8 {
            state[index] = state[index].wrapping_add(working[index]);
        }
    }

    let mut digest = [0_u8; 32];
    for (index, word) in state.iter().enumerate() {
        digest[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    digest
}

/// Lowercase hex of a byte slice.
pub fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        text.push(char::from_digit(u32::from(byte & 0x0F), 16).unwrap_or('0'));
    }
    text
}

/// Parse exactly 64 lowercase or uppercase hex digits into 32 bytes.
pub fn parse_digest(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut digest = [0_u8; 32];
    let bytes = text.as_bytes();
    for index in 0..32 {
        let high = hex_value(bytes[index * 2])?;
        let low = hex_value(bytes[index * 2 + 1])?;
        digest[index] = (high << 4) | low;
    }
    Some(digest)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// DEFLATE / zlib (RFC 1950/1951), std only and bounded.
// ---------------------------------------------------------------------------

/// Inflate a zlib stream into at most `cap` bytes.
pub fn inflate(input: &[u8], cap: usize) -> Result<Vec<u8>, PixelError> {
    if input.len() < 2 {
        return Err(PixelError::InflateCorrupt);
    }
    let method = input[0];
    let flags = input[1];
    if method & 0x0F != 8 || method >> 4 > 7 {
        return Err(PixelError::InflateCorrupt);
    }
    if (u16::from(method) * 256 + u16::from(flags)) % 31 != 0 {
        return Err(PixelError::InflateCorrupt);
    }
    if flags & 0x20 != 0 {
        return Err(PixelError::UnsupportedSubset);
    }
    let body = input.get(2..).ok_or(PixelError::InflateCorrupt)?;
    let mut reader = Bits::new(body);
    let mut output: Vec<u8> = Vec::new();
    loop {
        let last = reader.bits(1)?;
        let kind = reader.bits(2)?;
        match kind {
            0 => stored(&mut reader, &mut output, cap)?,
            1 => codes(
                &mut reader,
                &mut output,
                cap,
                &fixed_literal(),
                &fixed_distance(),
            )?,
            2 => dynamic(&mut reader, &mut output, cap)?,
            _ => return Err(PixelError::InflateCorrupt),
        }
        if last == 1 {
            break;
        }
    }
    Ok(output)
}

struct Bits<'a> {
    data: &'a [u8],
    position: usize,
    bit: u32,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            position: 0,
            bit: 0,
        }
    }

    fn bit(&mut self) -> Result<u32, PixelError> {
        let byte = *self
            .data
            .get(self.position)
            .ok_or(PixelError::InflateCorrupt)?;
        let value = u32::from((byte >> self.bit) & 1);
        self.bit += 1;
        if self.bit == 8 {
            self.bit = 0;
            self.position += 1;
        }
        Ok(value)
    }

    fn bits(&mut self, count: u32) -> Result<u32, PixelError> {
        let mut value = 0_u32;
        for index in 0..count {
            value |= self.bit()? << index;
        }
        Ok(value)
    }

    fn align(&mut self) {
        if self.bit != 0 {
            self.bit = 0;
            self.position += 1;
        }
    }
}

fn stored(reader: &mut Bits<'_>, output: &mut Vec<u8>, cap: usize) -> Result<(), PixelError> {
    reader.align();
    let length = reader.bits(16)? as usize;
    let complement = reader.bits(16)? as usize;
    if length != (!complement & 0xFFFF) {
        return Err(PixelError::InflateCorrupt);
    }
    if output.len().saturating_add(length) > cap {
        return Err(PixelError::InflateLimit);
    }
    let start = reader.position;
    let end = start
        .checked_add(length)
        .ok_or(PixelError::InflateCorrupt)?;
    let bytes = reader
        .data
        .get(start..end)
        .ok_or(PixelError::InflateCorrupt)?;
    output.extend_from_slice(bytes);
    reader.position = end;
    Ok(())
}

struct Huffman {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

fn huffman(lengths: &[u16]) -> Result<Huffman, PixelError> {
    let mut counts = [0_u16; 16];
    for &length in lengths {
        if length > 15 {
            return Err(PixelError::InflateCorrupt);
        }
        counts[length as usize] += 1;
    }
    counts[0] = 0;
    // A single-symbol code is legal; an over-subscribed code is not.
    let mut left = 1_i32;
    for count in counts.iter().skip(1).take(15) {
        left <<= 1;
        left -= i32::from(*count);
        if left < 0 {
            return Err(PixelError::InflateCorrupt);
        }
    }
    let mut offsets = [0_u16; 16];
    for length in 1..15 {
        offsets[length + 1] = offsets[length] + counts[length];
    }
    let mut symbols = vec![0_u16; lengths.len()];
    for (symbol, &length) in lengths.iter().enumerate() {
        if length != 0 {
            let slot = offsets[length as usize] as usize;
            if slot >= symbols.len() {
                return Err(PixelError::InflateCorrupt);
            }
            symbols[slot] = symbol as u16;
            offsets[length as usize] += 1;
        }
    }
    Ok(Huffman { counts, symbols })
}

impl Huffman {
    fn decode(&self, reader: &mut Bits<'_>) -> Result<u16, PixelError> {
        let mut code = 0_i32;
        let mut first = 0_i32;
        let mut index = 0_i32;
        for length in 1..16 {
            code |= reader.bit()? as i32;
            let count = i32::from(self.counts[length]);
            if code - count < first {
                let slot = index + (code - first);
                let symbol = *self
                    .symbols
                    .get(usize::try_from(slot).map_err(|_| PixelError::InflateCorrupt)?)
                    .ok_or(PixelError::InflateCorrupt)?;
                return Ok(symbol);
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err(PixelError::InflateCorrupt)
    }
}

fn fixed_literal() -> Huffman {
    let mut lengths = [0_u16; 288];
    for (symbol, length) in lengths.iter_mut().enumerate() {
        *length = match symbol {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    huffman(&lengths).unwrap_or(Huffman {
        counts: [0; 16],
        symbols: Vec::new(),
    })
}

fn fixed_distance() -> Huffman {
    huffman(&[5_u16; 32]).unwrap_or(Huffman {
        counts: [0; 16],
        symbols: Vec::new(),
    })
}

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u32; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DISTANCE_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DISTANCE_EXTRA: [u32; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

fn codes(
    reader: &mut Bits<'_>,
    output: &mut Vec<u8>,
    cap: usize,
    literal: &Huffman,
    distance: &Huffman,
) -> Result<(), PixelError> {
    loop {
        let symbol = literal.decode(reader)?;
        if symbol < 256 {
            if output.len() >= cap {
                return Err(PixelError::InflateLimit);
            }
            output.push(symbol as u8);
        } else if symbol == 256 {
            return Ok(());
        } else {
            let index = usize::from(symbol - 257);
            let base = *LENGTH_BASE.get(index).ok_or(PixelError::InflateCorrupt)?;
            let extra = *LENGTH_EXTRA.get(index).ok_or(PixelError::InflateCorrupt)?;
            let length = u32::from(base) + reader.bits(extra)?;
            let distance_symbol = usize::from(distance.decode(reader)?);
            let base = *DISTANCE_BASE
                .get(distance_symbol)
                .ok_or(PixelError::InflateCorrupt)?;
            let extra = *DISTANCE_EXTRA
                .get(distance_symbol)
                .ok_or(PixelError::InflateCorrupt)?;
            let distance = u32::from(base) + reader.bits(extra)?;
            let distance = usize::try_from(distance).map_err(|_| PixelError::InflateCorrupt)?;
            if distance == 0 || distance > output.len() {
                return Err(PixelError::InflateCorrupt);
            }
            let length = usize::try_from(length).map_err(|_| PixelError::InflateCorrupt)?;
            if output.len().saturating_add(length) > cap {
                return Err(PixelError::InflateLimit);
            }
            for _ in 0..length {
                let byte = output[output.len() - distance];
                output.push(byte);
            }
        }
    }
}

fn dynamic(reader: &mut Bits<'_>, output: &mut Vec<u8>, cap: usize) -> Result<(), PixelError> {
    const ORDER: [usize; 19] = [
        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];
    let literal_count = reader.bits(5)? as usize + 257;
    let distance_count = reader.bits(5)? as usize + 1;
    let code_count = reader.bits(4)? as usize + 4;
    if literal_count > 286 || distance_count > 30 || code_count > 19 {
        return Err(PixelError::InflateCorrupt);
    }
    let mut code_lengths = [0_u16; 19];
    for &slot in ORDER.iter().take(code_count) {
        code_lengths[slot] = reader.bits(3)? as u16;
    }
    let code_huffman = huffman(&code_lengths)?;
    let mut lengths = vec![0_u16; literal_count + distance_count];
    let mut index = 0;
    while index < lengths.len() {
        let symbol = code_huffman.decode(reader)?;
        match symbol {
            0..=15 => {
                lengths[index] = symbol;
                index += 1;
            }
            16 => {
                if index == 0 {
                    return Err(PixelError::InflateCorrupt);
                }
                let previous = lengths[index - 1];
                let repeat = 3 + reader.bits(2)? as usize;
                for _ in 0..repeat {
                    if index >= lengths.len() {
                        return Err(PixelError::InflateCorrupt);
                    }
                    lengths[index] = previous;
                    index += 1;
                }
            }
            17 | 18 => {
                let repeat = if symbol == 17 {
                    3 + reader.bits(3)? as usize
                } else {
                    11 + reader.bits(7)? as usize
                };
                for _ in 0..repeat {
                    if index >= lengths.len() {
                        return Err(PixelError::InflateCorrupt);
                    }
                    lengths[index] = 0;
                    index += 1;
                }
            }
            _ => return Err(PixelError::InflateCorrupt),
        }
    }
    let literal = huffman(&lengths[..literal_count])?;
    let distance = huffman(&lengths[literal_count..])?;
    codes(reader, output, cap, &literal, &distance)
}
