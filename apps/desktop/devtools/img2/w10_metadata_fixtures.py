"""Deterministic W10 JPEG metadata fixtures with independent segment construction.

The pixel payload is a real baseline 4:2:0 JPEG produced by the existing in-repo
W8 encoder. APP1 EXIF and APP2 ICC segments are constructed here from explicit
fields, so the encoded metadata bytes and their provenance stay reproducible and
no external encoder or colour library is involved.
"""

from __future__ import annotations

import hashlib
import struct

from w5_metadata_fixtures import _profile
from w8_jpeg_fixtures import encode_jpeg

WIDTH = 256
HEIGHT = 192
SAMPLING = "420"
QUANT_SCALE = 1
MAX_SEGMENT = 65533

ICC_PROFILE = _profile()


def sha(data: bytes) -> str:
    return "sha256:" + hashlib.sha256(data).hexdigest()


def _need(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def _segment(marker: int, body: bytes) -> bytes:
    _need(0 < len(body) <= MAX_SEGMENT, "segment_length_invalid")
    return bytes((0xFF, marker)) + struct.pack(">H", len(body) + 2) + body


def exif_app1(orientation: int | None, truncated: bool = False) -> bytes:
    """APP1 EXIF with a single little-endian IFD0 entry for Orientation (0x0112)."""
    entries = 1 if orientation is not None else 0
    tiff = bytearray(b"II" + struct.pack("<H", 42) + struct.pack("<I", 8))
    tiff += struct.pack("<H", entries)
    if orientation is not None:
        _need(1 <= orientation <= 8, "orientation_out_of_range")
        tiff += struct.pack("<HHI", 0x0112, 3, 1) + struct.pack("<HH", orientation, 0)
    tiff += struct.pack("<I", 0)
    body = b"Exif\x00\x00" + bytes(tiff)
    if truncated:
        body = body[:14]
    return _segment(0xE1, body)


def icc_app2(profile: bytes, sequence: int = 1, total: int = 1, truncated: int = 0) -> bytes:
    _need(len(profile) <= MAX_SEGMENT, "icc_profile_too_large")
    payload = b"ICC_PROFILE\x00" + bytes((sequence, total)) + profile
    if truncated:
        payload = payload[:truncated]
    return _segment(0xE2, payload)


def _assemble(segments: list[bytes]) -> bytes:
    base = encode_jpeg(WIDTH, HEIGHT, SAMPLING, QUANT_SCALE)
    _need(base[:2] == b"\xFF\xD8", "base_stream_missing_soi")
    return base[:2] + b"".join(segments) + base[2:]


CASES = (
    {"id": "meta_plain", "kind": "plain", "orientation": None, "icc": None,
     "truncated": None, "reference": "CONTROL", "segments": []},
    {"id": "exif_orient1", "kind": "orientation", "orientation": 1, "icc": None,
     "truncated": None, "reference": "DIFFERENTIAL", "segments": ["exif"]},
    {"id": "exif_orient3", "kind": "orientation", "orientation": 3, "icc": None,
     "truncated": None, "reference": "DIFFERENTIAL", "segments": ["exif"]},
    {"id": "exif_orient6", "kind": "orientation", "orientation": 6, "icc": None,
     "truncated": None, "reference": "DIFFERENTIAL", "segments": ["exif"]},
    {"id": "exif_orient8", "kind": "orientation", "orientation": 8, "icc": None,
     "truncated": None, "reference": "DIFFERENTIAL", "segments": ["exif"]},
    {"id": "icc_valid", "kind": "icc", "orientation": None, "icc": "valid",
     "truncated": None, "reference": "DIFFERENTIAL", "segments": ["icc"]},
    {"id": "exif_truncated", "kind": "malformed", "orientation": 6, "icc": None,
     "truncated": "exif", "reference": "TAMPER", "segments": ["exif_truncated"]},
    {"id": "icc_truncated", "kind": "malformed", "orientation": None, "icc": "valid",
     "truncated": "icc", "reference": "TAMPER", "segments": ["icc_truncated"]},
    {"id": "exif_icc_combined", "kind": "combined", "orientation": 6, "icc": "valid",
     "truncated": None, "reference": "DIFFERENTIAL", "segments": ["exif", "icc"]},
)

CASE_IDS = tuple(case["id"] for case in CASES)

TRANSFORM = {1: "identity", 3: "rotate_180", 6: "rotate_90_cw", 8: "rotate_90_ccw"}
EXPECTED_DIMS = {"none": (WIDTH, HEIGHT), "identity": (WIDTH, HEIGHT),
                 "rotate_180": (WIDTH, HEIGHT),
                 "rotate_90_cw": (HEIGHT, WIDTH), "rotate_90_ccw": (HEIGHT, WIDTH)}


def case_by_id(case_id: str) -> dict:
    for case in CASES:
        if case["id"] == case_id:
            return case
    raise ValueError("unknown_case:" + case_id)


def fixture_bytes(case_id: str) -> bytes:
    case = case_by_id(case_id)
    segments = []
    for name in case["segments"]:
        if name == "exif":
            segments.append(exif_app1(case["orientation"]))
        elif name == "exif_truncated":
            segments.append(exif_app1(case["orientation"], truncated=True))
        elif name == "icc":
            segments.append(icc_app2(ICC_PROFILE))
        elif name == "icc_truncated":
            segments.append(icc_app2(ICC_PROFILE, truncated=40))
        else:
            raise ValueError("unknown_segment:" + name)
    return _assemble(segments)


def expected_orientation(case: dict) -> int | None:
    return case["orientation"] if "exif" in case["segments"] else None


def expected_transform(case: dict) -> str:
    orientation = expected_orientation(case)
    if orientation is None:
        return "none"
    return TRANSFORM[orientation]


def expected_dims(case: dict) -> tuple[int, int]:
    return EXPECTED_DIMS[expected_transform(case)]


def write_fixture(case_id: str, path) -> dict:
    data = fixture_bytes(case_id)
    _need(len(data) <= 4 * 1024 * 1024, "fixture_cap_exceeded")
    path.write_bytes(data)
    case = case_by_id(case_id)
    width, height = expected_dims(case)
    return {"case_id": case_id, "path": path.name, "bytes": len(data), "sha256": sha(data),
            "width": WIDTH, "height": HEIGHT, "expected_output_width": width,
            "expected_output_height": height, "kind": case["kind"],
            "expected_orientation": expected_orientation(case),
            "expected_transform": expected_transform(case),
            "expected_icc": any(name.startswith("icc") for name in case["segments"]),
            "expected_icc_sha256": sha(ICC_PROFILE) if any(name.startswith("icc") for name in case["segments"]) else None,
            "truncated": case["truncated"], "reference": case["reference"]}
