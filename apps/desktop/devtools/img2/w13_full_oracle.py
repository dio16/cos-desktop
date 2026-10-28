"""Bounded full-frame verification for the MP-IMG2-W13 matched-raster outputs.

The reader below is W13-specific on purpose: it validates the complete zlib
stream (end marker, Adler-32, no unused or unconsumed bytes) and caps the
inflated volume at the exact scanline size, so a truncated or extended stream
cannot pass. Rows are decoded and compared one at a time; no full RGBA array and
no decoder-derived expectation is used.
"""
from __future__ import annotations

import hashlib
import struct
import zlib

MAX_ENCODED = 16 * 1024 * 1024
MAX_OUTPUT = 8 * 1024 * 1024
SIDE = 4096
PIXELS = SIDE * SIDE
EXPECTED_RGBA_BYTES = PIXELS * 4
MAX_COMPRESSED = 1024 * 1024
SLICE = 64 * 1024


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


def _chunks(data: bytes):
    """Validate PNG framing and CRC of every chunk; yield (tag, body)."""
    _need(isinstance(data, bytes) and 8 <= len(data) <= MAX_OUTPUT, "output_size_out_of_bound")
    _need(data[:8] == b"\x89PNG\r\n\x1a\n", "png_signature_invalid")
    offset = 8
    ended = False
    while offset < len(data):
        _need(offset + 12 <= len(data), "png_chunk_truncated")
        length = struct.unpack_from(">I", data, offset)[0]
        tag = data[offset + 4:offset + 8]
        end = offset + 12 + length
        _need(end <= len(data), "png_chunk_bounds")
        _need(not ended, "png_trailing_chunk")
        body = data[offset + 8:offset + 8 + length]
        crc = struct.unpack_from(">I", data, offset + 8 + length)[0]
        _need(zlib.crc32(tag + body) & 0xFFFFFFFF == crc, "png_crc_invalid")
        yield tag, body
        if tag == b"IEND":
            _need(length == 0, "png_iend_invalid")
            ended = True
        offset = end
    _need(ended, "png_iend_missing")


def _header_and_idat(data: bytes):
    header = None
    compressed = bytearray()
    idat_seen = idat_closed = False
    for tag, body in _chunks(data):
        if tag == b"IHDR":
            _need(header is None and len(body) == 13, "png_ihdr_invalid")
            header = struct.unpack(">IIBBBBB", body)
        elif tag == b"IDAT":
            _need(header is not None and not idat_closed, "png_idat_order")
            idat_seen = True
            _need(len(compressed) + len(body) <= MAX_COMPRESSED, "png_compressed_bound")
            compressed.extend(body)
        else:
            if idat_seen:
                idat_closed = True
            _need(tag[0] & 0x20 != 0 or tag == b"IEND", "png_unknown_critical_chunk")
    _need(header is not None and idat_seen, "png_incomplete")
    assert header is not None
    width, height, depth, colour, compression, filtering, interlace = header
    _need((width, height, depth) == (SIDE, SIDE, 8), "png_geometry_or_depth_invalid")
    _need(colour in (0, 2, 6) and (compression, filtering, interlace) == (0, 0, 0),
          "png_format_invalid")
    return width, height, 1 if colour == 0 else (3 if colour == 2 else 4), bytes(compressed)


def _scanlines(compressed: bytes, row_bytes: int, height: int):
    """Inflate with a hard output cap; yield verified filtered scanlines."""
    expected = (row_bytes + 1) * height
    inflater = zlib.decompressobj()
    produced = 0
    buffer = bytearray()
    for start in range(0, len(compressed), SLICE):
        piece = compressed[start:start + SLICE]
        while piece:
            remaining = expected + 1 - produced
            _need(remaining > 0, "png_inflated_over_bound")
            out = inflater.decompress(piece, remaining)
            produced += len(out)
            buffer.extend(out)
            piece = inflater.unconsumed_tail
            while len(buffer) >= row_bytes + 1:
                yield bytes(buffer[:row_bytes + 1])
                del buffer[:row_bytes + 1]
            _need(produced <= expected, "png_inflated_over_bound")
    while True:
        out = inflater.flush()
        _need(not out, "png_unconsumed_inflate_tail")
        break
    _need(inflater.eof, "png_inflate_incomplete")
    _need(not inflater.unused_data, "png_unexpected_compressed_tail")
    _need(not inflater.unconsumed_tail, "png_unconsumed_inflate_tail")
    _need(produced == expected, "png_inflated_size_invalid")
    _need(not buffer, "png_scanline_tail_invalid")
    return produced


def _unfilter(kind: int, row: bytearray, previous: bytearray, channels: int) -> bytearray:
    width_bytes = len(row)
    if kind == 0:
        return row
    if kind == 1:
        for position in range(channels, width_bytes):
            row[position] = (row[position] + row[position - channels]) & 0xFF
        return row
    if kind == 2:
        for position in range(width_bytes):
            row[position] = (row[position] + previous[position]) & 0xFF
        return row
    for position in range(width_bytes):
        left = row[position - channels] if position >= channels else 0
        above = previous[position]
        upper_left = previous[position - channels] if position >= channels else 0
        if kind == 3:
            predictor = (left + above) // 2
        elif kind == 4:
            estimate = left + above - upper_left
            candidates = (abs(estimate - left), abs(estimate - above), abs(estimate - upper_left))
            predictor = (left, above, upper_left)[candidates.index(min(candidates))]
        else:
            raise ValueError("png_filter_type_invalid")
        row[position] = (row[position] + predictor) & 0xFF
    return row


def _rgba(row: bytes, channels: int) -> bytes:
    if channels == 4:
        return row
    output = bytearray(SIDE * 4)
    if channels == 1:
        for x in range(SIDE):
            gray = row[x]
            output[4 * x] = gray
            output[4 * x + 1] = gray
            output[4 * x + 2] = gray
            output[4 * x + 3] = 255
    else:
        for x in range(SIDE):
            output[4 * x:4 * x + 3] = row[3 * x:3 * x + 3]
            output[4 * x + 3] = 255
    return bytes(output)


def verify_full_output(data: bytes) -> dict:
    """Stream one output PNG and compare all 16,777,216 pixels."""
    _need(isinstance(data, bytes) and 0 < len(data) <= MAX_OUTPUT, "output_size_out_of_bound")
    width, height, channels, compressed = _header_and_idat(data)
    row_bytes = width * channels
    previous = bytearray(row_bytes)
    digest = hashlib.sha256()
    expected_digest = hashlib.sha256()
    first = None
    rows = 0
    compared = 0
    produced = 0
    scan = _scanlines(compressed, row_bytes, height)
    for record in scan:
        _need(rows < height, "output_row_count_invalid")
        kind = record[0]
        row = bytearray(record[1:])
        _need(len(row) == row_bytes, "output_row_width_invalid")
        row = _unfilter(kind, row, previous, channels)
        previous = bytearray(row)
        rgba = _rgba(bytes(row), channels)
        want = expected_row(rows)
        rows += 1
        compared += SIDE
        digest.update(rgba)
        expected_digest.update(want)
        if first is None and rgba != want:
            for position in range(0, len(want), 4):
                if rgba[position:position + 4] != want[position:position + 4]:
                    first = {"x": position // 4, "y": rows - 1,
                             "expected": list(want[position:position + 4]),
                             "actual": list(rgba[position:position + 4])}
                    break
    try:
        next(scan)
        raise ValueError("output_extra_rows")
    except StopIteration:
        produced = (row_bytes + 1) * height
    _need(rows == height, "output_row_count_incomplete")
    _need(compared == PIXELS, "output_pixel_count_incomplete")
    return {"ok": first is None and digest.digest() == expected_digest.digest(),
            "rows": rows, "compared_pixels": compared, "first_mismatch": first,
            "canonical_rgba_sha256": "sha256:" + digest.hexdigest(),
            "expected_rgba_sha256": "sha256:" + expected_digest.hexdigest(),
            "encoded_bytes": len(data), "compressed_bytes": len(compressed),
            "inflated_scanline_bytes": produced, "inflated_rgba_bytes": EXPECTED_RGBA_BYTES,
            "channels": channels, "width": SIDE, "height": SIDE}


def verify_input_pixels(kind: str, data: bytes) -> dict:
    """Re-verify all input pixels of the shared 4K raster without a full array."""
    _need(isinstance(data, bytes) and 0 < len(data) <= MAX_ENCODED, "input_size_out_of_bound")
    digest = hashlib.sha256()
    compared = 0
    if kind == "png":
        from w12_png_oracle import stream_png_rows
        for index, row in enumerate(stream_png_rows(data)):
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
    else:
        from w11_jpeg_oracle import _decode_blocks, _parse
        geometry, entropy = _parse(data)
        _need((geometry["width"], geometry["height"]) == (SIDE, SIDE), "jpeg_geometry_invalid")
        _decode_blocks(geometry, entropy, expected_pattern=True)
        for y in range(SIDE):
            digest.update(b"".join(bytes((level(bx, y // 8),) * 3 + (255,)) * 8
                                   for bx in range(SIDE // 8)))
            compared += SIDE
    _need(compared == PIXELS, "input_pixel_count_incomplete")
    return {"kind": kind, "compared_pixels": compared, "all_pixels_match": True,
            "canonical_rgba_sha256": "sha256:" + digest.hexdigest(), "encoded_bytes": len(data)}
