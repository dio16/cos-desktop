"""Independent bounded PNG parser and pixel oracle for the W12 matched fixture.

Shared with the producer only the fixed block-level constants (base 48, step 20,
modulus 9, coefficients 3 and 5, xor on the block indices).  Every structural
rule below is re-derived here: signature and chunk lengths, CRC32, chunk order,
IHDR fields, bounded exact inflation, filter reconstruction and per-pixel
comparison.  Rows are streamed, so a full-frame RGBA raster is never retained.
"""

from __future__ import annotations

import hashlib
from itertools import accumulate
import struct
import zlib

MAX_ENCODED = 16 * 1024 * 1024
WIDTH = 4096
HEIGHT = 4096
_INFLATED_BYTES = (WIDTH * 1 + 1) * HEIGHT
MAX_ROI_PIXELS = 256 * 256
MAX_ROI_EDGE = 256
PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"


def _need(condition: bool, reason: str) -> None:
    if not condition:
        raise ValueError(reason)


def _level(bx: int, by: int) -> int:
    # Kept independent from the producer's row builder and deflate input.
    return 48 + 20 * ((3 * bx + 5 * by + (bx ^ by)) % 9)


def _expected_gray_row(by: int) -> bytes:
    """Declared gray bytes for every x whose y//8 == by."""
    row = bytearray(WIDTH)
    for bx in range(WIDTH // 8):
        row[8 * bx:8 * bx + 8] = bytes((_level(bx, by),)) * 8
    return bytes(row)


def _chunks(data: bytes, strict: bool):
    """Validate and yield (tag, body) in order; strict allows only structural chunks."""
    _need(isinstance(data, (bytes, bytearray)) and 8 <= len(data) <= MAX_ENCODED, "png_input_bound")
    view = memoryview(data)
    _need(bytes(view[:8]) == PNG_SIGNATURE, "png_signature_invalid")
    offset = 8
    first = True
    seen_header = False
    seen_idat = False
    idat_open = False
    ended = False
    while offset < len(data):
        _need(not ended, "png_trailing_bytes")
        _need(offset + 12 <= len(data), "png_chunk_truncated")
        length = struct.unpack_from(">I", data, offset)[0]
        tag = bytes(view[offset + 4:offset + 8])
        end = offset + 12 + length
        _need(end <= len(data), "png_chunk_bounds")
        body = view[offset + 8:offset + 8 + length]
        crc = struct.unpack_from(">I", data, offset + 8 + length)[0]
        _need((zlib.crc32(bytes(body), zlib.crc32(tag)) & 0xFFFFFFFF) == crc, "png_crc_invalid")
        if first:
            _need(tag == b"IHDR" and length == 13, "png_ihdr_order")
            first = False
        if tag == b"IHDR":
            _need(not seen_header, "png_ihdr_duplicate")
            seen_header = True
        elif tag == b"IDAT":
            _need(seen_header, "png_idat_before_ihdr")
            _need(not (seen_idat and not idat_open), "png_idat_not_contiguous")
            seen_idat = True
            idat_open = True
        elif tag == b"IEND":
            _need(seen_idat and length == 0, "png_iend_invalid")
        else:
            _need(not strict and (tag[0] & 0x20) != 0, "png_unknown_chunk")
            idat_open = False
        offset = end
        yield tag, body
        if tag == b"IEND":
            ended = True
            break
    _need(seen_header, "png_ihdr_missing")
    _need(seen_idat, "png_idat_missing")
    _need(ended, "png_iend_missing")
    _need(offset == len(data), "png_trailing_bytes")


def _unfilter(kind: int, row: bytearray, previous: bytes | bytearray,
              channels: int) -> bytearray:
    if kind == 0:
        return row
    if kind == 1:
        for channel in range(channels):
            row[channel::channels] = bytes(
                value & 0xFF for value in accumulate(row[channel::channels]))
        return row
    if kind == 2:
        for x in range(len(row)):
            row[x] = (row[x] + previous[x]) & 0xFF
        return row
    for x in range(len(row)):
        left = row[x - channels] if x >= channels else 0
        above = previous[x]
        upper_left = previous[x - channels] if x >= channels else 0
        if kind == 3:
            predictor = (left + above) // 2
        else:  # kind == 4, Paeth
            p = left + above - upper_left
            distances = (abs(p - left), abs(p - above), abs(p - upper_left))
            predictor = (left, above, upper_left)[distances.index(min(distances))]
        row[x] = (row[x] + predictor) & 0xFF
    return row


class _PngStream:
    """Validated non-interlaced 8-bit PNG whose unfiltered rows are streamed."""

    def __init__(self, data: bytes, *, strict: bool, colors: tuple[int, ...],
                 max_width: int, max_height: int, geometry: tuple[int, int] | None = None):
        self._chunks = _chunks(data, strict)
        try:
            tag, body = next(self._chunks)
        except StopIteration:
            raise ValueError("png_ihdr_missing") from None
        _need(tag == b"IHDR", "png_ihdr_order")
        width, height, depth, color, compression, filtering, interlace = struct.unpack(
            ">IIBBBBB", bytes(body))
        _need(0 < width <= max_width and 0 < height <= max_height, "png_dimensions_out_of_range")
        _need(depth == 8, "png_depth_unsupported")
        _need(color in colors, "png_colour_type_unsupported")
        _need((compression, filtering, interlace) == (0, 0, 0), "png_method_unsupported")
        if geometry is not None:
            _need((width, height) == tuple(geometry), "png_geometry_mismatch")
        self.width = width
        self.height = height
        self.depth = depth
        self.color = color
        self.channels = {0: 1, 2: 3, 6: 4}[color]
        self.stride = width * self.channels
        self.expected = (self.stride + 1) * height
        self.inflated_bytes = 0

    def rows(self):
        inflater = zlib.decompressobj()
        buffer = bytearray()
        previous = bytes(self.stride)
        produced = 0
        for tag, body in self._chunks:
            if tag != b"IDAT":
                continue
            remaining = self.expected - produced + 1
            try:
                raw = inflater.decompress(body, remaining)
            except zlib.error as error:
                raise ValueError("png_deflate_invalid") from error
            _need(not inflater.unconsumed_tail, "png_inflate_excess")
            produced += len(raw)
            _need(produced <= self.expected, "png_inflate_excess")
            buffer.extend(raw)
            offset = 0
            step = self.stride + 1
            while len(buffer) - offset >= step:
                kind = buffer[offset]
                _need(0 <= kind <= 4, "png_filter_type_invalid")
                row = bytearray(buffer[offset + 1:offset + step])
                offset += step
                row = _unfilter(kind, row, previous, self.channels)
                previous = row
                yield bytes(row)
            if offset:
                del buffer[:offset]
        _need(inflater.eof, "png_inflate_incomplete")
        _need(produced == self.expected, "png_inflate_size_invalid")
        _need(not inflater.unused_data, "png_inflate_trailing_data")
        _need(not buffer, "png_row_tail_invalid")
        self.inflated_bytes = produced


def stream_png_rows(data: bytes):
    """Yield every verified 8-bit gray row of the 4096x4096 matched PNG."""
    stream = _PngStream(data, strict=True, colors=(0,), max_width=WIDTH,
                        max_height=HEIGHT, geometry=(WIDTH, HEIGHT))
    count = 0
    for row in stream.rows():
        count += 1
        _need(count <= HEIGHT, "png_row_count_invalid")
        yield row
    _need(count == HEIGHT, "png_row_count_invalid")


def png_geometry(data: bytes) -> dict:
    """Validate container framing and CRCs, returning IHDR fields without inflation."""
    chunks = _chunks(data, strict=True)
    tag, body = next(chunks)
    _need(tag == b"IHDR", "png_ihdr_order")
    width, height, depth, color, compression, filtering, interlace = struct.unpack(
        ">IIBBBBB", bytes(body))
    _need(0 < width <= WIDTH and 0 < height <= HEIGHT, "png_dimensions_out_of_range")
    _need(depth == 8 and color == 0, "png_format_unsupported")
    _need((compression, filtering, interlace) == (0, 0, 0), "png_method_unsupported")
    for _tag, _body in chunks:
        pass
    return {"width": width, "height": height, "depth": depth, "color_type": color}


def verify_png(data: bytes) -> dict:
    """Fully validate the fixture container and count its streamed rows."""
    stream = _PngStream(data, strict=True, colors=(0,), max_width=WIDTH,
                        max_height=HEIGHT, geometry=(WIDTH, HEIGHT))
    rows = 0
    for _row in stream.rows():
        rows += 1
    _need(rows == HEIGHT and stream.inflated_bytes == _INFLATED_BYTES, "png_row_count_invalid")
    return {"width": stream.width, "height": stream.height, "depth": stream.depth,
            "color_type": stream.color, "bytes": len(data),
            "sha256": "sha256:" + hashlib.sha256(data).hexdigest(),
            "rows": rows, "inflated_bytes": stream.inflated_bytes}


def compare_all_pixels(png_bytes: bytes) -> dict:
    """Compare every PNG pixel with the fixed formula, row by row.

    Digests cover the streamed row-major 8-bit gray raster (no filter bytes, no
    retained full frame).
    """
    stream = _PngStream(png_bytes, strict=True, colors=(0,), max_width=WIDTH,
                        max_height=HEIGHT, geometry=(WIDTH, HEIGHT))
    expected_hash = hashlib.sha256()
    actual_hash = hashlib.sha256()
    compared = 0
    first_mismatch = None
    band = -1
    expected_row = b""
    for y, row in enumerate(stream.rows()):
        _need(y < HEIGHT, "png_row_count_invalid")
        if y // 8 != band:
            band = y // 8
            expected_row = _expected_gray_row(band)
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
            "actual_sha256": "sha256:" + actual_hash.hexdigest()}


def _rgba_rows(stream: "_PngStream"):
    for row in stream.rows():
        if stream.channels == 1:
            expanded = bytearray(len(row) * 4)
            for x, value in enumerate(row):
                expanded[4 * x:4 * x + 3] = bytes((value, value, value))
                expanded[4 * x + 3] = 0xFF
            yield bytes(expanded)
        elif stream.channels == 3:
            expanded = bytearray((len(row) // 3) * 4)
            for x in range(len(row) // 3):
                expanded[4 * x:4 * x + 3] = row[3 * x:3 * x + 3]
                expanded[4 * x + 3] = 0xFF
            yield bytes(expanded)
        else:
            yield row


def compare_roi(png_bytes: bytes, roi: list[int]) -> dict:
    """Compare a decoder ROI PNG against the fixed formula for that rectangle.

    ``roi`` is [x, y, width, height] in 4096x4096 source coordinates; the output
    PNG must be exactly width x height, 8-bit, colour type 0, 2 or 6, and is
    compared as RGBA.  Digests cover the streamed RGBA rectangle.
    """
    _need(isinstance(roi, list) and len(roi) == 4 and all(type(value) is int for value in roi),
          "roi_invalid")
    x, y, width, height = roi
    _need(x >= 0 and y >= 0 and width > 0 and height > 0
          and width <= MAX_ROI_EDGE and height <= MAX_ROI_EDGE
          and width * height <= MAX_ROI_PIXELS
          and x + width <= WIDTH and y + height <= HEIGHT, "roi_out_of_bounds")
    stream = _PngStream(png_bytes, strict=False, colors=(0, 2, 6),
                        max_width=MAX_ROI_EDGE, max_height=MAX_ROI_EDGE,
                        geometry=(width, height))
    expected_hash = hashlib.sha256()
    actual_hash = hashlib.sha256()
    compared = 0
    first_mismatch = None
    seen = 0
    for row_index, actual in enumerate(_rgba_rows(stream)):
        _need(row_index < height, "roi_output_row_count_invalid")
        expected = bytearray(width * 4)
        for px in range(width):
            gray = _level((x + px) // 8, (y + row_index) // 8)
            expected[4 * px:4 * px + 3] = bytes((gray, gray, gray))
            expected[4 * px + 3] = 0xFF
        expected_hash.update(expected)
        actual_hash.update(actual)
        if first_mismatch is None and actual != expected:
            offset = next(index // 4 for index in range(0, len(actual), 4)
                          if actual[index:index + 4] != expected[index:index + 4])
            first_mismatch = {"x": x + offset, "y": y + row_index,
                              "expected": list(expected[4 * offset:4 * offset + 4]),
                              "actual": list(actual[4 * offset:4 * offset + 4])}
        compared += width
        seen += 1
    _need(seen == height, "roi_output_row_count_invalid")
    expected_digest = "sha256:" + expected_hash.hexdigest()
    actual_digest = "sha256:" + actual_hash.hexdigest()
    return {"ok": first_mismatch is None, "compared_pixels": compared,
            "first_mismatch": first_mismatch,
            "expected_sha256": expected_digest, "actual_sha256": actual_digest,
            # WIC output comparison consumes RGBA digests under these names.
            "expected_rgba_sha256": expected_digest, "actual_rgba_sha256": actual_digest,
            "width": width, "height": height}
