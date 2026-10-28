"""Matched-raster JPEG/PNG equality for W12.

The expectation is the W11 JPEG oracle's own block-level formula source; this
module does not restate a second variant of it.  The JPEG is validated as a
complete DC-only grayscale scan whose decoded DC values match that formula, and
the PNG is streamed row by row so no full-frame raster is retained on either
side.
"""

from __future__ import annotations

import hashlib

from w11_jpeg_oracle import _decode_blocks, _level, _parse
from w12_png_oracle import HEIGHT, WIDTH, png_geometry, stream_png_rows

MAX_ENCODED = 16 * 1024 * 1024


def _need(condition: bool, reason: str) -> None:
    if not condition:
        raise ValueError(reason)


def _expected_row(by: int) -> bytes:
    """Row of the fixed formula, assembled only from the reused W11 level source."""
    row = bytearray(WIDTH)
    for bx in range(WIDTH // 8):
        row[8 * bx:8 * bx + 8] = bytes((_level(bx, by),)) * 8
    return bytes(row)


def compare_jpeg_png_equality(jpeg_bytes: bytes, png_bytes: bytes) -> dict:
    """Check that every PNG pixel equals the JPEG's declared DC block pattern."""
    _need(isinstance(jpeg_bytes, (bytes, bytearray)) and 4 <= len(jpeg_bytes) <= MAX_ENCODED,
          "jpeg_input_bound")
    geometry, entropy = _parse(bytes(jpeg_bytes))
    _decode_blocks(geometry, entropy, expected_pattern=True)
    png_header = png_geometry(png_bytes)
    _need((png_header["width"], png_header["height"]) == (geometry["width"], geometry["height"]),
          "matched_geometry_mismatch")
    _need((png_header["width"], png_header["height"]) == (WIDTH, HEIGHT),
          "matched_geometry_unsupported")

    expected_hash = hashlib.sha256()
    actual_hash = hashlib.sha256()
    compared = 0
    first_mismatch = None
    band = -1
    expected_row = b""
    for y, row in enumerate(stream_png_rows(png_bytes)):
        _need(y < HEIGHT, "png_row_count_invalid")
        if y // 8 != band:
            band = y // 8
            expected_row = _expected_row(band)
        expected_hash.update(expected_row)
        actual_hash.update(row)
        if first_mismatch is None and row != expected_row:
            x = next(index for index in range(len(row)) if row[index] != expected_row[index])
            first_mismatch = {"x": x, "y": y, "expected": expected_row[x], "actual": row[x]}
        compared += len(row)
    _need(compared == WIDTH * HEIGHT, "png_pixel_count_invalid")
    return {"ok": first_mismatch is None, "compared_pixels": compared,
            "first_mismatch": first_mismatch,
            "expected_sha256": "sha256:" + expected_hash.hexdigest(),
            "actual_sha256": "sha256:" + actual_hash.hexdigest(),
            "width": WIDTH, "height": HEIGHT,
            "jpeg_bytes": len(jpeg_bytes),
            "jpeg_sha256": "sha256:" + hashlib.sha256(bytes(jpeg_bytes)).hexdigest(),
            "png_sha256": "sha256:" + hashlib.sha256(png_bytes).hexdigest()}
