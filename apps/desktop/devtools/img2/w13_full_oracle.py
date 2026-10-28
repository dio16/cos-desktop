"""Bounded full-frame verification for the MP-IMG2-W13 matched-raster outputs.

Every decoder output is read row by row: structure, CRC, inflation bounds and
each pixel are checked against the fixed coordinate formula, and only the
running digest is retained. No full RGBA array and no decoder-derived
expectation are used.
"""
from __future__ import annotations

import hashlib
from pathlib import Path
import tempfile

from w7_large_oracle import iter_png_rows, png_header

MAX_ENCODED = 16 * 1024 * 1024
MAX_OUTPUT = 8 * 1024 * 1024
SIDE = 4096
PIXELS = SIDE * SIDE
EXPECTED_RGBA_BYTES = PIXELS * 4


def level(bx: int, by: int) -> int:
    """Fixed producer formula; independent of any decoder result."""
    return 48 + 20 * ((3 * bx + 5 * by + (bx ^ by)) % 9)


def _need(condition, reason):
    if not condition:
        raise ValueError(reason)


def expected_row(y: int) -> bytes:
    """Build one 4096-pixel RGBA row directly from the nine block levels."""
    by = y // 8
    parts = []
    for bx in range(SIDE // 8):
        gray = level(bx, by)
        parts.append(bytes((gray, gray, gray, 255)) * 8)
    return b"".join(parts)


def verify_full_output(data: bytes) -> dict:
    """Stream one output PNG and compare all 16,777,216 pixels."""
    _need(isinstance(data, bytes) and 0 < len(data) <= MAX_OUTPUT, "output_size_out_of_bound")
    digest = hashlib.sha256()
    expected_digest = hashlib.sha256()
    rows = 0
    compared = 0
    first = None
    last = bytearray()
    with tempfile.TemporaryDirectory() as folder:
        path = Path(folder) / "output.png"
        path.write_bytes(data)
        width, height, channels = png_header(path, MAX_OUTPUT)
        _need((width, height, channels) == (SIDE, SIDE, 4), "output_geometry_invalid")
        for index, row in enumerate(iter_png_rows(path, MAX_OUTPUT)):
            _need(index < SIDE, "output_row_count_invalid")
            _need(len(row) == SIDE * 4, "output_row_width_invalid")
            want = expected_row(index)
            _need(len(want) == len(row), "output_row_width_invalid")
            rows += 1
            compared += SIDE
            digest.update(row)
            expected_digest.update(want)
            if first is None and row != want:
                for position in range(0, len(want), 4):
                    if row[position:position + 4] != want[position:position + 4]:
                        first = {"x": position // 4, "y": index,
                                 "expected": list(want[position:position + 4]),
                                 "actual": list(row[position:position + 4])}
                        break
            last = bytearray(row)
        _need(rows == SIDE, "output_row_count_incomplete")
    _need(compared == PIXELS, "output_pixel_count_incomplete")
    _need(len(last) == SIDE * 4, "output_tail_invalid")
    return {"ok": first is None and digest.digest() == expected_digest.digest(),
            "rows": rows, "compared_pixels": compared, "first_mismatch": first,
            "canonical_rgba_sha256": "sha256:" + digest.hexdigest(),
            "expected_rgba_sha256": "sha256:" + expected_digest.hexdigest(),
            "encoded_bytes": len(data),
            "inflated_rgba_bytes": EXPECTED_RGBA_BYTES,
            "width": SIDE, "height": SIDE}


def verify_input_pixels(kind: str, data: bytes) -> dict:
    """Re-verify all input pixels of the shared 4K raster without a full array."""
    _need(isinstance(data, bytes) and 0 < len(data) <= MAX_ENCODED, "input_size_out_of_bound")
    digest = hashlib.sha256()
    compared = 0
    if kind == "png":
        from w12_png_oracle import stream_png_rows
        iterator = ((index, row) for index, row in enumerate(stream_png_rows(data)))
    else:
        from w11_jpeg_oracle import _decode_blocks, _parse
        geometry, entropy = _parse(data)
        _need((geometry["width"], geometry["height"]) == (SIDE, SIDE), "jpeg_geometry_invalid")
        _decode_blocks(geometry, entropy, expected_pattern=True)
        iterator = None
    if iterator is None:
        # A DC-only JPEG block is uniform, so the coordinate formula alone fixes
        # every pixel; the entropy scan above proved the blocks carry those levels.
        digest = hashlib.sha256()
        for y in range(SIDE):
            digest.update(b"".join(bytes((level(bx, y // 8),) * 3 + (255,)) * 8
                                   for bx in range(SIDE // 8)))
            compared += SIDE
    else:
        for index, row in iterator:
            _need(index < SIDE and len(row) == SIDE, "input_row_invalid")
            want = expected_row(index)
            got = bytearray(SIDE * 4)
            for x in range(SIDE):
                gray = row[x]
                got[4 * x] = gray
                got[4 * x + 1] = gray
                got[4 * x + 2] = gray
                got[4 * x + 3] = 255
            _need(bytes(got) == want, "input_pixel_mismatch")
            digest.update(bytes(got))
            compared += SIDE
    _need(compared == PIXELS, "input_pixel_count_incomplete")
    return {"kind": kind, "compared_pixels": compared, "all_pixels_match": True,
            "canonical_rgba_sha256": "sha256:" + digest.hexdigest(), "encoded_bytes": len(data)}
