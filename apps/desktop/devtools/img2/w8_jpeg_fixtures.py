"""Deterministic W8 JPEG fixtures with no external encoder.

Two producers live here:

* a hand-built DC-only baseline grayscale JPEG whose expected pixels follow from
  the parsed coefficient and quantization tables, and
* a small real baseline encoder (FDCT, quantization, flat canonical Huffman
  tables, byte stuffing) that produces non-uniform 4:4:4 and 4:2:0 sources.

Neither producer is a decoder, so the reference path stays independent.
"""

from __future__ import annotations

import hashlib
import math
import struct

MAX_ENCODED = 16 * 1024 * 1024
_PERIOD = 64
_ZIGZAG = (
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5,
    12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21, 28,
    35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51,
    58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
)
_COS = [[math.cos((2 * x + 1) * u * math.pi / 16.0) for x in range(8)] for u in range(8)]
_SCALE = [1.0 / math.sqrt(2.0)] + [1.0] * 7

# Flat, fully specified tables: DC categories 0..11 at length 4, and the 162 AC
# symbols (EOB, run/size, ZRL) at length 8. They are legal canonical Huffman
# tables, so no encoder-specific table has to be guessed.
_DC_SYMBOLS = list(range(12))
_AC_SYMBOLS = [0x00] + [(run << 4) | size for run in range(16) for size in range(1, 11)] + [0xF0]


def _need(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def _chunk(marker: int, body: bytes) -> bytes:
    return bytes((0xFF, marker)) + struct.pack(">H", len(body) + 2) + body


def _dht(table_class: int, table_id: int, counts: list[int], symbols: list[int]) -> bytes:
    _need(sum(counts) == len(symbols), "huffman_symbol_count_mismatch")
    return _chunk(0xC4, bytes((table_class << 4 | table_id,)) + bytes(counts) + bytes(symbols))


def _flat_dht(table_class: int, table_id: int, symbols: list[int], length: int) -> bytes:
    counts = [0] * 16
    counts[length - 1] = len(symbols)
    _need(len(symbols) <= 1 << length, "flat_table_overflows_code_space")
    return _dht(table_class, table_id, counts, symbols)


class _BitWriter:
    def __init__(self) -> None:
        self.buffer = bytearray()
        self.accumulator = 0
        self.count = 0

    def write(self, value: int, bits: int) -> None:
        for shift in range(bits - 1, -1, -1):
            self.accumulator = (self.accumulator << 1) | ((value >> shift) & 1)
            self.count += 1
            if self.count == 8:
                self.buffer.append(self.accumulator & 0xFF)
                self.accumulator = 0
                self.count = 0

    def finish(self) -> bytes:
        if self.count:
            self.write((1 << (8 - self.count)) - 1, 8 - self.count)
        stuffed = bytearray()
        for byte in self.buffer:
            stuffed.append(byte)
            if byte == 0xFF:
                stuffed.append(0x00)
        return bytes(stuffed)


def _fdct(block: list[list[float]]) -> list[list[float]]:
    temp = [[0.0] * 8 for _ in range(8)]
    for y in range(8):
        for u in range(8):
            total = 0.0
            for x in range(8):
                total += block[y][x] * _COS[u][x]
            temp[y][u] = total * 0.5 * _SCALE[u]
    out = [[0.0] * 8 for _ in range(8)]
    for u in range(8):
        for v in range(8):
            total = 0.0
            for y in range(8):
                total += temp[y][u] * _COS[v][y]
            out[v][u] = total * 0.5 * _SCALE[v]
    return out


def _category(value: int) -> tuple[int, int]:
    if value == 0:
        return 0, 0
    size = abs(value).bit_length()
    bits = value if value > 0 else value + (1 << size) - 1
    return size, bits


_DC_CODES = {symbol: index for index, symbol in enumerate(_DC_SYMBOLS)}
_AC_CODES = {symbol: index for index, symbol in enumerate(_AC_SYMBOLS)}


def _encode_component(writer: _BitWriter, planes: list[list[list[int]]], predictor: int) -> int:
    for value in planes:
        coefficients = _fdct([[float(v) for v in row] for row in value])
        quantized = [[0] * 8 for _ in range(8)]
        for v in range(8):
            for u in range(8):
                quantized[v][u] = int(round(coefficients[v][u] / 16.0))
        dc = quantized[0][0]
        size, bits = _category(dc - predictor)
        code = _DC_CODES[size]
        writer.write(code, 4)
        if size:
            writer.write(bits, size)
        predictor = dc
        run = 0
        for position in range(1, 64):
            v, u = divmod(_ZIGZAG[position], 8)
            value_coefficient = quantized[v][u]
            if value_coefficient == 0:
                run += 1
                continue
            while run > 15:
                writer.write(_AC_CODES[0xF0], 8)
                run -= 16
            size, bits = _category(value_coefficient)
            writer.write(_AC_CODES[(run << 4) | size], 8)
            writer.write(bits, size)
            run = 0
        if run:
            writer.write(_AC_CODES[0x00], 8)
    return predictor


def _mcu_planes(plane: list[list[int]], width: int, height: int, h: int, v: int):
    """Yield (component blocks per MCU) in the encoder's MCU raster order."""
    mcu_w, mcu_h = 8 * h, 8 * v
    for my in range(0, height, mcu_h):
        for mx in range(0, width, mcu_w):
            for j in range(v):
                for i in range(h):
                    block = [[0] * 8 for _ in range(8)]
                    for y in range(8):
                        source_y = min(my + j * 8 + y, height - 1)
                        for x in range(8):
                            source_x = min(mx + i * 8 + x, width - 1)
                            block[y][x] = plane[source_y][source_x]
                    yield block


def _pattern_rgb(width: int, height: int) -> tuple[list[list[int]], list[list[int]], list[list[int]]]:
    r_plane = [[0] * width for _ in range(height)]
    g_plane = [[0] * width for _ in range(height)]
    b_plane = [[0] * width for _ in range(height)]
    for y in range(height):
        for x in range(width):
            r_plane[y][x] = (x % _PERIOD) * 4
            g_plane[y][x] = (y % _PERIOD) * 4
            b_plane[y][x] = 32 + ((((x // _PERIOD) + (y // _PERIOD)) & 1) * 128)
    return r_plane, g_plane, b_plane


def _to_ycbcr(r_plane, g_plane, b_plane, width, height):
    y_plane = [[0] * width for _ in range(height)]
    cb_plane = [[0] * width for _ in range(height)]
    cr_plane = [[0] * width for _ in range(height)]
    for y in range(height):
        for x in range(width):
            r, g, b = r_plane[y][x], g_plane[y][x], b_plane[y][x]
            y_plane[y][x] = max(0, min(255, int(round(0.299 * r + 0.587 * g + 0.114 * b))))
            cb_plane[y][x] = max(0, min(255, int(round(-0.168736 * r - 0.331264 * g + 0.5 * b + 128))))
            cr_plane[y][x] = max(0, min(255, int(round(0.5 * r - 0.418688 * g - 0.081312 * b + 128))))
    return y_plane, cb_plane, cr_plane


def _downsample(plane, width, height):
    out_w, out_h = (width + 1) // 2, (height + 1) // 2
    out = [[0] * out_w for _ in range(out_h)]
    for y in range(out_h):
        for x in range(out_w):
            total = 0
            count = 0
            for dy in range(2):
                for dx in range(2):
                    sy, sx = min(2 * y + dy, height - 1), min(2 * x + dx, width - 1)
                    total += plane[sy][sx]
                    count += 1
            out[y][x] = int(round(total / count))
    return out


def encode_jpeg(width: int, height: int, sampling: str, quant_scale: int) -> bytes:
    """Encode the deterministic pattern as a real baseline JPEG."""
    _need(sampling in ("444", "420"), "unknown_sampling")
    r_plane, g_plane, b_plane = _pattern_rgb(width, height)
    y_plane, cb_plane, cr_plane = _to_ycbcr(r_plane, g_plane, b_plane, width, height)
    if sampling == "420":
        cb_plane = _downsample(cb_plane, width, height)
        cr_plane = _downsample(cr_plane, width, height)
        sampling_factor = 0x22
    else:
        sampling_factor = 0x11
    components = [{"id": 1, "sampling": sampling_factor, "table": 0},
                  {"id": 2, "sampling": 0x11, "table": 0},
                  {"id": 3, "sampling": 0x11, "table": 0}]
    quant = [quant_scale] * 64
    out = bytearray(b"\xff\xd8")
    out += _chunk(0xE0, b"JFIF\x00\x01\x01\x00\x00\x01\x00\x01\x00\x00")
    out += _chunk(0xDB, bytes((0x00,)) + bytes(quant))
    sof = bytes((8,)) + struct.pack(">HH", height, width) + bytes((3,))
    for component in components:
        sof += bytes((component["id"], component["sampling"], component["table"]))
    out += _chunk(0xC0, sof)
    out += _flat_dht(0, 0, _DC_SYMBOLS, 4)
    out += _flat_dht(1, 0, _AC_SYMBOLS, 8)
    sos = bytes((3,)) + bytes((1, 0x00, 2, 0x00, 3, 0x00)) + bytes((0, 63, 0))
    out += _chunk(0xDA, sos)
    writer = _BitWriter()
    if sampling == "420":
        y_blocks = list(_mcu_planes(y_plane, width, height, 2, 2))
        cb_blocks = list(_mcu_planes(cb_plane, (width + 1) // 2, (height + 1) // 2, 1, 1))
        cr_blocks = list(_mcu_planes(cr_plane, (width + 1) // 2, (height + 1) // 2, 1, 1))
        y_pred = cb_pred = cr_pred = 0
        for index in range(len(cb_blocks)):
            for sub in range(4):
                y_pred = _encode_component(writer, [y_blocks[index * 4 + sub]], y_pred)
            cb_pred = _encode_component(writer, [cb_blocks[index]], cb_pred)
            cr_pred = _encode_component(writer, [cr_blocks[index]], cr_pred)
    else:
        y_blocks = list(_mcu_planes(y_plane, width, height, 1, 1))
        cb_blocks = list(_mcu_planes(cb_plane, width, height, 1, 1))
        cr_blocks = list(_mcu_planes(cr_plane, width, height, 1, 1))
        y_pred = cb_pred = cr_pred = 0
        for index in range(len(y_blocks)):
            y_pred = _encode_component(writer, [y_blocks[index]], y_pred)
            cb_pred = _encode_component(writer, [cb_blocks[index]], cb_pred)
            cr_pred = _encode_component(writer, [cr_blocks[index]], cr_pred)
    out += writer.finish()
    out += b"\xff\xd9"
    _need(len(out) <= MAX_ENCODED, "encoded_over_bound")
    return bytes(out)


def dc_jpeg(width: int, height: int, levels: list[int], dc_step: int = 1) -> bytes:
    """Hand-built single-component baseline JPEG whose blocks are DC-only."""
    _need(width % 8 == 0 and height % 8 == 0, "dc_fixture_dimensions_must_be_block_aligned")
    blocks = (width // 8) * (height // 8)
    _need(len(levels) == blocks, "dc_fixture_level_count_mismatch")
    out = bytearray(b"\xff\xd8")
    out += _chunk(0xDB, bytes((0x00,)) + bytes([dc_step] * 64))
    out += _chunk(0xC0, bytes((8,)) + struct.pack(">HH", height, width) + bytes((1, 1, 0x11, 0)))
    out += _flat_dht(0, 0, _DC_SYMBOLS, 4)
    out += _flat_dht(1, 0, _AC_SYMBOLS, 8)
    out += _chunk(0xDA, bytes((1,)) + bytes((1, 0x00)) + bytes((0, 63, 0)))
    writer = _BitWriter()
    predictor = 0
    for level in levels:
        dc = int(round((level - 128) * 8 / dc_step))
        size, bits = _category(dc - predictor)
        writer.write(_DC_CODES[size], 4)
        if size:
            writer.write(bits, size)
        predictor = dc
        writer.write(_AC_CODES[0x00], 8)
    out += writer.finish()
    out += b"\xff\xd9"
    _need(len(out) <= MAX_ENCODED, "encoded_over_bound")
    return bytes(out)


CASES = (
    {"id": "dc_gray_8x8", "kind": "dc", "width": 8, "height": 8, "levels": [96], "dc_step": 1,
     "reference": "ABSOLUTE"},
    {"id": "dc_gray_16x16", "kind": "dc", "width": 16, "height": 16, "levels": [32, 64, 96, 160],
     "dc_step": 1, "reference": "ABSOLUTE"},
    {"id": "dc_gray_32x24", "kind": "dc", "width": 32, "height": 24,
     "levels": [16, 48, 80, 112, 144, 176, 208, 232, 24, 72, 120, 200], "dc_step": 1,
     "reference": "ABSOLUTE"},
    {"id": "jpeg_444_256", "kind": "encoded", "width": 256, "height": 256, "sampling": "444",
     "quant_scale": 16, "reference": "DIFFERENTIAL"},
    {"id": "jpeg_420_256", "kind": "encoded", "width": 256, "height": 256, "sampling": "420",
     "quant_scale": 16, "reference": "DIFFERENTIAL"},
    {"id": "jpeg_444_256_lowq", "kind": "encoded", "width": 256, "height": 256, "sampling": "444",
     "quant_scale": 48, "reference": "DIFFERENTIAL"},
    {"id": "jpeg_420_1024", "kind": "encoded", "width": 1024, "height": 1024, "sampling": "420",
     "quant_scale": 16, "reference": "DIFFERENTIAL"},
    {"id": "jpeg_444_1024", "kind": "encoded", "width": 1024, "height": 1024, "sampling": "444",
     "quant_scale": 24, "reference": "DIFFERENTIAL"},
    {"id": "malformed_truncated", "kind": "malformed", "source": "jpeg_420_256",
     "damage": "truncated_scan", "reference": "NOT_VERIFIED"},
    {"id": "malformed_bad_length", "kind": "malformed", "source": "jpeg_420_256",
     "damage": "bad_segment_length", "reference": "NOT_VERIFIED"},
    {"id": "progressive_256", "kind": "not_run", "width": 256, "height": 256,
     "reason": "No approved progressive JPEG encoder exists in the verified environment; a hand-built progressive stream would need unapproved encoder logic.",
     "reference": "NOT_RUN"},
    {"id": "progressive_1024", "kind": "not_run", "width": 1024, "height": 1024,
     "reason": "Same progressive-encoder gap as progressive_256; the class stays NOT_RUN instead of being dropped.",
     "reference": "NOT_RUN"},
)
BY_ID = {case["id"]: case for case in CASES}
_MEASURED: dict[str, dict] = {}


def _emit_base(case) -> bytes:
    if case["kind"] == "dc":
        return dc_jpeg(case["width"], case["height"], case["levels"], case["dc_step"])
    if case["kind"] == "encoded":
        return encode_jpeg(case["width"], case["height"], case["sampling"], case["quant_scale"])
    if case["kind"] == "malformed":
        source = _emit_base(BY_ID[case["source"]])
        if case["damage"] == "truncated_scan":
            sos = source.find(b"\xff\xda")
            _need(sos > 0, "malformed_source_has_no_sos")
            keep = sos + 20 + (len(source) - sos - 20) // 2
            return source[:keep]
        if case["damage"] == "bad_segment_length":
            data = bytearray(source)
            sos = data.find(b"\xff\xda")
            _need(sos > 0, "malformed_source_has_no_sos")
            struct.pack_into(">H", data, sos + 2, 0x7FFF)
            return bytes(data)
        raise ValueError("unknown_damage")
    raise ValueError("case_not_runnable")


def measure(case_id: str) -> dict:
    if case_id in _MEASURED:
        return _MEASURED[case_id]
    case = BY_ID[case_id]
    if case["kind"] == "not_run":
        record = {"bytes": 0, "sha256": None}
    else:
        data = _emit_base(case)
        record = {"bytes": len(data), "sha256": "sha256:" + hashlib.sha256(data).hexdigest()}
    _MEASURED[case_id] = record
    return record


def fixture_bytes(case_id: str) -> bytes:
    """Regenerate one case's exact encoded bytes (no file system access)."""
    need(case_id in BY_ID, "unknown_case")
    need(BY_ID[case_id]["kind"] != "not_run", "not_run_case_has_no_bytes")
    return _emit_base(BY_ID[case_id])


def write_fixture(case_id: str, path) -> dict:
    case = BY_ID[case_id]
    _need(case["kind"] != "not_run", "not_run_case_has_no_bytes")
    data = _emit_base(case)
    _need(len(data) <= MAX_ENCODED, "encoded_over_bound")
    with open(path, "xb") as stream:
        stream.write(data)
    digest = hashlib.sha256(data).hexdigest()
    _need("sha256:" + digest == measure(case_id)["sha256"], "fixture_regeneration_mismatch")
    return {"case_id": case_id, "bytes": len(data), "sha256": digest}
