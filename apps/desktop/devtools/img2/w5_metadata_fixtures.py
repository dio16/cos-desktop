"""Deterministic small W5 metadata fixtures.

TIFF is used for direct orientation/ICC tag control. The ICC payload is a
minimal, structurally defined matrix profile generated here, so its bytes and
provenance are reproducible; no colour-fidelity claim is made.
"""
from __future__ import annotations

import hashlib
from pathlib import Path
import struct
import zlib

from w3_fixtures import fixture_bytes as w3_fixture_bytes

MARKERS = bytes((
    255, 0, 0,   0, 255, 0,   0, 0, 255,
    255, 255, 0, 0, 255, 255, 255, 0, 255,
))
WIDTH, HEIGHT = 3, 2


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _png() -> bytes:
    raw = b"".join(b"\x00" + MARKERS[y * WIDTH * 3:(y + 1) * WIDTH * 3] for y in range(HEIGHT))

    def chunk(tag: bytes, body: bytes) -> bytes:
        return struct.pack(">I", len(body)) + tag + body + struct.pack(
            ">I", zlib.crc32(tag + body) & 0xFFFFFFFF)

    return (b"\x89PNG\r\n\x1a\n" +
            chunk(b"IHDR", struct.pack(">IIBBBBB", WIDTH, HEIGHT, 8, 2, 0, 0, 0)) +
            chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def _bmp24() -> bytes:
    stride = (WIDTH * 3 + 3) & ~3
    payload = bytearray()
    for y in range(HEIGHT - 1, -1, -1):
        row = bytearray()
        for x in range(WIDTH):
            o = (y * WIDTH + x) * 3
            r, g, b = MARKERS[o:o + 3]
            row.extend((b, g, r))
        row.extend(b"\x00" * (stride - len(row)))
        payload.extend(row)
    size = 14 + 40 + len(payload)
    return (b"BM" + struct.pack("<IHHI", size, 0, 0, 54) +
            struct.pack("<IiiHHIIiiII", 40, WIDTH, HEIGHT, 1, 24, 0, len(payload), 2835, 2835, 0, 0) +
            bytes(payload))


def _xyz(x: float, y: float, z: float) -> bytes:
    def s15(v: float) -> int:
        return int(round(v * 65536))
    return b"XYZ " + b"\x00\x00\x00\x00" + struct.pack(">iii", s15(x), s15(y), s15(z))


def _desc(text: str) -> bytes:
    ascii_part = text.encode("ascii") + b"\x00"
    return (b"desc" + b"\x00\x00\x00\x00" + struct.pack(">I", len(ascii_part)) + ascii_part +
            b"\x00\x00\x00\x00" + b"\x00\x00\x00\x00" + b"\x00\x00\x00\x00" + b"\x00" * 67)


def _text(text: str) -> bytes:
    return b"text" + b"\x00\x00\x00\x00" + text.encode("ascii") + b"\x00"


def _curve() -> bytes:
    return b"curv" + b"\x00\x00\x00\x00" + struct.pack(">I", 0)


def _profile(padding: int = 0) -> bytes:
    tags = [
        (b"desc", _desc("W5 deterministic matrix profile")),
        (b"cprt", _text("Public-domain generated for COS Desktop W5 testing")),
        (b"wtpt", _xyz(0.9642, 1.0, 0.8249)),
        (b"rXYZ", _xyz(0.4361, 0.2225, 0.0139)),
        (b"gXYZ", _xyz(0.3851, 0.7169, 0.0971)),
        (b"bXYZ", _xyz(0.1431, 0.0606, 0.7141)),
        (b"rTRC", _curve()), (b"gTRC", _curve()), (b"bTRC", _curve()),
    ]
    offset = 128 + 4 + 12 * len(tags)
    table = struct.pack(">I", len(tags))
    body = b""
    for sig, payload in tags:
        while (offset + len(body)) % 4:
            body += b"\x00"
        table += sig + struct.pack(">II", offset + len(body), len(payload))
        body += payload
    size = 128 + len(table) + len(body)
    while (size) % 4:
        size += 1
    size += padding
    header = bytearray(128)
    struct.pack_into(">I", header, 0, size)
    header[4:8] = b"none"
    struct.pack_into(">I", header, 8, 0x02100000)
    header[12:16] = b"mntr"
    header[16:20] = b"RGB "
    header[20:24] = b"XYZ "
    struct.pack_into(">HHHHHH", header, 24, 2020, 1, 1, 0, 0, 0)
    header[36:40] = b"acsp"
    struct.pack_into(">I", header, 64, 0)
    struct.pack_into(">iii", header, 68, int(0.9642 * 65536), 65536, int(0.8249 * 65536))
    header[80:84] = b"W5PY"
    body = bytes(header) + table + body
    return body + b"\x00" * (size - len(body))


def _tiff(orientation: int | None, icc: bytes | None = None,
          duplicate_orientation: bool = False, truncate_icc: bool = False) -> tuple[bytes, dict]:
    entries = [
        (256, 4, 1, WIDTH), (257, 4, 1, HEIGHT), (258, 3, 3, None),
        (259, 3, 1, 1), (262, 3, 1, 2), (273, 4, 1, None), (274, 3, 1, orientation),
        (277, 3, 1, 3), (278, 4, 1, HEIGHT), (279, 4, 1, WIDTH * HEIGHT * 3),
        (284, 3, 1, 1),
    ]
    if duplicate_orientation:
        entries.append((274, 3, 1, 8))
    if icc is not None:
        entries.append((34675, 7, len(icc), None))
    entries = [e for e in entries if e[3] is not None or e[0] in (258, 273, 34675)]
    entries.sort(key=lambda e: e[0])
    ifd_size = 2 + 12 * len(entries) + 4
    extra = 8 + ifd_size
    extras = bytearray()
    offsets = {}
    # BitsPerSample array
    offsets[258] = extra
    extras.extend(struct.pack("<HHH", 8, 8, 8))
    while (extra + len(extras)) % 4:
        extras.append(0)
    if icc is not None:
        offsets[34675] = extra + len(extras)
        payload = icc[:64] if truncate_icc else icc
        extras.extend(payload)
        while (extra + len(extras)) % 4:
            extras.append(0)
        icc_offset = offsets[34675]
        icc_length = len(icc) if truncate_icc else len(icc)
    else:
        icc_offset = icc_length = None
    offsets[273] = extra + len(extras)
    extras.extend(MARKERS)
    out = bytearray(b"II" + struct.pack("<HI", 42, 8))
    out.extend(struct.pack("<H", len(entries)))
    orientation_entry_offset = None
    for tag, typ, count, value in entries:
        entry_pos = len(out)
        out.extend(struct.pack("<HHI", tag, typ, count))
        if tag == 258:
            out.extend(struct.pack("<I", offsets[258]))
        elif tag == 273:
            out.extend(struct.pack("<I", offsets[273]))
        elif tag == 34675:
            out.extend(struct.pack("<I", icc_offset))
        elif typ == 3 and count == 1:
            out.extend(struct.pack("<H", value) + b"\x00\x00")
            if tag == 274 and orientation_entry_offset is None:
                orientation_entry_offset = entry_pos
        else:
            out.extend(struct.pack("<I", value))
    out.extend(struct.pack("<I", 0))
    out.extend(extras)
    meta = {"orientation_tag_offset": orientation_entry_offset,
            "orientation_value": orientation, "icc_tag_offset": icc_offset,
            "icc_declared_length": icc_length}
    return bytes(out), meta


def generate(root: Path) -> list[dict]:
    root.mkdir(parents=True, exist_ok=False)
    profile = _profile()
    cases = []
    def add(cid, name, data, **fields):
        (root / name).write_bytes(data)
        width = fields.pop("width", WIDTH)
        height = fields.pop("height", HEIGHT)
        source_hex = fields.pop("source_raster_hex", MARKERS.hex())
        rec = {"id": cid, "path": name, "bytes": len(data), "sha256": sha(data),
               "width": width, "height": height, "container": name.rsplit(".", 1)[-1],
               "source_raster_hex": source_hex, **fields}
        cases.append(rec)
    add("png_plain", "plain.png", _png(), orientation=None, icc=None, expected="metadata_absent")
    add("bmp24_plain", "plain.bmp", _bmp24(), orientation=None, icc=None, expected="metadata_absent")
    for value in (1, 3, 6, 8):
        data, meta = _tiff(value)
        add(f"tiff_ori{value}", f"ori{value}.tif", data, orientation=value, icc=None,
            expected="orientation_metadata", **meta)
    data, meta = _tiff(99)
    add("tiff_ori_invalid", "ori99.tif", data, orientation=99, icc=None,
        expected="invalid_orientation", **meta)
    data, meta = _tiff(6, duplicate_orientation=True)
    add("tiff_ori_conflict", "oriconflict.tif", data, orientation=6, icc=None,
        expected="conflicting_orientation", **meta)
    data, meta = _tiff(1)
    add("tiff_icc_absent", "iccabsent.tif", data, orientation=1, icc=None,
        expected="icc_absent", **meta)
    data, meta = _tiff(1, icc=profile)
    add("tiff_icc_valid", "iccvalid.tif", data, orientation=1, icc="generated_matrix",
        icc_sha256=sha(profile), icc_bytes=len(profile), expected="icc_present", **meta)
    data, meta = _tiff(1, icc=profile, truncate_icc=True)
    add("tiff_icc_truncated", "icctruncated.tif", data, orientation=1, icc="generated_matrix_truncated",
        icc_sha256=sha(profile[:64]), expected="icc_truncated", **meta)
    oversized_profile = _profile(70000)
    data, meta = _tiff(1, icc=oversized_profile)
    add("tiff_icc_oversized", "iccoversized.tif", data, orientation=1, icc="oversized",
        icc_bytes=len(oversized_profile), icc_sha256=sha(oversized_profile), expected="metadata_cap", **meta)
    data, meta = _tiff(6)
    add("tiff_ori6_apply", "ori6apply.tif", data, orientation=6, icc=None,
        expected="explicit_transform", **meta)
    unsupported = w3_fixture_bytes()["unsupported"]
    add("unsupported_source", "unsupported.bmp", unsupported, orientation=None, icc=None,
        expected="unsupported_container", width=3, height=3)
    return cases


def main() -> int:
    import argparse, json
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    args = parser.parse_args()
    print(json.dumps(generate(args.root), indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
