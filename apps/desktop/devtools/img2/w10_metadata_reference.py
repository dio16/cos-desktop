"""Independent W10 metadata parser: segment walk, EXIF IFD0 and ICC header checks.

This module never decodes pixels. It reads the encoded bytes only, so the
expectations it produces stay independent of every decoder under test.
"""

from __future__ import annotations

import hashlib
import struct

ALLOWED_MARKERS = {"SOI", "APP0", "APP1", "APP2", "DQT", "SOF0", "DHT", "DRI", "COM", "SOS", "EOI"}


def sha(data: bytes) -> str:
    return "sha256:" + hashlib.sha256(data).hexdigest()


def _need(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def segments(data: bytes) -> list[dict]:
    _need(data[:2] == b"\xff\xd8", "soi_missing")
    out = [{"marker": "SOI", "offset": 0, "length": 2}]
    index = 2
    while index < len(data):
        _need(data[index] == 0xFF, "marker_prefix_missing")
        marker = data[index + 1]
        if marker == 0xD9:
            out.append({"marker": "EOI", "offset": index, "length": 2})
            index += 2
            break
        if marker == 0xDA:
            length = struct.unpack(">H", data[index + 2:index + 4])[0]
            out.append({"marker": "SOS", "offset": index, "length": length + 2,
                        "body": data[index + 4:index + 2 + length]})
            index += 2 + length
            break
        length = struct.unpack(">H", data[index + 2:index + 4])[0]
        _need(length >= 2, "segment_length_invalid")
        name = {0xE0: "APP0", 0xE1: "APP1", 0xE2: "APP2", 0xDB: "DQT", 0xC0: "SOF0",
                0xC4: "DHT", 0xC2: "SOF2", 0xDD: "DRI", 0xFE: "COM"}.get(marker, "unknown")
        out.append({"marker": name, "offset": index, "length": length + 2,
                    "body": data[index + 4:index + 2 + length]})
        index += 2 + length
    return out


def parse_exif(body: bytes) -> dict:
    _need(body[:6] == b"Exif\x00\x00", "exif_signature_missing")
    tiff = body[6:]
    _need(len(tiff) >= 8, "exif_tiff_truncated")
    order = tiff[:2]
    _need(order in (b"II", b"MM"), "exif_byte_order_invalid")
    endian = "<" if order == b"II" else ">"
    _need(struct.unpack(endian + "H", tiff[2:4])[0] == 42, "exif_tiff_magic_invalid")
    ifd = struct.unpack(endian + "I", tiff[4:8])[0]
    _need(8 <= ifd <= len(tiff) - 2, "exif_ifd_offset_invalid")
    count = struct.unpack(endian + "H", tiff[ifd:ifd + 2])[0]
    _need(count <= 64, "exif_entry_count_invalid")
    entries = {}
    base = ifd + 2
    for position in range(count):
        start = base + position * 12
        _need(start + 12 <= len(tiff), "exif_entry_truncated")
        tag, kind, number = struct.unpack(endian + "HHI", tiff[start:start + 8])
        value = tiff[start + 8:start + 12]
        entries[tag] = {"type": kind, "count": number, "value": value}
    orientation = None
    if 0x0112 in entries:
        entry = entries[0x0112]
        _need(entry["type"] == 3 and entry["count"] == 1, "exif_orientation_type_invalid")
        orientation = struct.unpack(endian + "H", entry["value"][:2])[0]
    return {"byte_order": order.decode(), "entries": sorted(entries), "orientation": orientation}


def parse_icc(body: bytes) -> dict:
    _need(body[:12] == b"ICC_PROFILE\x00", "icc_signature_missing")
    _need(len(body) >= 14, "icc_header_truncated")
    sequence, total = body[12], body[13]
    _need(1 <= sequence <= total, "icc_sequence_invalid")
    profile = body[14:]
    _need(len(profile) >= 132, "icc_profile_truncated")
    declared = struct.unpack(">I", profile[:4])[0]
    _need(declared == len(profile), "icc_declared_size_mismatch")
    _need(profile[36:40] == b"acsp", "icc_signature_invalid")
    _need(profile[16:20] in (b"RGB ", b"CMYK", b"GRAY"), "icc_colour_space_invalid")
    return {"sequence": sequence, "total": total, "bytes": len(profile),
            "sha256": sha(profile), "colour_space": profile[16:20].decode("ascii").strip(),
            "version": f"{profile[8]}.{profile[9] >> 4}"}


def classify(structure: dict) -> str:
    if structure.get("exif_error") or structure.get("icc_error"):
        return "METADATA_TAMPERED"
    exif = structure.get("exif")
    icc = structure.get("icc")
    if exif is None and icc is None:
        return "METADATA_ABSENT"
    orientation = None if exif is None else exif["orientation"]
    if orientation in (None, 1, 3, 6, 8) and (icc is None or icc["bytes"] > 0):
        return "METADATA_PARSED"
    return "METADATA_UNVERIFIED"


def parse(data: bytes) -> dict:
    rows = segments(data)
    names = [row["marker"] for row in rows]
    unknown = sorted(set(names) - ALLOWED_MARKERS)
    exif = icc = None
    exif_error = icc_error = None
    icc_partial = None
    app1 = [row for row in rows if row["marker"] == "APP1"]
    app2 = [row for row in rows if row["marker"] == "APP2"]
    if len(app1) > 1:
        raise ValueError("duplicate_app1_segment")
    if len(app2) > 1:
        raise ValueError("duplicate_app2_segment")
    if app1:
        try:
            exif = parse_exif(app1[0]["body"])
        except (ValueError, KeyError, IndexError, struct.error) as exc:
            exif_error = type(exc).__name__ + ":" + str(exc)[:80]
    if app2:
        body = app2[0]["body"]
        if body[:12] == b"ICC_PROFILE\x00" and len(body) > 14:
            icc_partial = {"bytes": len(body) - 14, "payload_sha256": sha(body[14:]),
                           "declared_size": int.from_bytes(body[14:18], "big") if len(body) >= 18 else None}
        try:
            icc = parse_icc(body)
        except (ValueError, KeyError, IndexError, struct.error) as exc:
            icc_error = type(exc).__name__ + ":" + str(exc)[:80]
    structure = {"markers": names, "unknown_markers": unknown, "bytes": len(data),
                 "exif": exif, "exif_error": exif_error, "icc": icc, "icc_error": icc_error,
                 "icc_partial": icc_partial}
    structure["classification"] = classify(structure)
    return structure
