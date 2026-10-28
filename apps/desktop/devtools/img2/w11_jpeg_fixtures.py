"""Deterministic bounded DC-only grayscale JPEG sources for W11."""
from __future__ import annotations

import hashlib
import struct
from pathlib import Path

MAX_ENCODED = 16 * 1024 * 1024
_QUANT = 8
_DC_SYMBOLS = tuple(range(12))
_AC_SYMBOLS = (0,) + tuple((run << 4) | size for run in range(16) for size in range(1, 11)) + (0xF0,)

FIXTURES = (
    {"id": "jpeg_gray_4k", "width": 4096, "height": 4096, "codec": "jpeg"},
    {"id": "jpeg_gray_16k", "width": 16384, "height": 16384, "codec": "jpeg"},
)
_BY_ID = {item["id"]: item for item in FIXTURES}
_METRICS: dict[str, dict] = {}
_FIXTURE_BYTES: dict[str, bytes] = {}


def _chunk(marker: int, body: bytes) -> bytes:
    return b"\xff" + bytes((marker,)) + struct.pack(">H", len(body) + 2) + body


def _header(width: int, height: int) -> bytes:
    dqt = _chunk(0xDB, b"\x00" + bytes((_QUANT,)) * 64)
    sof = _chunk(0xC0, bytes((8,)) + struct.pack(">HH", height, width) + b"\x01\x01\x11\x00")
    dc = _chunk(0xC4, b"\x00" + bytes((0, 0, 0, 12)) + bytes(12) + bytes(_DC_SYMBOLS))
    ac_counts = bytes((0, 0, 0, 0, 0, 0, 0, len(_AC_SYMBOLS))) + bytes(8)
    ac = _chunk(0xC4, b"\x10" + ac_counts + bytes(_AC_SYMBOLS))
    sos = _chunk(0xDA, b"\x01\x01\x00\x00\x3f\x00")
    return b"\xff\xd8" + dqt + sof + dc + ac + sos


def _level(bx: int, by: int) -> int:
    return 48 + 20 * ((3 * bx + 5 * by + (bx ^ by)) % 9)


def _entropy_chunks(width: int, height: int):
    """Emit stuffed, padded entropy in bounded chunks; only one byte buffer is live."""
    accumulator = 0
    bit_count = 0
    chunk = bytearray()

    def packed_byte(value: int):
        chunk.append(value)
        if value == 0xFF:
            chunk.append(0)

    blocks_w = width // 8
    blocks_h = height // 8
    previous = 0
    for by in range(blocks_h):
        for bx in range(blocks_w):
            dc = _level(bx, by) - 128
            difference = dc - previous
            previous = dc
            category = abs(difference).bit_length()
            encoded = difference if difference >= 0 else difference + (1 << category) - 1
            # Flat canonical DC table has category as its four-bit code; EOB is 0x00/8.
            bits = (category << (category + 8)) | (encoded << 8)
            count = category + 12
            accumulator = (accumulator << count) | bits
            bit_count += count
            while bit_count >= 8:
                bit_count -= 8
                packed_byte((accumulator >> bit_count) & 0xFF)
                accumulator &= (1 << bit_count) - 1 if bit_count else 0
            if len(chunk) >= 65536:
                yield bytes(chunk)
                chunk.clear()
    if bit_count:
        pad = 8 - bit_count
        packed_byte((accumulator << pad) | ((1 << pad) - 1))
    if chunk:
        yield bytes(chunk)


def _chunks(fixture_id: str):
    fixture = _BY_ID.get(fixture_id)
    if fixture is None:
        raise ValueError("unknown_fixture")
    yield _header(fixture["width"], fixture["height"])
    yield from _entropy_chunks(fixture["width"], fixture["height"])
    yield b"\xff\xd9"


def fixture_bytes(fixture_id: str) -> bytes:
    """Regenerate one deterministic JPEG, retaining only its bounded encoded bytes."""
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


def write_fixture(fixture_id: str, path) -> dict:
    """Write exclusively to path while hashing; an existing destination is untouched."""
    fixture = _BY_ID.get(fixture_id)
    if fixture is None:
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


def measure(fixture_id: str) -> dict:
    if fixture_id not in _BY_ID:
        raise ValueError("unknown_fixture")
    record = _METRICS.get(fixture_id)
    if record is None:
        data = fixture_bytes(fixture_id)
        record = {"bytes": len(data), "sha256": "sha256:" + hashlib.sha256(data).hexdigest()}
        _METRICS[fixture_id] = record
    return dict(record)
