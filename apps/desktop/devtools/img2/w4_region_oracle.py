"""Independent W4 all-pixel oracle for a rectangular WIC output.

Input expectations come from the deterministic source fixture bytes: PNG through
the W2 stdlib CRC/inflate/filter decoder, 24-bit BMP through its own row parser,
and RGB565 BMP through the retained W3 bitfield oracle. WIC output is never used
to derive an expectation.
"""
from __future__ import annotations

import hashlib
import struct

from w2_independent_oracle import png_pixels
from w3_oracle import bmp565


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def bmp24(data: bytes) -> dict:
    assert len(data) <= 1024 * 1024 and data[:2] == b"BM"
    file_size, _, _, offset = struct.unpack_from("<IHHI", data, 2)
    assert file_size == len(data) and offset == 54
    dib, width, signed_height, planes, depth, compression = struct.unpack_from("<IiiHHI", data, 14)
    height = abs(signed_height)
    assert dib == 40 and 0 < width <= 1024 and 0 < height <= 1024
    assert planes == 1 and depth == 24 and compression == 0 and signed_height != 0
    stride = (width * 3 + 3) & ~3
    assert len(data) == offset + stride * height
    rgba = bytearray()
    for y in range(height):
        source_y = y if signed_height < 0 else height - 1 - y
        row = offset + source_y * stride
        for x in range(width):
            b, g, r = data[row + x * 3:row + x * 3 + 3]
            rgba.extend((r, g, b, 255))
    return {"width": width, "height": height, "rgba": bytes(rgba),
            "row_order": "top_down" if signed_height < 0 else "bottom_up",
            "encoded_row_stride": stride, "method": "independent 24-bit BMP rows, BGR to RGBA"}


def bmp555(data: bytes) -> dict:
    assert len(data) <= 1024 * 1024 and data[:2] == b"BM"
    file_size, _, _, offset = struct.unpack_from("<IHHI", data, 2)
    assert file_size == len(data) and offset == 66
    dib, width, signed_height, planes, depth, compression = struct.unpack_from("<IiiHHI", data, 14)
    height = abs(signed_height)
    masks = struct.unpack_from("<III", data, 54)
    assert dib == 40 and 0 < width <= 1024 and 0 < height <= 1024
    assert planes == 1 and depth == 16 and compression == 3 and signed_height != 0
    assert masks == (0x7C00, 0x03E0, 0x001F), "BMP masks are not the Bgr555 control"
    stride = ((width * 2 + 3) // 4) * 4
    assert len(data) == offset + stride * height
    rgba = bytearray()
    for y in range(height):
        source_y = y if signed_height < 0 else height - 1 - y
        row = offset + source_y * stride
        for x in range(width):
            packed = struct.unpack_from("<H", data, row + x * 2)[0]
            r5, g5, b5 = (packed >> 10) & 31, (packed >> 5) & 31, packed & 31
            rgba.extend(((r5 << 3) | (r5 >> 2), (g5 << 3) | (g5 >> 2),
                         (b5 << 3) | (b5 >> 2), 255))
    return {"width": width, "height": height, "rgba": bytes(rgba),
            "row_order": "top_down" if signed_height < 0 else "bottom_up",
            "encoded_row_stride": stride,
            "method": "independent 5/5/5 BMP control masks/payload and fixed bit replication"}


def source_rgba(fixture: dict, data: bytes) -> dict:
    cid = fixture["id"]
    if cid.startswith("png"):
        width, height, rgba = png_pixels(data)
        return {"width": width, "height": height, "rgba": rgba,
                "method": "W2 stdlib PNG CRC/inflate/filter reconstruction"}
    if cid == "bmp24":
        return bmp24(data)
    if cid == "bmp555_control":
        return bmp555(data)
    if cid in ("bmp565", "bmp565_odd"):
        result = bmp565(data)
        return {"width": result["width"], "height": result["height"], "rgba": result["rgba"],
                "method": result["method"] if "method" in result else
                "independent BMP RGB565 masks/payload and fixed bit replication"}
    raise ValueError("fixture has no independent RGBA oracle")


def slice_rgba(source: dict, x: int, y: int, width: int, height: int) -> bytes:
    assert 0 <= x and 0 <= y and 0 < width and 0 < height
    assert x + width <= source["width"] and y + height <= source["height"]
    out = bytearray()
    for row in range(y, y + height):
        start = (row * source["width"] + x) * 4
        out.extend(source["rgba"][start:start + width * 4])
    return bytes(out)


def compare_roi(fixture: dict, input_data: bytes, output_png: bytes,
                x: int, y: int, width: int, height: int) -> dict:
    source = source_rgba(fixture, input_data)
    expected = slice_rgba(source, x, y, width, height)
    out_width, out_height, actual = png_pixels(output_png)
    assert (out_width, out_height) == (width, height), "ROI output dimensions differ"
    assert len(actual) == len(expected)
    mismatches = []
    for pixel in range(width * height):
        for channel, name in enumerate(("r", "g", "b", "a")):
            base = pixel * 4 + channel
            if expected[base] != actual[base]:
                if len(mismatches) < 16:
                    mismatches.append({"x": pixel % width, "y": pixel // width,
                                       "channel": name, "expected": expected[base],
                                       "actual": actual[base]})
    mismatch_count = sum(1 for a, b in zip(expected, actual) if a != b)
    return {
        "ok": mismatch_count == 0 and (out_width, out_height) == (width, height),
        "width": width, "height": height, "x": x, "y": y,
        "expected_rgba_sha256": sha(expected),
        "actual_rgba_sha256": sha(actual),
        "mismatch_count": mismatch_count, "mismatches": mismatches,
        "source_width": source["width"], "source_height": source["height"],
        "method": source["method"] + "; ROI rows sliced from the independent source raster",
    }
