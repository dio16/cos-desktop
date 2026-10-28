"""Independent bounded parser and ROI pixel oracle for W11 grayscale JPEGs."""
from __future__ import annotations

import hashlib
import struct
import zlib

from w2_independent_oracle import png_pixels

MAX_ENCODED = 16 * 1024 * 1024
MAX_ROI_PIXELS = 256 * 256
_EXPECTED_DC_COUNTS = bytes((0, 0, 0, 12)) + bytes(12)
_EXPECTED_DC_SYMBOLS = bytes(range(12))
_EXPECTED_AC_SYMBOLS = bytes((0,)) + bytes((run << 4) | size for run in range(16) for size in range(1, 11)) + bytes((0xF0,))
_EXPECTED_AC_COUNTS = bytes((0, 0, 0, 0, 0, 0, 0, len(_EXPECTED_AC_SYMBOLS))) + bytes(8)
_SOURCE_CACHE: dict[str, tuple[bytes, dict]] = {}
_VERIFIED_CACHE: dict[str, tuple[bytes, dict]] = {}


def _cache_put(cache: dict, key: str, value: tuple[bytes, dict]) -> None:
    cache.pop(key, None)
    cache[key] = value
    if len(cache) > 2:
        del cache[next(iter(cache))]


def _need(condition: bool, reason: str) -> None:
    if not condition:
        raise ValueError(reason)


def _level(bx: int, by: int) -> int:
    # Kept independent from the producer's block iterator and encoded DC values.
    return 48 + 20 * ((3 * bx + 5 * by + (bx ^ by)) % 9)


class _Bits:
    def __init__(self, data: bytes):
        self.data = data
        self.index = 0
        self.bit_offset = 0
        self.byte = 0

    def bit(self) -> int:
        if self.bit_offset == 0:
            _need(self.index < len(self.data), "jpeg_entropy_truncated")
            value = self.data[self.index]
            self.index += 1
            if value == 0xFF:
                _need(self.index < len(self.data) and self.data[self.index] == 0,
                      "jpeg_entropy_stuffing_invalid")
                self.index += 1
            self.byte = value
            self.bit_offset = 8
        self.bit_offset -= 1
        return (self.byte >> self.bit_offset) & 1

    def read(self, count: int) -> int:
        value = 0
        for _ in range(count):
            value = (value << 1) | self.bit()
        return value

    def finish(self) -> None:
        if self.bit_offset:
            mask = (1 << self.bit_offset) - 1
            _need((self.byte & mask) == mask, "jpeg_entropy_padding_invalid")
            self.bit_offset = 0
        _need(self.index == len(self.data), "jpeg_entropy_extra_data")


def _segment(data: bytes, pos: int, marker: int):
    _need(pos + 4 <= len(data) and data[pos:pos + 2] == b"\xff" + bytes((marker,)), "jpeg_marker_sequence_invalid")
    length = int.from_bytes(data[pos + 2:pos + 4], "big")
    _need(length >= 2 and pos + 2 + length <= len(data), "jpeg_segment_bounds")
    return data[pos + 4:pos + 2 + length], pos + 2 + length


def _parse(data: bytes) -> tuple[dict, bytes]:
    _need(isinstance(data, bytes) and 4 <= len(data) <= MAX_ENCODED, "jpeg_input_bound")
    _need(data[:2] == b"\xff\xd8", "jpeg_soi_missing")
    dqt, pos = _segment(data, 2, 0xDB)
    _need(dqt == b"\x00" + bytes((8,)) * 64, "jpeg_quant_table_invalid")
    sof, pos = _segment(data, pos, 0xC0)
    _need(len(sof) == 9 and sof[0] == 8 and sof[5] == 1 and sof[6:] == b"\x01\x11\x00", "jpeg_sof0_grayscale_invalid")
    height, width = struct.unpack_from(">HH", sof, 1)
    _need(width > 0 and height > 0 and width % 8 == 0 and height % 8 == 0, "jpeg_geometry_invalid")
    dc, pos = _segment(data, pos, 0xC4)
    _need(dc == b"\x00" + _EXPECTED_DC_COUNTS + _EXPECTED_DC_SYMBOLS, "jpeg_dc_table_invalid")
    ac, pos = _segment(data, pos, 0xC4)
    _need(ac == b"\x10" + _EXPECTED_AC_COUNTS + _EXPECTED_AC_SYMBOLS, "jpeg_ac_table_invalid")
    sos, pos = _segment(data, pos, 0xDA)
    _need(sos == b"\x01\x01\x00\x00\x3f\x00", "jpeg_sos_invalid")
    # Find the first unstuffed marker; this stream permits no restart markers.
    end = pos
    while end + 1 < len(data):
        if data[end] == 0xFF:
            if data[end + 1] == 0:
                end += 2
                continue
            break
        end += 1
    _need(end + 2 == len(data) and data[end:end + 2] == b"\xff\xd9", "jpeg_eoi_or_trailing_invalid")
    entropy = data[pos:end]
    geometry = {"width": width, "height": height, "bits_per_channel": 8,
                "bits_per_pixel": 8, "decoder": "JpegBitmapDecoder", "pixel_format": "Gray8",
                "sof_marker": "SOF0", "component_count": 1, "quantization_step": 8,
                "blocks": (width // 8) * (height // 8), "entropy_bytes": len(entropy)}
    return geometry, entropy


def _decode_blocks(geometry: dict, entropy: bytes, expected_pattern: bool) -> None:
    bits = _Bits(entropy)
    width_blocks, height_blocks = geometry["width"] // 8, geometry["height"] // 8
    predictor = 0
    for by in range(height_blocks):
        for bx in range(width_blocks):
            category = bits.read(4)
            _need(category <= 11, "jpeg_dc_category_invalid")
            coded = bits.read(category) if category else 0
            difference = coded if category == 0 or coded >= (1 << (category - 1)) else coded - (1 << category) + 1
            predictor += difference
            _need(bits.read(8) == 0, "jpeg_ac_not_dc_only")
            if expected_pattern:
                _need(predictor == _level(bx, by) - 128, "jpeg_block_pattern_invalid")
    bits.finish()


def source_geometry(data: bytes) -> dict:
    """Validate framing, fixed tables and the complete DC-only scan, without rasterizing."""
    _need(isinstance(data, bytes) and 4 <= len(data) <= MAX_ENCODED, "jpeg_input_bound")
    digest = hashlib.sha256(data).hexdigest()
    cached = _SOURCE_CACHE.get(digest)
    if cached is not None and cached[0] == data:
        return dict(cached[1])
    geometry, entropy = _parse(data)
    _decode_blocks(geometry, entropy, expected_pattern=False)
    _cache_put(_SOURCE_CACHE, digest, (data, geometry))
    return dict(geometry)


def verify_fixture(fixture_id: str, data: bytes) -> dict:
    from w11_jpeg_fixtures import FIXTURES
    fixture = next((f for f in FIXTURES if f["id"] == fixture_id), None)
    if fixture is None:
        raise ValueError("unknown_fixture")
    _need(isinstance(data, bytes) and 4 <= len(data) <= MAX_ENCODED, "jpeg_input_bound")
    digest = hashlib.sha256(data).hexdigest()
    cached = _VERIFIED_CACHE.get(fixture_id)
    if cached is not None and cached[0] == data:
        return {**cached[1], "geometry": dict(cached[1]["geometry"])}
    geometry, entropy = _parse(data)
    _need((geometry["width"], geometry["height"]) == (fixture["width"], fixture["height"]), "jpeg_fixture_geometry_mismatch")
    _decode_blocks(geometry, entropy, expected_pattern=True)
    result = {"fixture_id": fixture_id, "bytes": len(data), "sha256": "sha256:" + digest,
              "geometry": dict(geometry)}
    _cache_put(_VERIFIED_CACHE, fixture_id, (data, result))
    _cache_put(_SOURCE_CACHE, digest, (data, geometry))
    return {**result, "geometry": dict(geometry)}

def _preflight_png(data: bytes) -> tuple[int, int]:
    """Bound PNG framing and inflation before calling the established pixel decoder."""
    _need(isinstance(data, bytes) and 33 <= len(data) <= 1024 * 1024
          and data[:8] == b"\x89PNG\r\n\x1a\n", "roi_output_png_invalid")
    offset = 8
    header = None
    compressed = bytearray()
    seen_idat = False
    idat_closed = False
    ended = False
    first = True
    while offset < len(data):
        _need(offset + 12 <= len(data), "roi_png_chunk_truncated")
        length = int.from_bytes(data[offset:offset + 4], "big")
        tag = data[offset + 4:offset + 8]
        end = offset + 12 + length
        _need(end <= len(data), "roi_png_chunk_bounds")
        body = data[offset + 8:offset + 8 + length]
        crc = int.from_bytes(data[offset + 8 + length:end], "big")
        _need((zlib.crc32(tag + body) & 0xFFFFFFFF) == crc, "roi_png_crc_invalid")
        _need(not ended, "roi_png_trailing_chunk")
        if first:
            _need(tag == b"IHDR", "roi_png_ihdr_order")
            first = False
        if tag == b"IHDR":
            _need(header is None and length == 13, "roi_png_ihdr_invalid")
            header = struct.unpack(">IIBBBBB", body)
        elif tag == b"IDAT":
            _need(header is not None and not idat_closed, "roi_png_idat_order")
            seen_idat = True
            _need(len(compressed) + length <= 1024 * 1024, "roi_png_compressed_bound")
            compressed.extend(body)
        elif tag == b"IEND":
            _need(seen_idat and length == 0, "roi_png_iend_invalid")
            ended = True
        else:
            if seen_idat:
                idat_closed = True
            _need(tag[0] & 0x20 != 0, "roi_png_unknown_critical_chunk")
        offset = end
    _need(header is not None and seen_idat and ended and offset == len(data), "roi_png_incomplete")
    assert header is not None
    width, height, depth, color, compression, filtering, interlace = header
    _need(0 < width <= 256 and 0 < height <= 256
          and depth == 8 and color in (2, 6)
          and (compression, filtering, interlace) == (0, 0, 0), "roi_png_format_invalid")
    channels = 3 if color == 2 else 4
    expected = (width * channels + 1) * height
    try:
        inflater = zlib.decompressobj()
        raw = inflater.decompress(bytes(compressed), expected + 1)
    except zlib.error as error:
        raise ValueError("roi_png_deflate_invalid") from error
    _need(len(raw) == expected and inflater.eof and not inflater.unused_data
          and not inflater.unconsumed_tail, "roi_png_inflated_size_invalid")
    return width, height


def compare_output(data: bytes, output_png: bytes, roi: list[int]) -> dict:
    """Compare a bounded RGBA PNG ROI against independently derived JPEG block levels."""
    geometry = source_geometry(data)
    _need(isinstance(roi, list) and len(roi) == 4 and all(type(v) is int for v in roi), "roi_invalid")
    x, y, width, height = roi
    _need(x >= 0 and y >= 0 and width > 0 and height > 0 and width <= 256 and height <= 256
          and width * height <= MAX_ROI_PIXELS and x + width <= geometry["width"]
          and y + height <= geometry["height"], "roi_out_of_bounds")
    width_out, height_out = _preflight_png(output_png)
    _need(width_out == roi[2] and height_out == roi[3], "roi_output_geometry_invalid")
    try:
        decoded_width, decoded_height, actual = png_pixels(output_png)
    except (AssertionError, struct.error, ValueError, zlib.error) as error:
        raise ValueError("roi_output_png_invalid") from error
    _need(decoded_width == width_out and decoded_height == height_out
          and len(actual) == width_out * height_out * 4, "roi_output_geometry_invalid")
    _need(decoded_width == width and decoded_height == height
          and len(actual) == width * height * 4, "roi_output_geometry_invalid")
    expected = bytearray()
    for py in range(y, y + height):
        for px in range(x, x + width):
            gray = _level(px // 8, py // 8)
            expected.extend((gray, gray, gray, 255))
    mismatch = None
    for offset in range(0, len(expected), 4):
        if expected[offset:offset + 4] != actual[offset:offset + 4]:
            pixel = offset // 4
            mismatch = {"x": x + pixel % width, "y": y + pixel // width,
                        "expected": list(expected[offset:offset + 4]),
                        "actual": list(actual[offset:offset + 4])}
            break
    return {"ok": mismatch is None, "compared_pixels": width * height, "first_mismatch": mismatch,
            "expected_rgba_sha256": "sha256:" + hashlib.sha256(expected).hexdigest(),
            "actual_rgba_sha256": "sha256:" + hashlib.sha256(actual).hexdigest(), "width": width, "height": height}
