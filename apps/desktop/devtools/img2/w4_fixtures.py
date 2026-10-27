"""Deterministic small W4 fixtures for real-WIC ROI output classification.

Only the standard library is used. Bgr565/Bgr555 bytes come from the retained W3
generator so the two slices share exact source pixels without re-implementing the
bitfield packing.
"""
from __future__ import annotations

import hashlib
from pathlib import Path
import struct
import zlib

from w3_fixtures import fixture_bytes as w3_fixture_bytes


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _png(width: int, height: int, rgba: bytes) -> bytes:
    assert len(rgba) == width * height * 4
    alpha = any(rgba[i] != 255 for i in range(3, len(rgba), 4))
    color = 6 if alpha else 2
    raw = bytearray()
    for y in range(height):
        raw.append(0)
        for x in range(width):
            start = (y * width + x) * 4
            raw.extend(rgba[start:start + (4 if color == 6 else 3)])

    def chunk(tag: bytes, body: bytes) -> bytes:
        return struct.pack(">I", len(body)) + tag + body + struct.pack(
            ">I", zlib.crc32(tag + body) & 0xFFFFFFFF)

    header = struct.pack(">IIBBBBB", width, height, 8, color, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) +
            chunk(b"IDAT", zlib.compress(bytes(raw), 9)) + chunk(b"IEND", b""))


def _bmp24(width: int, height: int, rgba: bytes) -> bytes:
    stride = (width * 3 + 3) & ~3
    payload = bytearray()
    for y in range(height - 1, -1, -1):
        row = bytearray()
        for x in range(width):
            start = (y * width + x) * 4
            r, g, b = rgba[start:start + 3]
            row.extend((b, g, r))
        row.extend(b"\x00" * (stride - len(row)))
        payload.extend(row)
    size = 14 + 40 + len(payload)
    file_header = b"BM" + struct.pack("<IHHI", size, 0, 0, 54)
    dib = struct.pack("<IiiHHIIiiII", 40, width, height, 1, 24, 0, len(payload), 2835, 2835, 0, 0)
    return file_header + dib + bytes(payload)


def _pattern(width: int, height: int, alpha: bool = False) -> bytes:
    out = bytearray()
    for y in range(height):
        for x in range(width):
            out.extend(((x * 53 + y * 11) & 255, (x * 17 + y * 71) & 255,
                        (x * 97 + y * 29) & 255, (x * 41 + y * 13) & 255 if alpha else 255))
    return bytes(out)


def _large(width: int, height: int) -> bytes:
    out = bytearray()
    for y in range(height):
        for x in range(width):
            out.extend(((x * 7 + y * 3) & 255, (x * 11 + y * 5) & 255,
                        (x * 13 + y * 17) & 255, 255))
    return bytes(out)


def generate(root: Path) -> list[dict]:
    root.mkdir(parents=True, exist_ok=False)
    raw = {
        "png_rgb": (7, 5, _pattern(7, 5), _png(7, 5, _pattern(7, 5))),
        "png_rgba": (6, 4, _pattern(6, 4, alpha=True), _png(6, 4, _pattern(6, 4, alpha=True))),
        "bmp24": (7, 5, _pattern(7, 5), _bmp24(7, 5, _pattern(7, 5))),
        "png_large": (64, 64, _large(64, 64), _png(64, 64, _large(64, 64))),
    }
    w3 = w3_fixture_bytes()

    def dims(data):
        return struct.unpack_from("<ii", data, 18)

    raw["bmp565"] = (*dims(w3["primaries"])[:1], abs(dims(w3["primaries"])[1]), b"", w3["primaries"])
    raw["bmp565_odd"] = (*dims(w3["odd_bottom"])[:1], abs(dims(w3["odd_bottom"])[1]), b"", w3["odd_bottom"])
    raw["bmp555_control"] = (*dims(w3["not_bgr565"])[:1], abs(dims(w3["not_bgr565"])[1]), b"", w3["not_bgr565"])
    raw["bmp565_truncated"] = (*dims(w3["truncated"])[:1], abs(dims(w3["truncated"])[1]), b"", w3["truncated"])
    raw["bmp565_unsupported"] = (*dims(w3["unsupported"])[:1], abs(dims(w3["unsupported"])[1]), b"", w3["unsupported"])
    records = []
    for cid, (width, height, rgba, data) in raw.items():
        name = f"{cid}.{'bmp' if cid.startswith('bmp') else 'png'}"
        (root / name).write_bytes(data)
        record = {
            "id": cid,
            "path": name,
            "format": "Bmp" if name.endswith(".bmp") else "Png",
            "width": width,
            "height": height,
            "bytes": len(data),
            "sha256": sha(data),
        }
        if rgba:
            record["expected_rgba_sha256"] = sha(rgba)
            record["expected_rgba_hex"] = rgba.hex()
        records.append(record)
    return records


def main() -> int:
    import argparse
    import json
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    args = parser.parse_args()
    print(json.dumps(generate(args.root), indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
