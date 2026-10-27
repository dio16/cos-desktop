"""Independent strict BMP565-to-PNG pixel oracle for MP-IMG2-W3."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import struct

from w2_independent_oracle import png_pixels

_BMP_HEADER = struct.Struct("<2sIHHI")
_DIB_HEADER = struct.Struct("<IiiHHIIiiII")
_MASKS = (0xF800, 0x07E0, 0x001F)
_MAX_DIMENSION = 32
_MAX_BYTES = 1024 * 1024


def _need(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def bmp565(data: bytes) -> dict:
    """Parse a tightly specified 16-bit BI_BITFIELDS BMP into logical RGBA."""
    _need(isinstance(data, bytes), "BMP input must be bytes")
    _need(len(data) <= _MAX_BYTES, "BMP exceeds encoded input limit")
    _need(len(data) >= 66, "BMP header or bitfield masks are truncated")
    signature, file_size, reserved1, reserved2, pixel_offset = _BMP_HEADER.unpack_from(data)
    _need(signature == b"BM", "invalid BMP signature")
    _need(reserved1 == 0 and reserved2 == 0, "BMP reserved fields must be zero")
    _need(file_size == len(data), "BMP file size does not match input length")
    dib_size, width, signed_height, planes, depth, compression, image_size, _, _, _, _ = _DIB_HEADER.unpack_from(data, 14)
    _need(dib_size == 40, "BMP DIB header must be 40 bytes")
    _need(1 <= width <= _MAX_DIMENSION, "BMP width outside supported range")
    _need(signed_height != 0 and -_MAX_DIMENSION <= signed_height <= _MAX_DIMENSION,
          "BMP height outside supported range")
    height = abs(signed_height)
    _need(planes == 1, "BMP planes must equal one")
    _need(depth == 16, "BMP must be 16 bits per pixel")
    _need(compression == 3, "BMP compression must be BI_BITFIELDS")
    _need(image_size in (0, (width * 2 + 3) // 4 * 4 * height), "BMP image size is inconsistent")
    _need(pixel_offset == 66, "BMP pixel offset must immediately follow three bitfield masks")
    masks = struct.unpack_from("<III", data, 54)
    _need(masks == _MASKS, "BMP masks are not disjoint RGB565 masks")
    stride = ((width * 2 + 3) // 4) * 4
    logical_row_bytes = width * 2
    logical_source_bytes = logical_row_bytes * height
    _need(stride <= _MAX_BYTES // height, "BMP raster size overflow")
    _need(len(data) == pixel_offset + stride * height, "BMP pixel payload is missing, truncated, or has trailing bytes")
    rgba = bytearray(logical_source_bytes * 2)
    for y in range(height):
        source_y = y if signed_height < 0 else height - 1 - y
        row_start = pixel_offset + source_y * stride
        for x in range(width):
            packed = struct.unpack_from("<H", data, row_start + x * 2)[0]
            r5, g6, b5 = (packed >> 11) & 31, (packed >> 5) & 63, packed & 31
            target = (y * width + x) * 4
            rgba[target:target + 4] = bytes(((r5 << 3) | (r5 >> 2),
                                             (g6 << 2) | (g6 >> 4),
                                             (b5 << 3) | (b5 >> 2), 255))
    return {"width": width, "height": height,
            "row_order": "top_down" if signed_height < 0 else "bottom_up",
            "masks": masks, "encoded_row_stride": stride,
            "logical_row_bytes": logical_row_bytes,
            "logical_source_bytes": logical_source_bytes, "rgba": bytes(rgba)}


def compare(input_data: bytes, output_png: bytes) -> dict:
    """Compare every PNG pixel with independently decoded BMP565 pixels."""
    source = bmp565(input_data)
    _need(isinstance(output_png, bytes), "PNG output must be bytes")
    _need(output_png.endswith(b"\x00\x00\x00\x00IEND\xaeB`\x82"),
          "PNG output is missing its terminal IEND chunk")
    try:
        width, height, actual = png_pixels(output_png)
    except Exception as exc:
        # The shared W2 decoder uses assertions for malformed PNGs. Normalize those
        # and decoder implementation errors into this oracle's documented failure.
        raise ValueError("invalid or unsupported PNG output") from exc
    _need((width, height) == (source["width"], source["height"]), "PNG dimensions do not match BMP")
    expected = source["rgba"]
    mismatches = []
    mismatch_count = 0
    channel_names = ("r", "g", "b", "a")
    for pixel in range(width * height):
        base = pixel * 4
        for channel, name in enumerate(channel_names):
            wanted, got = expected[base + channel], actual[base + channel]
            if wanted != got:
                mismatch_count += 1
                if len(mismatches) < 16:
                    mismatches.append({"x": pixel % width, "y": pixel // width,
                                       "channel": name, "expected": wanted, "actual": got})
    return {"ok": mismatch_count == 0, "width": width, "height": height,
            "expected_rgba_sha256": hashlib.sha256(expected).hexdigest(),
            "actual_rgba_sha256": hashlib.sha256(actual).hexdigest(),
            "mismatches": mismatches, "mismatch_count": mismatch_count,
            "method": "independent BMP RGB565 masks/payload and fixed bit replication; W2 stdlib PNG CRC/inflate/filter reconstruction"}


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", required=True, help="source RGB565 BMP")
    parser.add_argument("--output", required=True, help="decoded PNG")
    parser.add_argument("--out", help="optional JSON report path")
    args = parser.parse_args(argv)
    try:
        report = compare(Path(args.input).read_bytes(), Path(args.output).read_bytes())
    except (OSError, ValueError) as exc:
        report = {"ok": False, "error": str(exc)}
    rendered = json.dumps(report, indent=2) + "\n"
    if args.out:
        Path(args.out).write_text(rendered, encoding="utf-8")
    print(rendered, end="")
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
