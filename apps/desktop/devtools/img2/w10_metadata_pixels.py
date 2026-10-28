"""W10 raster evidence: strict PNG reconstruction and independent EXIF rotations.

Relations to a decoded control are differential, not an absolute JPEG oracle.
The rotation mapping is independent of the Windows transform implementation.
"""
from __future__ import annotations

import struct

from w2_independent_oracle import png_pixels
from w2_resource_verify import sha
from w5_metadata_oracle import rotate


def raster(data: bytes) -> tuple[int, int, bytes]:
    if not isinstance(data, bytes) or not 33 <= len(data) <= 1024 * 1024:
        raise ValueError("output_png_byte_bound")
    if data[:8] != b"\x89PNG\r\n\x1a\n" or data[12:16] != b"IHDR":
        raise ValueError("output_png_header")
    width, height = struct.unpack_from(">II", data, 16)
    if not (0 < width <= 256 and 0 < height <= 256):
        raise ValueError("output_png_geometry_bound")
    # Reject duplicate headers and require a complete chunk stream before reusing
    # the bounded existing stdlib RGBA reconstruction.
    offset, headers, ended = 8, 0, False
    while offset < len(data):
        if offset + 12 > len(data):
            raise ValueError("output_png_chunk_truncated")
        length = int.from_bytes(data[offset:offset + 4], "big")
        tag = data[offset + 4:offset + 8]
        if offset + 12 + length > len(data):
            raise ValueError("output_png_chunk_length")
        headers += tag == b"IHDR"
        offset += 12 + length
        if tag == b"IEND":
            if length != 0 or offset != len(data):
                raise ValueError("output_png_end")
            ended = True
            break
    if headers != 1 or not ended:
        raise ValueError("output_png_incomplete")
    return png_pixels(data)


def output_record(data: bytes | None) -> dict:
    if data is None:
        return {"present": False, "bytes": 0, "sha256": None, "hex": None,
                "width": None, "height": None, "rgba_sha256": None}
    width, height, rgba = raster(data)
    return {"present": True, "bytes": len(data), "sha256": sha(data), "hex": data.hex(),
            "width": width, "height": height, "rgba_sha256": sha(rgba)}


def difference(left, right) -> dict:
    lw, lh, a = left
    rw, rh, b = right
    result = {"left_width": lw, "left_height": lh, "right_width": rw, "right_height": rh,
              "left_rgba_sha256": sha(a), "right_rgba_sha256": sha(b),
              "geometry_equal": (lw, lh) == (rw, rh), "exact": False,
              "differing_pixels": None, "maximum_channel_difference": None}
    if (lw, lh) != (rw, rh):
        return result
    result["differing_pixels"] = sum(a[i:i + 4] != b[i:i + 4] for i in range(0, len(a), 4))
    result["maximum_channel_difference"] = max((abs(x - y) for x, y in zip(a, b)), default=0)
    result["exact"] = a == b
    return result


def comparisons(cases: list[dict]) -> dict:
    decoded = {}
    for row in cases:
        for leg in row["legs"]:
            output = leg["output"]
            if output["present"]:
                decoded[row["case_id"], leg["leg"]] = raster(bytes.fromhex(output["hex"]))
    result = {}
    for row in cases:
        cid = row["case_id"]
        observations = {"decoder_differential": None, "control_relations": {},
                        "orientation_relation": None,
                        "icc_colour_conversion": "NOT_MEASURED",
                        "jpeg_absolute_correctness": "NOT_VERIFIED"}
        wic, gdi = decoded.get((cid, "wic_metadata")), decoded.get((cid, "gdiplus_metadata"))
        if wic is not None and gdi is not None:
            observations["decoder_differential"] = difference(wic, gdi)
        for name in ("wic_metadata", "gdiplus_metadata"):
            actual, control = decoded.get((cid, name)), decoded.get(("meta_plain", name))
            if actual is not None and control is not None:
                observations["control_relations"][name] = difference(control, actual)
        transformed = decoded.get((cid, "wic_transform"))
        if wic is not None and transformed is not None:
            orientation = row["structure"]["exif"]["orientation"]
            degrees = {1: 0, 3: 180, 6: 90, 8: 270}[orientation]
            rotated = rotate(wic[2], wic[0], wic[1], degrees)
            observations["orientation_relation"] = {"degrees": degrees,
                "reference": "independent index rotation of the untransformed decoded raster",
                **difference(rotated, transformed)}
        result[cid] = observations
    return result
