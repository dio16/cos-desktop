//! Deterministic, explicitly local generated PNG used by native image experiments only.
use desktop_gpui::image_upload::{FRAME_EDGE, UploadRequest};
use media::{
    Cancellation, DecodeBudget, DecodePolicy, LocalId, LocalProvenance, MediaError, Orientation,
    PixelSize, RepresentationKey,
};
pub const MAX_FIXTURE_ENCODED_BYTES: usize = 16 * 1024 * 1024;
pub fn generate_fixture_png(revision: u64, cancel: &Cancellation) -> Result<Vec<u8>, MediaError> {
    let row_len = (FRAME_EDGE as usize)
        .checked_mul(3)
        .and_then(|bytes| bytes.checked_add(1))
        .ok_or(MediaError::TooLarge)?;
    let raw_len = row_len
        .checked_mul(FRAME_EDGE as usize)
        .ok_or(MediaError::TooLarge)?;
    let mut raw = Vec::new();
    raw.try_reserve_exact(raw_len)
        .map_err(|_| MediaError::Allocation)?;
    let revision_color = if revision % 2 == 1 {
        [255, 0, 255]
    } else {
        [0, 255, 255]
    };
    for y in 0..FRAME_EDGE {
        if cancel.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        raw.push(0); // PNG filter None.
        for x in 0..FRAME_EDGE {
            let rgb = if (128..256).contains(&x) && (128..256).contains(&y) {
                revision_color
            } else if (1016..1032).contains(&x) || (1016..1032).contains(&y) {
                [0, 0, 0]
            } else if x < FRAME_EDGE / 2 && y < FRAME_EDGE / 2 {
                [255, 0, 0]
            } else if x >= FRAME_EDGE / 2 && y < FRAME_EDGE / 2 {
                [0, 255, 0]
            } else if x < FRAME_EDGE / 2 {
                [0, 0, 255]
            } else {
                [255, 255, 0]
            };
            raw.extend_from_slice(&rgb);
        }
    }
    if cancel.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    let blocks = raw.len().div_ceil(u16::MAX as usize);
    let deflate_len = 2usize
        .checked_add(raw.len())
        .and_then(|len| len.checked_add(blocks.checked_mul(5)?))
        .and_then(|len| len.checked_add(4))
        .ok_or(MediaError::TooLarge)?;
    let mut idat = Vec::new();
    idat.try_reserve_exact(deflate_len)
        .map_err(|_| MediaError::Allocation)?;
    idat.extend_from_slice(&[0x78, 0x01]); // zlib, fastest/no-compression DEFLATE blocks.
    let block_count = raw.len().div_ceil(u16::MAX as usize);
    for (index, block) in raw.chunks(u16::MAX as usize).enumerate() {
        if cancel.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        idat.push(u8::from(index + 1 == block_count)); // BFINAL, BTYPE=00, byte aligned.
        let len = block.len() as u16;
        idat.extend_from_slice(&len.to_le_bytes());
        idat.extend_from_slice(&(!len).to_le_bytes());
        idat.extend_from_slice(block);
    }
    idat.extend_from_slice(&adler32(&raw, cancel)?.to_be_bytes());
    drop(raw);

    let mut png = Vec::new();
    png.try_reserve_exact(idat.len() + 64)
        .map_err(|_| MediaError::Allocation)?;
    png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = [0u8; 13];
    ihdr[0..4].copy_from_slice(&FRAME_EDGE.to_be_bytes());
    ihdr[4..8].copy_from_slice(&FRAME_EDGE.to_be_bytes());
    ihdr[8] = 8; // bit depth
    ihdr[9] = 2; // truecolour RGB, no alpha
    append_png_chunk(&mut png, *b"IHDR", &ihdr, cancel)?;
    append_png_chunk(&mut png, *b"IDAT", &idat, cancel)?;
    append_png_chunk(&mut png, *b"IEND", &[], cancel)?;
    if png.len() > MAX_FIXTURE_ENCODED_BYTES {
        return Err(MediaError::TooLarge);
    }
    Ok(png)
}

fn adler32(bytes: &[u8], cancel: &Cancellation) -> Result<u32, MediaError> {
    const MODULUS: u32 = 65_521;
    let (mut a, mut b) = (1u32, 0u32);
    for block in bytes.chunks(5_552) {
        if cancel.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        for byte in block {
            a += u32::from(*byte);
            b += a;
        }
        a %= MODULUS;
        b %= MODULUS;
    }
    Ok((b << 16) | a)
}

fn append_png_chunk(
    png: &mut Vec<u8>,
    chunk_type: [u8; 4],
    data: &[u8],
    cancel: &Cancellation,
) -> Result<(), MediaError> {
    let len = u32::try_from(data.len()).map_err(|_| MediaError::TooLarge)?;
    png.extend_from_slice(&len.to_be_bytes());
    png.extend_from_slice(&chunk_type);
    png.extend_from_slice(data);
    let mut crc = !0u32;
    for byte in chunk_type.iter().chain(data.iter()) {
        if cancel.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        crc = (crc >> 8) ^ crc32_table()[((crc as u8) ^ *byte) as usize];
    }
    png.extend_from_slice(&(!crc).to_be_bytes());
    Ok(())
}

fn crc32_table() -> &'static [u32; 256] {
    static TABLE: std::sync::LazyLock<[u32; 256]> = std::sync::LazyLock::new(|| {
        std::array::from_fn(|index| {
            let mut value = index as u32;
            for _ in 0..8 {
                value = if value & 1 == 1 {
                    (value >> 1) ^ 0xedb8_8320
                } else {
                    value >> 1
                };
            }
            value
        })
    });
    &TABLE
}
/// One request for a named fixture source: the manifest uses distinct source ids so each entry is
/// a distinct identity, and a repeated (source, revision) pair is a genuinely shared frame.
pub fn fixture_request_for(
    source: &str,
    revision: u64,
    generation: u64,
) -> Result<UploadRequest, MediaError> {
    let key = RepresentationKey {
        source: media::RepresentationSource::Local {
            provenance: LocalProvenance::Fixture {
                fixture: LocalId::new("img3-s2")?,
                manifest: LocalId::new("generated-v1")?,
                source: LocalId::new(source)?,
            },
            revision,
        },
        kind: desktop_protocol::RepresentationKind::Preview,
        size: PixelSize::new(FRAME_EDGE, FRAME_EDGE)?,
        orientation: Orientation::Normal,
        policy: DecodePolicy {
            decoder: LocalId::new("media-image-png-rgb8")?,
            colour: LocalId::new("srgb-bytes-no-transform")?,
            generation: 1,
        },
    };
    Ok(UploadRequest {
        key,
        source_revision: revision,
        device_generation: generation,
        current_device_generation: generation,
        decode_limits: DecodeBudget::ratified_default().limits()?,
        max_encoded_bytes: MAX_FIXTURE_ENCODED_BYTES,
    })
}
