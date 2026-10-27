"""Independent W5 metadata and raster oracle.

Parses the original PNG/BMP24/TIFF bytes itself, including TIFF orientation and
ICC tags, and defines rotations from the stored raster. No WIC output supplies an
expectation.
"""
from __future__ import annotations

import hashlib
import struct

from w2_independent_oracle import png_pixels
from w4_region_oracle import bmp24


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def rotate(rgba: bytes, width: int, height: int, degrees: int) -> tuple[int, int, bytes]:
    assert degrees in (0, 90, 180, 270)
    if degrees == 0:
        return width, height, rgba
    if degrees == 180:
        out = bytearray()
        for y in range(height - 1, -1, -1):
            for x in range(width - 1, -1, -1):
                o = (y * width + x) * 4
                out.extend(rgba[o:o + 4])
        return width, height, bytes(out)
    out = bytearray()
    if degrees == 90:
        for x in range(width):
            for y in range(height - 1, -1, -1):
                o = (y * width + x) * 4
                out.extend(rgba[o:o + 4])
        return height, width, bytes(out)
    for x in range(width - 1, -1, -1):
        for y in range(height):
            o = (y * width + x) * 4
            out.extend(rgba[o:o + 4])
    return height, width, bytes(out)


def tiff(data: bytes) -> dict:
    assert len(data) <= 1024 * 1024 and data[:2] in (b"II", b"MM")
    little = data[:2] == b"II"
    fmt = "<" if little else ">"
    assert struct.unpack_from(fmt + "HI", data, 2)[0] == 42
    ifd = struct.unpack_from(fmt + "I", data, 4)[0]
    count = struct.unpack_from(fmt + "H", data, ifd)[0]
    tags = {}
    orientation_entries = []
    for index in range(count):
        pos = ifd + 2 + 12 * index
        tag, typ, n = struct.unpack_from(fmt + "HHI", data, pos)
        raw = data[pos + 8:pos + 12]
        if typ == 3 and n == 1:
            value = struct.unpack_from(fmt + "H", raw)[0]
            offset = None
        elif typ == 4 and n == 1:
            value = struct.unpack_from(fmt + "I", raw)[0]
            offset = None
        else:
            offset = struct.unpack_from(fmt + "I", raw)[0]
            value = n
        tags.setdefault(tag, []).append({"type": typ, "count": n, "value": value,
                                         "offset": offset, "entry": pos})
        if tag == 274:
            orientation_entries.append(tags[tag][-1])
    width = tags[256][0]["value"]
    height = tags[257][0]["value"]
    samples = tags.get(277, [{"value": 3}])[0]["value"]
    compression = tags.get(259, [{"value": 1}])[0]["value"]
    strip = tags[273][0]["value"]
    byte_count = tags[279][0]["value"]
    assert compression == 1 and samples == 3
    assert byte_count >= width * height * 3 and strip + byte_count <= len(data)
    raw = data[strip:strip + byte_count]
    rgba = bytearray()
    for i in range(width * height):
        r, g, b = raw[i * 3:i * 3 + 3]
        rgba.extend((r, g, b, 255))
    icc = None
    if 34675 in tags:
        entry = tags[34675][0]
        length = entry["count"]
        if entry["offset"] + length <= len(data):
            icc = {"declared": length, "bytes": data[entry["offset"]:entry["offset"] + length]}
        else:
            icc = {"declared": length, "bytes": data[entry["offset"]:],
                   "truncated": True}
    return {"width": width, "height": height, "rgba": bytes(rgba),
            "orientation_values": [e["value"] for e in orientation_entries],
            "orientation_entry": orientation_entries[0]["entry"] if orientation_entries else None,
            "icc": icc, "method": "independent TIFF IFD/strip parser"}


def source(fixture: dict, data: bytes) -> dict:
    cid = fixture["id"]
    if cid.startswith("png"):
        w, h, rgba = png_pixels(data)
        return {"width": w, "height": h, "rgba": rgba, "orientation_values": [],
                "icc": None, "method": "W2 stdlib PNG reconstruction"}
    if cid.startswith("bmp24"):
        r = bmp24(data)
        return {**r, "orientation_values": [], "icc": None}
    if cid.startswith("tiff"):
        return tiff(data)
    raise ValueError("fixture_has_no_oracle")


def orientation_spec(values):
    if not values:
        return "absent", None
    if len(set(values)) > 1:
        return "conflicting", values[0]
    value = values[0]
    mapping = {1: "identity", 3: "rotate_180", 6: "rotate_90_cw", 8: "rotate_90_ccw"}
    return ("valid", value) if value in mapping else ("invalid", value)


def degrees_for(value):
    return {1: 0, 3: 180, 6: 90, 8: 270}.get(value)


def compare(fixture, data, output_png, apply_orientation):
    src = source(fixture, data)
    width, height, actual = png_pixels(output_png)
    kind, value = orientation_spec(src["orientation_values"])
    degrees = degrees_for(value) if kind == "valid" else None
    stored = {"width": src["width"], "height": src["height"], "rgba": src["rgba"]}
    rotated = (rotate(src["rgba"], src["width"], src["height"], degrees)
               if degrees is not None else None)
    actual_sha = sha(actual)
    raw_equal = (width, height, actual) == (src["width"], src["height"], src["rgba"])
    rotated_equal = (rotated is not None and (width, height, actual) == rotated)
    if apply_orientation:
        expected = rotated
        observed = "applied_by_probe" if (width, height, actual) == expected else "mismatch"
    elif kind in ("invalid", "conflicting", "absent"):
        expected = (src["width"], src["height"], src["rgba"])
        observed = "not_observed" if raw_equal else "unexpected_transform"
    else:
        expected = (src["width"], src["height"], src["rgba"])
        observed = "not_observed" if raw_equal else ("observed" if rotated_equal else "mismatch")
    ok = (width, height, actual) == expected
    return {"ok": ok, "kind": kind, "orientation_value": value,
            "expected_degrees": degrees, "transform_requested": bool(apply_orientation),
            "orientation_application": observed,
            "width": width, "height": height,
            "source_width": src["width"], "source_height": src["height"],
            "actual_rgba_sha256": actual_sha,
            "stored_rgba_sha256": sha(src["rgba"]),
            "rotated_rgba_sha256": sha(rotated[2]) if rotated else None,
            "raw_equal": raw_equal, "rotated_equal": rotated_equal,
            "icc_expected_sha256": ("sha256:" + sha(src["icc"]["bytes"])) if src.get("icc") else None,
            "icc_declared_length": src["icc"]["declared"] if src.get("icc") else None,
            "icc_truncated": bool(src["icc"].get("truncated")) if src.get("icc") else False,
            "method": src["method"] + "; rotations defined from the stored raster"}
