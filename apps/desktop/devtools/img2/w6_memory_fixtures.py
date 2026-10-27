"""Deterministic bounded W6 image fixtures and independent raster checks."""
from __future__ import annotations

from functools import lru_cache
import hashlib
import json
from pathlib import Path
import struct
import zlib

import w2_independent_oracle as png_oracle

FIXTURE_IDS = ("png_256", "bmp24_256", "bgr565_256",
               "png_1024", "bmp24_1024", "bgr565_1024")
_MASKS = (0xF800, 0x07E0, 0x001F)
_MAX_SOURCE = 4 * 1024 * 1024
_MAX_OUTPUT = 16 * 1024 * 1024


def _sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _pattern(width: int, height: int) -> bytes:
    out = bytearray(width * height * 4)
    pos = 0
    for y in range(height):
        for x in range(width):
            out[pos:pos + 4] = bytes(((x * 7 + y * 3) & 255,
                                      (x * 11 + y * 5) & 255,
                                      (x * 13 + y * 17) & 255, 255))
            pos += 4
    return bytes(out)


def _png(width: int, height: int, rgba: bytes) -> bytes:
    raw = bytearray()
    for y in range(height):
        raw.append(0)
        for x in range(width):
            p = (y * width + x) * 4
            raw.extend(rgba[p:p + 3])

    def chunk(tag: bytes, body: bytes) -> bytes:
        return struct.pack(">I", len(body)) + tag + body + struct.pack(">I", zlib.crc32(tag + body) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(bytes(raw), 9)) + chunk(b"IEND", b"")


def _bmp(width: int, height: int, rgba: bytes, depth: int) -> bytes:
    row_bytes = width * (depth // 8)
    stride = (row_bytes + 3) & ~3
    payload = bytearray()
    for y in range(height - 1, -1, -1):
        row = bytearray()
        for x in range(width):
            p = (y * width + x) * 4
            r, g, b = rgba[p:p + 3]
            if depth == 24:
                row.extend((b, g, r))
            else:
                packed = ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3)
                row.extend(struct.pack("<H", packed))
        row.extend(bytes(stride - len(row)))
        payload.extend(row)
    offset = 54 if depth == 24 else 66
    compression = 0 if depth == 24 else 3
    size = offset + len(payload)
    header = b"BM" + struct.pack("<IHHI", size, 0, 0, offset)
    dib = struct.pack("<IiiHHIIiiII", 40, width, height, 1, depth, compression,
                      len(payload), 2835, 2835, 0, 0)
    return header + dib + (struct.pack("<III", *_MASKS) if depth == 16 else b"") + payload


@lru_cache(maxsize=len(FIXTURE_IDS))
def fixture_bytes(fixture_id: str) -> bytes:
    if fixture_id not in FIXTURE_IDS:
        raise ValueError(f"unknown W6 fixture: {fixture_id!r}")
    codec, dimension = fixture_id.rsplit("_", 1)
    width = height = int(dimension)
    rgba = _pattern(width, height)
    return (_png(width, height, rgba) if codec == "png" else
            _bmp(width, height, rgba, 24 if codec == "bmp24" else 16))


def _decode_source(data: bytes) -> dict:
    if not isinstance(data, bytes) or len(data) > _MAX_SOURCE:
        raise ValueError("encoded source must be bytes within the 4 MiB source bound")
    if data.startswith(b"\x89PNG\r\n\x1a\n"):
        try:
            width, height, rgba = png_oracle.png_pixels(data)
            header = struct.unpack_from(">IIBBBBB", data, 16)
        except Exception as exc:
            raise ValueError("invalid PNG source") from exc
        _need(header[2:] == (8, 2, 0, 0, 0), "source PNG must be noninterlaced RGB8")
        return {"width": width, "height": height, "rgba": rgba, "pixel_format": "Bgr24",
                "bits_per_channel": 8, "bits_per_pixel": 24, "logical_row_bytes": width * 3,
                "encoded_row_stride": width * 3, "masks": None}
    try:
        _need(len(data) >= 54 and data[:2] == b"BM", "invalid BMP header")
        file_size, reserved1, reserved2, offset = struct.unpack_from("<IHHI", data, 2)
        dib, width, signed_height, planes, depth, compression, image_size = struct.unpack_from("<IiiHHII", data, 14)
        _need(file_size == len(data) and reserved1 == reserved2 == 0, "BMP file size/reserved fields invalid")
        _need(dib == 40 and 1 <= width <= 1024 and signed_height != 0 and abs(signed_height) <= 1024, "unsupported BMP dimensions/header")
        _need(planes == 1 and depth in (24, 16), "unsupported BMP planes/depth")
        height = abs(signed_height)
        if depth == 24:
            _need(compression == 0 and offset == 54, "BMP24 must be uncompressed with a 40-byte DIB")
            masks = None
        else:
            _need(compression == 3 and offset == 66 and len(data) >= 66, "BMP565 must use BI_BITFIELDS and three masks")
            masks = struct.unpack_from("<III", data, 54)
            _need(masks == _MASKS, "BMP565 masks are invalid")
        row_bytes = width * (depth // 8)
        stride = (row_bytes + 3) & ~3
        _need(image_size in (0, stride * height) and len(data) == offset + stride * height, "BMP raster size/stride invalid")
        rgba = bytearray(width * height * 4)
        for y in range(height):
            sy = y if signed_height < 0 else height - y - 1
            start = offset + sy * stride
            for x in range(width):
                dst = (y * width + x) * 4
                if depth == 24:
                    b, g, r = data[start + x * 3:start + x * 3 + 3]
                else:
                    value = struct.unpack_from("<H", data, start + x * 2)[0]
                    r5, g6, b5 = (value >> 11) & 31, (value >> 5) & 63, value & 31
                    r, g, b = (r5 << 3) | (r5 >> 2), (g6 << 2) | (g6 >> 4), (b5 << 3) | (b5 >> 2)
                rgba[dst:dst + 4] = bytes((r, g, b, 255))
        return {"width": width, "height": height, "rgba": bytes(rgba),
                "pixel_format": "Bgr24" if depth == 24 else "Bgr565",
                "bits_per_channel": 8 if depth == 24 else 6, "bits_per_pixel": depth,
                "logical_row_bytes": row_bytes, "encoded_row_stride": stride, "masks": masks}
    except (struct.error, OverflowError) as exc:
        raise ValueError("invalid BMP source") from exc


def _need(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def source_rgba(fixture: dict, data: bytes) -> dict:
    source = _decode_source(data)
    _need(source["width"] == fixture.get("width") and source["height"] == fixture.get("height"), "source dimensions differ from fixture")
    return {k: source[k] for k in ("width", "height", "rgba", "pixel_format", "bits_per_channel", "bits_per_pixel")}


def manifest() -> list[dict]:
    records = []
    for fid in FIXTURE_IDS:
        data = fixture_bytes(fid)
        codec, size = fid.rsplit("_", 1)
        width = height = int(size)
        source = _decode_source(data)
        decoded = width * height * (source["bits_per_pixel"] // 8)
        bgra = width * height * 4
        records.append({"id": fid, "path": fid + (".png" if codec == "png" else ".bmp"),
                        "codec": codec, "width": source["width"], "height": source["height"],
                        "pixel_format": source["pixel_format"], "bits_per_channel": source["bits_per_channel"],
                        "bits_per_pixel": source["bits_per_pixel"], "logical_row_bytes": source["logical_row_bytes"],
                        "decoded_bytes_required": decoded, "bgra32_bytes_required": bgra,
                        "declared_scratch_bytes": 0, "scratch_bytes_required": bgra,
                        "bytes": len(data), "sha256": _sha(data),
                        "expected_rgba_sha256": _sha(source["rgba"]),
                        "generator": "w6_memory_fixtures deterministic x/y RGB pattern; PNG RGB8, BMP24 or BI_BITFIELDS RGB565"})
    return records


def generate(root: Path) -> list[dict]:
    root.mkdir(parents=True, exist_ok=False)
    records = manifest()
    for record in records:
        (root / record["path"]).write_bytes(fixture_bytes(record["id"]))
    (root / "manifest.json").write_text(json.dumps(records, indent=2) + "\n", encoding="utf-8")
    return records


def compare(fixture: dict, input_bytes: bytes, output_png: bytes) -> dict:
    expected = source_rgba(fixture, input_bytes)["rgba"]
    _need(isinstance(output_png, bytes) and len(output_png) <= _MAX_OUTPUT, "output PNG exceeds bound or is not bytes")
    try:
        width, height, actual = png_oracle.png_pixels(output_png)
    except Exception as exc:
        raise ValueError("invalid or unsupported output PNG") from exc
    _need((width, height) == (fixture["width"], fixture["height"]), "output dimensions differ from source")
    mismatch_count = sum(a != b for a, b in zip(expected, actual))
    return {"ok": mismatch_count == 0, "width": width, "height": height,
            "expected_rgba_sha256": _sha(expected), "actual_rgba_sha256": _sha(actual),
            "mismatch_count": mismatch_count,
            "method": "independent encoded-source BMP/PNG parse and W2 CRC/inflate/filter output decode; all-channel pixel comparison"}
