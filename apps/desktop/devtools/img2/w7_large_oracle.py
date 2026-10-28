"""Independent streaming raster oracle for MP-IMG2-W7.

Expectations are reconstructed only from the encoded source bytes with the
standard library: PNG CRCs/inflate/filters, BMP24 headers/stride, and real
Bgr565 BI_BITFIELDS masks. WIC output is a separate stream compared row by row,
so no full-frame buffer of either side is retained.
"""

from __future__ import annotations

import hashlib
import struct
from collections.abc import Iterator
from itertools import accumulate
import zlib

PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"
_BMP_HEADER = struct.Struct("<2sIHHI")
_DIB_HEADER = struct.Struct("<IiiHHIIiiII")
_MASKS = (0xF800, 0x07E0, 0x001F)


def _need(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def _add_bytes(left: bytes, right: bytes) -> bytes:
    """Per-byte addition modulo 256 without carrying into the next byte.

    Even and odd byte lanes live in separate 16-bit fields of one big integer,
    so a lane sum cannot overflow into its neighbour and only masked lanes are
    recombined. This keeps PNG Up-filter reconstruction a C-level operation.
    """
    _need(len(left) == len(right), "add_length_mismatch")
    if not left:
        return b""
    even = len(left) // 2 * 2
    body_left = int.from_bytes(left[:even], "little")
    body_right = int.from_bytes(right[:even], "little")
    pattern = int.from_bytes(b"\xff\x00" * (even // 2), "little")
    low = (body_left & pattern) + (body_right & pattern)
    high = ((body_left >> 8) & pattern) + ((body_right >> 8) & pattern)
    body = ((low & pattern) | ((high & pattern) << 8)).to_bytes(even, "little")
    if even == len(left):
        return body
    return body + bytes(((left[-1] + right[-1]) & 0xFF,))


def _png_chunks(path, limit) -> Iterator[tuple[bytes, bytes]]:
    total = 0
    with open(path, "rb") as stream:
        signature = stream.read(8)
        _need(signature == PNG_SIGNATURE, "png_signature_invalid")
        while True:
            header = stream.read(8)
            _need(len(header) == 8, "png_chunk_header_truncated")
            length, tag = struct.unpack(">I", header[:4])[0], header[4:]
            _need(length <= limit, "png_chunk_over_bound")
            body = stream.read(length)
            _need(len(body) == length, "png_chunk_body_truncated")
            crc_bytes = stream.read(4)
            _need(len(crc_bytes) == 4, "png_crc_truncated")
            crc = struct.unpack(">I", crc_bytes)[0]
            _need(zlib.crc32(tag + body) & 0xFFFFFFFF == crc, "png_crc_mismatch")
            total += 12 + length
            _need(total <= limit, "png_total_over_bound")
            yield tag, body
            if tag == b"IEND":
                break
        _need(stream.read(1) == b"", "png_trailing_bytes")


def png_header(path, limit) -> tuple[int, int, int]:
    """Validate the chunk stream and return (width, height, channels, crcs_ok)."""
    header = None
    seen_end = False
    idat_seen = False
    for tag, body in _png_chunks(path, limit):
        if tag == b"IHDR":
            _need(header is None and len(body) == 13, "png_ihdr_invalid")
            width, height, depth, colour, compression, filter_method, interlace = struct.unpack(">IIBBBBB", body)
            _need(0 < width <= 65536 and 0 < height <= 65536, "png_dimensions_out_of_range")
            _need(depth == 8 and colour in (2, 6), "png_depth_or_colour_unsupported")
            _need((compression, filter_method, interlace) == (0, 0, 0), "png_method_unsupported")
            header = (width, height, 3 if colour == 2 else 4)
        elif tag == b"IDAT":
            _need(header is not None, "png_idat_before_ihdr")
            idat_seen = True
        elif tag == b"IEND":
            _need(idat_seen, "png_idat_missing")
            seen_end = True
    _need(header is not None and seen_end, "png_structure_incomplete")
    assert header is not None
    return header


def iter_png_rows(path, limit) -> Iterator[bytes]:
    """Yield RGBA rows decoded from a single non-interlaced 8-bit PNG stream."""
    width, height, channels = png_header(path, limit)
    stride = width * channels
    inflater = zlib.decompressobj()
    prefix = bytearray()
    previous = bytearray(stride)
    produced = 0

    def rows_from(buffer):
        nonlocal produced, previous
        while True:
            need = (stride + 1) - len(buffer)
            if need > 0:
                return
            kind = buffer[0]
            _need(0 <= kind <= 4, "png_filter_type_invalid")
            row = bytearray(buffer[1:1 + stride])
            del buffer[:stride + 1]
            if kind == 1:
                # Sub applies per channel, so each channel is an independent
                # running sum modulo 256; accumulate keeps it a C-level loop.
                for channel in range(channels):
                    row[channel::channels] = bytes(
                        value & 0xFF for value in accumulate(row[channel::channels]))
            elif kind == 2:
                row = bytearray(_add_bytes(bytes(row), bytes(previous)))
            elif kind != 0:
                for x in range(stride):
                    left = row[x - channels] if x >= channels else 0
                    above = previous[x]
                    upper_left = previous[x - channels] if x >= channels else 0
                    if kind == 2:
                        predictor = above
                    elif kind == 3:
                        predictor = (left + above) // 2
                    else:
                        p = left + above - upper_left
                        distances = (abs(p - left), abs(p - above), abs(p - upper_left))
                        predictor = (left, above, upper_left)[distances.index(min(distances))]
                    row[x] = (row[x] + predictor) & 0xFF
            previous = row
            produced += 1
            if channels == 3:
                expanded = bytearray(width * 4)
                for x in range(width):
                    expanded[4 * x:4 * x + 3] = row[3 * x:3 * x + 3]
                    expanded[4 * x + 3] = 0xFF
                yield bytes(expanded)
            else:
                yield bytes(row)

    for tag, body in _png_chunks(path, limit):
        if tag != b"IDAT":
            continue
        prefix.extend(inflater.decompress(body))
        for row in rows_from(prefix):
            yield row
            if produced == height:
                return
    prefix.extend(inflater.flush())
    for row in rows_from(prefix):
        yield row
        if produced == height:
            return
    _need(produced == height, "png_row_count_incomplete")
    _need(inflater.eof and not inflater.unused_data, "png_inflate_incomplete")


def _bmp_fields(path) -> dict:
    with open(path, "rb") as stream:
        header = stream.read(66)
    _need(len(header) >= 66, "bmp_header_truncated")
    signature, file_size, reserved1, reserved2, pixel_offset = _BMP_HEADER.unpack_from(header)
    _need(signature == b"BM", "bmp_signature_invalid")
    _need(reserved1 == 0 and reserved2 == 0, "bmp_reserved_not_zero")
    dib_size, width, signed_height, planes, depth, compression, image_size, _, _, _, _ = _DIB_HEADER.unpack_from(header, 14)
    _need(dib_size == 40, "bmp_dib_header_invalid")
    _need(0 < width <= 65536 and signed_height != 0 and abs(signed_height) <= 65536, "bmp_dimensions_out_of_range")
    _need(planes == 1, "bmp_planes_invalid")
    _need(depth in (16, 24), "bmp_depth_unsupported")
    _need(compression in (0, 3) and not (depth == 24 and compression != 0), "bmp_compression_invalid")
    height = abs(signed_height)
    masks = struct.unpack_from("<III", header, 54) if depth == 16 else None
    if depth == 16:
        _need(masks == _MASKS, "bmp_masks_not_rgb565")
    stride = ((width * depth // 8) + 3) // 4 * 4
    _need(image_size in (0, stride * height), "bmp_image_size_inconsistent")
    _need(file_size == pixel_offset + stride * height, "bmp_file_size_inconsistent")
    return {"width": width, "height": height, "depth": depth, "stride": stride,
            "pixel_offset": pixel_offset, "file_size": file_size,
            "top_down": signed_height < 0, "masks": masks}


def iter_bmp_rows(path) -> Iterator[bytes]:
    fields = _bmp_fields(path)
    width, height, stride = fields["width"], fields["height"], fields["stride"]
    with open(path, "rb") as stream:
        for index in range(height):
            y = index if fields["top_down"] else height - 1 - index
            stream.seek(fields["pixel_offset"] + y * stride)
            row = stream.read(stride)
            _need(len(row) == stride, "bmp_row_truncated")
            if fields["depth"] == 24:
                _need(width * 3 <= stride, "bmp24_row_extent_invalid")
                expanded = bytearray(width * 4)
                for x in range(width):
                    b, g, r = row[3 * x:3 * x + 3]
                    expanded[4 * x] = r
                    expanded[4 * x + 1] = g
                    expanded[4 * x + 2] = b
                    expanded[4 * x + 3] = 0xFF
            else:
                _need(width * 2 <= stride, "bmp565_row_extent_invalid")
                expanded = bytearray(width * 4)
                for x in range(width):
                    value = struct.unpack_from("<H", row, 2 * x)[0]
                    r5 = (value >> 11) & 0x1F
                    g6 = (value >> 5) & 0x3F
                    b5 = value & 0x1F
                    expanded[4 * x] = (r5 << 3) | (r5 >> 2)
                    expanded[4 * x + 1] = (g6 << 2) | (g6 >> 4)
                    expanded[4 * x + 2] = (b5 << 3) | (b5 >> 2)
                    expanded[4 * x + 3] = 0xFF
            yield bytes(expanded)


def source_rows(codec, path, limit) -> Iterator[bytes]:
    if codec == "png":
        return iter_png_rows(path, limit)
    if codec in ("bmp24", "bgr565"):
        return iter_bmp_rows(path)
    raise ValueError("unknown_codec")


def source_geometry(codec, path, limit):
    if codec == "png":
        width, height, channels = png_header(path, limit)
        return {"width": width, "height": height, "channels": channels}
    fields = _bmp_fields(path)
    return {"width": fields["width"], "height": fields["height"],
            "channels": 3 if fields["depth"] == 24 else 4}


def expected_digest(case, path, limit):
    digest = hashlib.sha256()
    pixels = 0
    width = 0
    for row in source_rows(case["codec"], path, limit):
        digest.update(row)
        pixels += len(row) // 4
        width = len(row) // 4
    return {"rgba_sha256": "sha256:" + digest.hexdigest(), "pixels": pixels, "width": width}


def compare(case, source_path, output_path, output_codec, limit):
    """Stream the independent expectation and the reported WIC output together."""
    width = case["width"]
    digest_expected = hashlib.sha256()
    digest_actual = hashlib.sha256()
    pixels = 0
    first = None
    rows = 0
    expected = source_rows(case["codec"], source_path, limit)
    actual = iter_png_rows(output_path, limit) if output_codec == "png" else iter_bmp_rows(output_path)
    while True:
        try:
            want = next(expected)
        except StopIteration:
            want = None
        try:
            got = next(actual)
        except StopIteration:
            got = None
        if want is None or got is None:
            _need(want is None and got is None, "raster_row_count_differs")
            break
        _need(len(want) == len(got) == width * 4, "raster_row_width_differs")
        digest_expected.update(want)
        digest_actual.update(got)
        if first is None and want != got:
            for index in range(0, len(want), 4):
                if want[index:index + 4] != got[index:index + 4]:
                    first = {"row": rows, "column": index // 4,
                             "expected_rgba": list(want[index:index + 4]),
                             "actual_rgba": list(got[index:index + 4])}
                    break
        pixels += width
        rows += 1
    return {"ok": first is None and pixels == width * case["height"],
            "compared_pixels": pixels,
            "compared_rows": rows,
            "expected_rgba_sha256": "sha256:" + digest_expected.hexdigest(),
            "actual_rgba_sha256": "sha256:" + digest_actual.hexdigest(),
            "first_mismatch": first,
            "method": ("independent streaming PNG CRC/inflate/filter, BMP24 header/stride and Bgr565 "
                       "BI_BITFIELDS mask reconstruction compared row by row against the reported output")}
