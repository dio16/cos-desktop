"""Deterministic bounded W7 large-dimension fixtures.

The generator is a pure function of the case definition: the same bytes are
reproduced by hashing the emitted stream, so a recorded digest is checkable
without trusting the producer's file. Large PNGs are streamed row by row and
never buffered as a whole frame.
"""

from __future__ import annotations

import hashlib
import struct
import zlib

MAX_ENCODED_BYTES = 64 * 1024 * 1024

# Twelve fixed cases: six materialization candidates, four admission refusals
# (two with a 1x1 ROI request) and two corrupt-input refusals.
CASES = (
    {"id": "png_2048", "codec": "png", "width": 2048, "height": 2048, "role": "decode"},
    {"id": "png_4096", "codec": "png", "width": 4096, "height": 4096, "role": "decode"},
    {"id": "bmp24_2048", "codec": "bmp24", "width": 2048, "height": 2048, "role": "decode"},
    {"id": "bmp24_4096", "codec": "bmp24", "width": 4096, "height": 4096, "role": "decode"},
    {"id": "bgr565_2048", "codec": "bgr565", "width": 2048, "height": 2048, "role": "decode"},
    {"id": "bgr565_4096", "codec": "bgr565", "width": 4096, "height": 4096, "role": "decode"},
    {"id": "png_8192", "codec": "png", "width": 8192, "height": 8192, "role": "refusal"},
    {"id": "png_16384", "codec": "png", "width": 16384, "height": 16384, "role": "refusal"},
    {"id": "roi_8192_1x1", "codec": "png", "width": 8192, "height": 8192, "role": "roi_refusal",
     "source": "png_8192", "roi": "0,0,1,1", "roi_width": 1, "roi_height": 1},
    {"id": "roi_16384_1x1", "codec": "png", "width": 16384, "height": 16384, "role": "roi_refusal",
     "source": "png_16384", "roi": "0,0,1,1", "roi_width": 1, "roi_height": 1},
    # Both corrupt inputs stay inside the W7 budget so admission succeeds and the
    # container failure is exercised at materialization instead of being masked by
    # an earlier dimension refusal.
    {"id": "corrupt_4096_truncated", "codec": "png", "width": 4096, "height": 4096,
     "role": "corrupt", "source": "png_4096", "damage": "truncated"},
    {"id": "corrupt_4096_dimension", "codec": "png", "width": 4000, "height": 4096,
     "role": "corrupt", "source": "png_4096", "damage": "dimension_tampered",
     "tampered_width": 4000},
)
CASE_IDS = tuple(case["id"] for case in CASES)
BY_ID = {case["id"]: case for case in CASES}
_MASKS = (0xF800, 0x07E0, 0x001F)
# A 64-pixel period keeps the encoded stream small while every row and column
# differs, so a constant or transposed raster cannot pass the oracle.
_PERIOD = 64


def _pixel(x: int, y: int) -> tuple[int, int, int]:
    red = ((x % _PERIOD) * 4) & 0xFF
    green = ((y % _PERIOD) * 4) & 0xFF
    blue = 32 + ((((x // _PERIOD) + (y // _PERIOD)) & 1) * 128)
    return red, green, blue


def _tile(blue: int, green: int) -> bytes:
    row = bytearray(_PERIOD * 3)
    for x in range(_PERIOD):
        row[3 * x] = (x * 4) & 0xFF
        row[3 * x + 1] = green
        row[3 * x + 2] = blue
    return bytes(row)


_TILES = {(blue, green): _tile(blue, green)
          for blue in (32, 160) for green in (0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44,
                                              48, 52, 56, 60, 64, 68, 72, 76, 80, 84, 88, 92,
                                              96, 100, 104, 108, 112, 116, 120, 124, 128, 132,
                                              136, 140, 144, 148, 152, 156, 160, 164, 168, 172,
                                              176, 180, 184, 188, 192, 196, 200, 204, 208, 212,
                                              216, 220, 224, 228, 232, 236, 240, 244, 248, 252)}


def _tile_for(x_block_parity: int, y: int) -> bytes:
    blue = 32 + (((x_block_parity + (y // _PERIOD)) & 1) * 128)
    green = ((y % _PERIOD) * 4) & 0xFF
    return _TILES[(blue, green)]


def _row_rgb(y: int, width: int) -> bytes:
    if width % _PERIOD:
        raise ValueError("width_not_period_aligned")
    return (_tile_for(0, y) + _tile_for(1, y)) * (width // (2 * _PERIOD))


_FILTERED: dict[tuple[int, int, int], bytes] = {}


def _filtered_row(y: int, width: int) -> bytes:
    """Sub-filtered row; PNG Sub uses zero for the first bpp bytes.

    Only 128 rows are distinct (64 green levels times two blue parities), so the
    filtered bytes are built once per class and reused, which keeps a true
    per-row Sub filter without repeating a cyclically wrapped tile.
    """
    key = (width, y % _PERIOD, (y // _PERIOD) & 1)
    cached = _FILTERED.get(key)
    if cached is not None:
        return cached
    raw = _row_rgb(y, width)
    filtered = bytearray(len(raw))
    for x in range(len(raw)):
        filtered[x] = (raw[x] - (raw[x - 3] if x >= 3 else 0)) & 0xFF
    result = bytes(filtered)
    _FILTERED[key] = result
    return result


def _chunk(tag: bytes, body: bytes) -> bytes:
    return struct.pack(">I", len(body)) + tag + body + struct.pack(">I", zlib.crc32(tag + body) & 0xFFFFFFFF)


def _png(case, sink) -> None:
    width, height = case["width"], case["height"]
    sink(b"\x89PNG\r\n\x1a\n")
    sink(_chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)))
    compressor = zlib.compressobj(6)
    pending = bytearray()

    def flush(force: bool) -> None:
        data = compressor.compress(bytes(pending)) if pending else b""
        pending.clear()
        if data:
            sink(_chunk(b"IDAT", data))
        if force:
            tail = compressor.flush()
            if tail:
                sink(_chunk(b"IDAT", tail))

    for y in range(height):
        # Filter type 1 (Sub) keeps periodic ramps highly compressible while the
        # row payload still carries every true pixel value.
        pending.append(1)
        pending.extend(_filtered_row(y, width))
        if len(pending) >= 1 << 20:
            flush(False)
    flush(True)
    sink(_chunk(b"IEND", b""))


def _bmp24(case, sink) -> None:
    width, height = case["width"], case["height"]
    stride = ((width * 3) + 3) // 4 * 4
    size = 54 + stride * height
    sink(struct.pack("<2sIHHI", b"BM", size, 0, 0, 54))
    sink(struct.pack("<IiiHHIIiiII", 40, width, height, 1, 24, 0, stride * height, 2835, 2835, 0, 0))
    padding = b"\x00" * (stride - width * 3)
    for index in range(height):
        y = height - 1 - index
        raw = _row_rgb(y, width)
        payload = bytearray(width * 3)
        for x in range(width):
            payload[3 * x] = raw[3 * x + 2]
            payload[3 * x + 1] = raw[3 * x + 1]
            payload[3 * x + 2] = raw[3 * x]
        sink(bytes(payload) + padding)


def _bgr565(case, sink) -> None:
    width, height = case["width"], case["height"]
    stride = ((width * 2) + 3) // 4 * 4
    size = 66 + stride * height
    sink(struct.pack("<2sIHHI", b"BM", size, 0, 0, 66))
    sink(struct.pack("<IiiHHIIiiII", 40, width, height, 1, 16, 3, stride * height, 2835, 2835, 0, 0))
    sink(struct.pack("<III", *_MASKS))
    padding = b"\x00" * (stride - width * 2)
    for index in range(height):
        y = height - 1 - index
        payload = bytearray(width * 2)
        for x in range(width):
            r, g, b = _pixel(x, y)
            value = ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3)
            struct.pack_into("<H", payload, 2 * x, value)
        sink(bytes(payload) + padding)


def _emit_base(case, sink) -> None:
    if case["codec"] == "png":
        _png(case, sink)
    elif case["codec"] == "bmp24":
        _bmp24(case, sink)
    elif case["codec"] == "bgr565":
        _bgr565(case, sink)
    else:
        raise ValueError("unknown_codec")


def _emit(case, sink) -> None:
    """Emit one case's exact encoded bytes, applying any derived damage."""
    if "source" in case:
        base = BY_ID[case["source"]]
        data = bytearray()
        _emit_base(base, data.extend)
        if "damage" not in case:
            sink(bytes(data))
            return
        if case.get("damage") == "truncated":
            # A container cut inside its IDAT payload cannot be completed by a
            # lenient decoder, unlike a long prefix that still holds the whole stream.
            sink(bytes(data[: 4096 + 512]))
            return
        if case.get("damage") == "dimension_tampered":
            # IHDR data starts at byte 16; corrupt the declared width and leave
            # the original CRC in place so the container is genuinely invalid.
            struct.pack_into(">I", data, 16, case.get("tampered_width", base["width"] + 1))
            sink(bytes(data))
            return
        raise ValueError("unknown_damage")
    _emit_base(case, sink)


_RGBA_TILES: dict[tuple[int, int, str], bytes] = {}


def _rgba_tile(blue: int, green: int, codec: str) -> bytes:
    key = (blue, green, codec)
    cached = _RGBA_TILES.get(key)
    if cached is not None:
        return cached
    tile = bytearray(_PERIOD * 4)
    for x in range(_PERIOD):
        red = (x * 4) & 0xFF
        r, g, b = red, green, blue
        if codec == "bgr565":
            r = ((r >> 3) << 3) | ((r >> 3) >> 2)
            g = ((g >> 2) << 2) | ((g >> 2) >> 4)
            b = ((b >> 3) << 3) | ((b >> 3) >> 2)
        tile[4 * x] = r
        tile[4 * x + 1] = g
        tile[4 * x + 2] = b
        tile[4 * x + 3] = 0xFF
    result = bytes(tile)
    _RGBA_TILES[key] = result
    return result


def expected_rgba_digest(case_id: str) -> dict:
    """RGBA digest of the declared pattern, derived without parsing the file.

    Bgr565 storage quantizes each channel, so that case applies the same
    five/six/five-bit expansion the independent oracle reconstructs from bytes.
    """
    case = BY_ID[case_id]
    width, height = case["width"], case["height"]
    digest = hashlib.sha256()
    for y in range(height):
        blue = 32 + (((y // _PERIOD) & 1) * 128)
        green = ((y % _PERIOD) * 4) & 0xFF
        other = 32 + ((((y // _PERIOD) + 1) & 1) * 128)
        row = (_rgba_tile(blue, green, case["codec"]) +
               _rgba_tile(other, green, case["codec"])) * (width // (2 * _PERIOD))
        digest.update(row)
    return {"rgba_sha256": "sha256:" + digest.hexdigest(), "pixels": width * height}


_MEASURED: dict[str, dict] = {}


def _measure(case_id: str) -> dict:
    if case_id in _MEASURED:
        return _MEASURED[case_id]
    digest = hashlib.sha256()
    counter = {"bytes": 0}

    def sink(data: bytes) -> None:
        digest.update(data)
        counter["bytes"] += len(data)

    _emit(BY_ID[case_id], sink)
    if counter["bytes"] > MAX_ENCODED_BYTES:
        raise ValueError("encoded_input_over_bound")
    record = {"bytes": counter["bytes"], "sha256": "sha256:" + digest.hexdigest()}
    _MEASURED[case_id] = record
    return record


def expected_encoded_bytes(case_id: str) -> int:
    return _measure(case_id)["bytes"]


def expected_sha256(case_id: str) -> str:
    return _measure(case_id)["sha256"]


def write_fixture(case_id: str, path) -> dict:
    digest = hashlib.sha256()
    size = {"bytes": 0}

    def sink(data: bytes) -> None:
        digest.update(data)
        size["bytes"] += len(data)

    with open(path, "xb") as stream:

        def write(data: bytes) -> None:
            stream.write(data)
            sink(data)

        _emit(BY_ID[case_id], write)
    if size["bytes"] > MAX_ENCODED_BYTES:
        raise ValueError("encoded_input_over_bound")
    return {"id": case_id, "bytes": size["bytes"], "sha256": digest.hexdigest()}


def manifest():
    records = []
    for case in CASES:
        record = dict(case)
        record["bytes"] = expected_encoded_bytes(case["id"])
        record["sha256"] = expected_sha256(case["id"])
        records.append(record)
    return records
