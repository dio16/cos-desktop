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
    _need(isinstance(data, bytes) and 4 <= len(data) <= 4 * 1024 * 1024, "jpeg_input_invalid")
    _need(data[:2] == b"\xff\xd8", "soi_missing")
    out = [{"marker": "SOI", "offset": 0, "length": 2}]
    index = 2
    frame_count = scan_count = eoi_count = 0
    while index < len(data):
        _need(data[index] == 0xFF, "marker_prefix_missing")
        marker_offset = index
        while index < len(data) and data[index] == 0xFF:
            index += 1
        _need(index < len(data), "marker_truncated")
        marker = data[index]
        index += 1
        _need(marker != 0x00, "stuffed_byte_outside_entropy")
        if marker == 0xD9:
            out.append({"marker": "EOI", "offset": marker_offset, "length": index - marker_offset})
            eoi_count += 1
            _need(index == len(data), "jpeg_trailing_bytes")
            break
        _need(marker not in (0xD8, *range(0xD0, 0xD8), 0x01), "standalone_marker_invalid")
        _need(index + 2 <= len(data), "segment_length_truncated")
        length = struct.unpack(">H", data[index:index + 2])[0]
        _need(length >= 2 and index + length <= len(data), "segment_out_of_bounds")
        body = data[index + 2:index + length]
        name = {0xE0: "APP0", 0xE1: "APP1", 0xE2: "APP2", 0xDB: "DQT", 0xC0: "SOF0",
                0xC4: "DHT", 0xDD: "DRI", 0xFE: "COM", 0xDA: "SOS"}.get(marker, "unknown")
        out.append({"marker": name, "offset": marker_offset, "length": length + 2, "body": body})
        index += length
        if marker == 0xC0:
            frame_count += 1
            _need(len(body) >= 6 and body[0] == 8, "sof0_header_invalid")
            component_count = body[5]
            _need(component_count > 0 and len(body) == 6 + 3 * component_count
                  and int.from_bytes(body[1:3], "big") > 0
                  and int.from_bytes(body[3:5], "big") > 0, "sof0_header_invalid")
        if marker != 0xDA:
            continue
        scan_count += 1
        _need(frame_count == 1 and scan_count == 1, "jpeg_scan_order_invalid")
        component_count = body[0] if body else 0
        _need(component_count > 0 and len(body) == 1 + 2 * component_count + 3,
              "sos_header_invalid")
        while index < len(data):
            if data[index] != 0xFF:
                index += 1
                continue
            marker_start = index
            while index < len(data) and data[index] == 0xFF:
                index += 1
            _need(index < len(data), "entropy_marker_truncated")
            entropy_marker = data[index]
            if entropy_marker == 0x00 or 0xD0 <= entropy_marker <= 0xD7:
                index += 1
                continue
            _need(entropy_marker == 0xD9, "multiple_or_unexpected_scan_marker")
            index = marker_start
            break
        _need(index < len(data), "eoi_missing")
    _need(frame_count == 1, "sof0_missing_or_duplicate")
    _need(scan_count == 1, "sos_missing_or_duplicate")
    _need(eoi_count == 1, "eoi_missing")
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
    table_end = ifd + 2 + count * 12 + 4
    _need(table_end <= len(tiff), "exif_ifd_out_of_bounds")
    entries = {}
    type_sizes = {1: 1, 2: 1, 3: 2, 4: 4, 5: 8, 6: 1, 7: 1, 8: 2, 9: 4, 10: 8, 11: 4, 12: 8}
    for position in range(count):
        start = ifd + 2 + position * 12
        tag, kind, number = struct.unpack(endian + "HHI", tiff[start:start + 8])
        _need(tag not in entries, "exif_duplicate_tag")
        _need(kind in type_sizes and number > 0, "exif_entry_type_or_count_invalid")
        byte_count = type_sizes[kind] * number
        value = tiff[start + 8:start + 12]
        if byte_count > 4:
            value_offset = struct.unpack(endian + "I", value)[0]
            _need(value_offset >= table_end and value_offset + byte_count <= len(tiff),
                  "exif_value_offset_invalid")
        entries[tag] = {"type": kind, "count": number, "value": value}
    orientation = None
    if 0x0112 in entries:
        entry = entries[0x0112]
        _need(entry["type"] == 3 and entry["count"] == 1, "exif_orientation_type_invalid")
        orientation = struct.unpack(endian + "H", entry["value"][:2])[0]
        _need(orientation in (1, 3, 6, 8), "exif_orientation_value_invalid")
    return {"byte_order": order.decode(), "entries": sorted(entries), "orientation": orientation}


def parse_icc(body: bytes) -> dict:
    _need(body[:12] == b"ICC_PROFILE\x00", "icc_signature_missing")
    _need(len(body) >= 14, "icc_header_truncated")
    sequence, total = body[12], body[13]
    _need(sequence == total == 1, "icc_sequence_invalid")
    profile = body[14:]
    _need(len(profile) >= 132, "icc_profile_truncated")
    declared = struct.unpack(">I", profile[:4])[0]
    _need(declared == len(profile), "icc_declared_size_mismatch")
    _need(profile[36:40] == b"acsp", "icc_signature_invalid")
    _need(profile[16:20] == b"RGB " and declared == 540, "icc_profile_shape_invalid")
    tag_count = struct.unpack(">I", profile[128:132])[0]
    _need(tag_count == 9 and 132 + 12 * tag_count <= declared, "icc_tag_table_invalid")
    ranges = []
    signatures = set()
    for i in range(tag_count):
        start = 132 + i * 12
        signature = profile[start:start + 4]
        offset, size = struct.unpack(">II", profile[start + 4:start + 12])
        _need(signature not in signatures and size >= 8 and offset >= 132 + 12 * tag_count
              and offset % 4 == 0 and offset + size <= declared, "icc_tag_bounds_invalid")
        signatures.add(signature)
        ranges.append((offset, offset + size))
    _need(signatures == {b"desc", b"cprt", b"wtpt", b"rXYZ", b"gXYZ", b"bXYZ",
                         b"rTRC", b"gTRC", b"bTRC"}, "icc_tag_set_invalid")
    ranges.sort()
    _need(all(ranges[i - 1][1] <= ranges[i][0] for i in range(1, len(ranges))), "icc_tag_overlap")
    return {"sequence": sequence, "total": total, "bytes": len(profile),
            "sha256": sha(profile), "colour_space": "RGB",
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
