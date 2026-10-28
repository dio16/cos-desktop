"""Deterministic progressive JPEG fixtures with an in-repo minimal encoder.

The encoder emits a real multi-scan progressive stream (SOF2): one DC scan and
one AC scan, both with successive approximation Ah=Al=0, using the same flat
canonical Huffman tables as the W8 generator. No external encoder is involved.
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
_DC_SYMBOLS = list(range(12))
_AC_SYMBOLS = [0x00] + [(run << 4) | size for run in range(16) for size in range(1, 11)] + [0xF0]
_DC_CODES = {symbol: index for index, symbol in enumerate(_DC_SYMBOLS)}
_AC_CODES = {symbol: index for index, symbol in enumerate(_AC_SYMBOLS)}
QUANT = 16


def _need(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def _chunk(marker: int, body: bytes) -> bytes:
    return bytes((0xFF, marker)) + struct.pack(">H", len(body) + 2) + body


def _flat_dht(table_class: int, table_id: int, symbols: list[int], length: int) -> bytes:
    counts = [0] * 16
    counts[length - 1] = len(symbols)
    _need(len(symbols) <= 1 << length, "flat_table_overflows_code_space")
    return _chunk(0xC4, bytes((table_class << 4 | table_id,)) + bytes(counts) + bytes(symbols))


class _Bits:
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


def _grey_blocks(width: int, height: int) -> list[list[list[float]]]:
    blocks = []
    for by in range(0, height, 8):
        for bx in range(0, width, 8):
            block = [[0.0] * 8 for _ in range(8)]
            for y in range(8):
                sy = min(by + y, height - 1)
                for x in range(8):
                    sx = min(bx + x, width - 1)
                    level = ((sx % _PERIOD) * 2 + (sy % _PERIOD) * 3) & 0xFF
                    block[y][x] = float(level - 128)
            blocks.append(block)
    return blocks


def _quantized(block) -> list[list[int]]:
    coefficients = _fdct(block)
    return [[int(round(coefficients[v][u] / QUANT)) for u in range(8)] for v in range(8)]


def encode_progressive(width: int, height: int) -> bytes:
    """Emit a two-scan progressive grayscale JPEG for the deterministic pattern."""
    blocks = [_quantized(block) for block in _grey_blocks(width, height)]
    out = bytearray(b"\xff\xd8")
    out += _chunk(0xDB, bytes((0x00,)) + bytes([QUANT] * 64))
    frame = bytes((8,)) + struct.pack(">HH", height, width) + bytes((1, 1, 0x11, 0))
    out += _chunk(0xC2, frame)                       # SOF2: progressive DCT
    out += _flat_dht(0, 0, _DC_SYMBOLS, 4)           # DC table
    out += _flat_dht(1, 0, _AC_SYMBOLS, 8)           # AC table
    # Scan 1: DC only, initial, no successive approximation.
    out += _chunk(0xDA, bytes((1,)) + bytes((1, 0x00)) + bytes((0, 0, 0x00)))
    writer = _Bits()
    predictor = 0
    for block in blocks:
        size, bits = _category(block[0][0] - predictor)
        writer.write(_DC_CODES[size], 4)
        if size:
            writer.write(bits, size)
        predictor = block[0][0]
    out += writer.finish()
    # Scan 2: AC band 1..63, initial, no successive approximation.
    out += _chunk(0xDA, bytes((1,)) + bytes((1, 0x00)) + bytes((1, 63, 0x00)))
    writer = _Bits()
    for block in blocks:
        run = 0
        for position in range(1, 64):
            v, u = divmod(_ZIGZAG[position], 8)
            value = block[v][u]
            if value == 0:
                run += 1
                continue
            while run > 15:
                writer.write(_AC_CODES[0xF0], 8)
                run -= 16
            size, bits = _category(value)
            writer.write(_AC_CODES[(run << 4) | size], 8)
            writer.write(bits, size)
            run = 0
        if run:
            writer.write(_AC_CODES[0x00], 8)
    out += writer.finish()
    out += b"\xff\xd9"
    _need(len(out) <= MAX_ENCODED, "encoded_over_bound")
    return bytes(out)


CASES = (
    {"id": "progressive_gray_256", "kind": "progressive", "width": 256, "height": 256,
     "reference": "DIFFERENTIAL"},
    {"id": "progressive_gray_1024", "kind": "progressive", "width": 1024, "height": 1024,
     "reference": "DIFFERENTIAL"},
    {"id": "malformed_scan_truncated", "kind": "malformed", "source": "progressive_gray_256",
     "width": 256, "height": 256, "damage": "truncated_scan", "reference": "NOT_VERIFIED"},
    {"id": "malformed_spectral_invalid", "kind": "malformed", "source": "progressive_gray_256",
     "width": 256, "height": 256, "damage": "invalid_spectral", "reference": "NOT_VERIFIED"},
    {"id": "baseline_control_420_256", "kind": "control", "width": 256, "height": 256,
     "reference": "CONTROL"},
)
BY_ID = {case["id"]: case for case in CASES}
_MEASURED: dict[str, dict] = {}


def _emit(case) -> bytes:
    if case["kind"] == "progressive":
        return encode_progressive(case["width"], case["height"])
    if case["kind"] == "control":
        from w8_jpeg_fixtures import fixture_bytes
        return fixture_bytes("jpeg_420_256")
    if case["kind"] == "malformed":
        source = _emit(BY_ID[case["source"]])
        data = bytearray(source)
        if case["damage"] == "truncated_scan":
            second = data.find(b"\xff\xda", data.find(b"\xff\xda") + 2)
            _need(second > 0, "malformed_source_has_no_second_scan")
            return bytes(data[: second + 20 + (len(data) - second) // 3])
        if case["damage"] == "invalid_spectral":
            # Fixed layout of this generator: SOI 2 + DQT 69 + SOF2 13 + DHT(DC) 33
            # + DHT(AC) 183 places the first SOS at 300, so its Ss byte is 307.
            _need(data[300:302] == b"\xff\xda", "malformed_source_layout_changed")
            data[307] = 40                                 # Ss=40 is outside 0..63
            from w9_progressive_reference import parse_scans
            try:
                parse_scans(bytes(data))
            except ValueError:
                return bytes(data)
            raise ValueError("invalid_spectral_mutation_not_rejected")
        raise ValueError("unknown_damage")
    raise ValueError("case_not_runnable")


def measure(case_id: str) -> dict:
    if case_id in _MEASURED:
        return _MEASURED[case_id]
    data = _emit(BY_ID[case_id])
    record = {"bytes": len(data), "sha256": "sha256:" + hashlib.sha256(data).hexdigest()}
    _MEASURED[case_id] = record
    return record


def fixture_bytes(case_id: str) -> bytes:
    _need(case_id in BY_ID, "unknown_case")
    return _emit(BY_ID[case_id])


def write_fixture(case_id: str, path) -> dict:
    data = _emit(BY_ID[case_id])
    _need(len(data) <= MAX_ENCODED, "encoded_over_bound")
    with open(path, "xb") as stream:
        stream.write(data)
    return {"case_id": case_id, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}
