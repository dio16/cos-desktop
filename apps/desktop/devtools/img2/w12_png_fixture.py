"""Deterministic bounded matched-raster PNG source for W12.

The raster is the same fixed block-coordinate pattern the W11 DC-only grayscale
JPEG declares: block (bx, by) holds 48 + 20 * ((3*bx + 5*by + (bx ^ by)) % 9)
for every pixel whose x//8 == bx and y//8 == by.  Rows are streamed into one
bounded per-chunk deflate run, so no full-frame RGBA buffer and no whole-image
raster is ever held.  The encoded container is emitted as signature, IHDR,
IDAT(s), IEND with CRC32 per chunk; zlib level 6 is fixed so regeneration is
byte-identical.
"""

from __future__ import annotations

import hashlib
import struct
from pathlib import Path
import zlib

MAX_ENCODED = 16 * 1024 * 1024
COMPRESSION_LEVEL = 6
_FLUSH_BYTES = 1 << 20
_SIGNATURE = b"\x89PNG\r\n\x1a\n"

FIXTURES = (
    {"id": "png_gray_matched_4k", "width": 4096, "height": 4096, "codec": "png"},
)
PNG_ID = FIXTURES[0]["id"]
_BY_ID = {item["id"]: item for item in FIXTURES}
_METRICS: dict[str, dict] = {}
_FIXTURE_BYTES: dict[str, bytes] = {}


def _level(bx: int, by: int) -> int:
    return 48 + 20 * ((3 * bx + 5 * by + (bx ^ by)) % 9)


def _row(by: int, width: int) -> bytes:
    """One 8-bit gray row; only the 512 block values are recomputed per band."""
    row = bytearray(width)
    for bx in range(width // 8):
        row[8 * bx:8 * bx + 8] = bytes((_level(bx, by),)) * 8
    return bytes(row)


def _raster_rows(width: int, height: int):
    """Yield gray rows top-down; the eight rows of one block band share bytes."""
    if width % 8 or height % 8:
        raise ValueError("geometry_not_block_aligned")
    for by in range(height // 8):
        row = _row(by, width)
        for _ in range(8):
            yield row


def _chunk(tag: bytes, body: bytes) -> bytes:
    return (struct.pack(">I", len(body)) + tag + body
            + struct.pack(">I", zlib.crc32(tag + body) & 0xFFFFFFFF))


def _chunks(fixture_id: str):
    fixture = _BY_ID.get(fixture_id)
    if fixture is None:
        raise ValueError("unknown_fixture")
    width, height = fixture["width"], fixture["height"]
    yield _SIGNATURE
    yield _chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 0, 0, 0, 0))
    compressor = zlib.compressobj(COMPRESSION_LEVEL)
    pending = bytearray()
    for row in _raster_rows(width, height):
        # Filter type 0 keeps every row an exact copy of the declared pixels.
        pending.append(0)
        pending.extend(row)
        if len(pending) >= _FLUSH_BYTES:
            data = compressor.compress(bytes(pending))
            pending.clear()
            if data:
                yield _chunk(b"IDAT", data)
    if pending:
        data = compressor.compress(bytes(pending))
        pending.clear()
        if data:
            yield _chunk(b"IDAT", data)
    tail = compressor.flush()
    if tail:
        yield _chunk(b"IDAT", tail)
    yield _chunk(b"IEND", b"")


def fixture_bytes(fixture_id: str = "png_gray_matched_4k") -> bytes:
    """Regenerate one deterministic PNG, retaining only its bounded encoded bytes."""
    cached = _FIXTURE_BYTES.get(fixture_id)
    if cached is not None:
        return cached
    output = bytearray()
    for chunk in _chunks(fixture_id):
        if len(output) + len(chunk) > MAX_ENCODED:
            raise ValueError("encoded_over_bound")
        output.extend(chunk)
    data = bytes(output)
    _FIXTURE_BYTES[fixture_id] = data
    return data


def write_png(path, fixture_id: str = "png_gray_matched_4k") -> dict:
    """Write exclusively to path while hashing; an existing destination is untouched."""
    if fixture_id not in _BY_ID:
        raise ValueError("unknown_fixture")
    digest = hashlib.sha256()
    size = 0
    with Path(path).open("xb") as stream:
        for chunk in _chunks(fixture_id):
            size += len(chunk)
            if size > MAX_ENCODED:
                raise ValueError("encoded_over_bound")
            stream.write(chunk)
            digest.update(chunk)
    hexdigest = digest.hexdigest()
    measured = {"bytes": size, "sha256": "sha256:" + hexdigest}
    expected = _METRICS.get(fixture_id)
    if expected is not None and measured != expected:
        raise ValueError("fixture_regeneration_mismatch")
    _METRICS[fixture_id] = measured
    return {"case_id": fixture_id, "bytes": size, "sha256": hexdigest}


def measure(fixture_id: str = "png_gray_matched_4k") -> dict:
    if fixture_id not in _BY_ID:
        raise ValueError("unknown_fixture")
    record = _METRICS.get(fixture_id)
    if record is None:
        data = fixture_bytes(fixture_id)
        record = {"bytes": len(data), "sha256": "sha256:" + hashlib.sha256(data).hexdigest()}
        _METRICS[fixture_id] = record
    return dict(record)
