"""Independent multi-scan progressive JPEG structure analysis.

Only facts present in the encoded bytes are reported. A SOF2 marker alone is not
a valid progressive stream; the scan list, spectral selection, successive
approximation, entropy boundaries and EOI are all checked and classified.
"""

from __future__ import annotations

_SOF = {0xC0: "baseline_dct", 0xC1: "extended_sequential", 0xC2: "progressive_dct",
        0xC3: "lossless", 0xC5: "differential_sequential", 0xC6: "differential_progressive",
        0xC7: "differential_lossless"}
_STANDALONE = set(range(0xD0, 0xD8)) | {0x01, 0xD8, 0xD9}


def _need(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def _sof(marker: int, body: bytes) -> dict:
    _need(len(body) >= 6, "jpeg_sof_truncated")
    precision, height, width, count = body[0], int.from_bytes(body[1:3], "big"), int.from_bytes(body[3:5], "big"), body[5]
    _need(count >= 1 and len(body) == 6 + 3 * count, "jpeg_sof_component_count_invalid")
    components = []
    for index in range(count):
        base = 6 + 3 * index
        components.append({"id": body[base], "sampling": body[base + 1],
                           "h": body[base + 1] >> 4, "v": body[base + 1] & 0x0F,
                           "quant_table": body[base + 2]})
    return {"marker": marker, "variant": _SOF[marker], "precision": precision,
            "width": width, "height": height, "component_count": count, "components": components}


def parse_scans(data: bytes) -> dict:
    """Parse SOF/DQT/DHT and every SOS scan of a JPEG byte stream."""
    _need(isinstance(data, bytes) and len(data) >= 4, "jpeg_input_invalid")
    _need(data[:2] == b"\xff\xd8", "jpeg_soi_missing")
    offset = 2
    frame = None
    quant = {}
    huffman = {}
    scans = []
    markers = []
    restart_interval = 0
    while offset < len(data):
        _need(data[offset] == 0xFF, "jpeg_marker_expected")
        while offset < len(data) and data[offset] == 0xFF:
            offset += 1
        _need(offset < len(data), "jpeg_marker_truncated")
        marker = data[offset]
        offset += 1
        if marker in _STANDALONE:
            markers.append(marker)
            if marker == 0xD9:
                _need(offset == len(data), "jpeg_trailing_bytes")
                _need(frame is not None, "jpeg_sof_missing")
                _need(bool(scans), "jpeg_sos_missing")
                return {"frame": frame, "quant": quant, "huffman": huffman, "scans": scans,
                        "restart_interval": restart_interval, "bytes": len(data), "markers": markers}
            continue
        _need(offset + 2 <= len(data), "jpeg_length_truncated")
        length = int.from_bytes(data[offset:offset + 2], "big")
        _need(length >= 2 and offset + length <= len(data), "jpeg_segment_out_of_bounds")
        body = data[offset + 2:offset + length]
        markers.append(marker)
        offset += length
        if marker in _SOF:
            _need(frame is None, "jpeg_multiple_frames")
            frame = _sof(marker, body)
        elif marker == 0xDB:
            index = 0
            while index < len(body):
                spec = body[index]
                precision = spec >> 4
                size = 128 if precision else 64
                _need(index + 1 + size <= len(body), "jpeg_dqt_truncated")
                values = list(body[index + 1:index + 1 + size])
                if precision:
                    values = [int.from_bytes(bytes(values[i:i + 2]), "big") for i in range(0, size, 2)]
                quant[spec & 0x0F] = {"precision": precision, "values": values}
                index += 1 + size
        elif marker == 0xC4:
            index = 0
            while index < len(body):
                spec = body[index]
                _need(index + 17 <= len(body), "jpeg_dht_truncated")
                counts = list(body[index + 1:index + 17])
                total = sum(counts)
                _need(index + 17 + total <= len(body), "jpeg_dht_symbols_truncated")
                huffman[(spec >> 4, spec & 0x0F)] = {
                    "counts": counts, "symbols": list(body[index + 17:index + 17 + total])}
                index += 17 + total
        elif marker == 0xDD:
            _need(len(body) == 2, "jpeg_dri_invalid")
            restart_interval = int.from_bytes(body, "big")
        elif marker == 0xDA:
            _need(frame is not None, "jpeg_sos_before_sof")
            assert frame is not None
            scan = _sos(body, frame)
            start = offset
            index = start
            saw_restart = False
            while index + 1 < len(data):
                if data[index] == 0xFF:
                    nxt = data[index + 1]
                    if nxt == 0x00:
                        index += 2
                        continue
                    if 0xD0 <= nxt <= 0xD7:
                        saw_restart = True
                        index += 2
                        continue
                    break
                index += 1
            scan["entropy_offset"] = start
            scan["entropy_bytes"] = index - start
            scan["restart_markers"] = saw_restart
            _need(scan["entropy_bytes"] > 0, "jpeg_scan_entropy_empty")
            _need(index < len(data), "jpeg_scan_unterminated")
            scans.append(scan)
            offset = index
            continue
    raise ValueError("jpeg_eoi_missing")


def _sos(body: bytes, frame: dict) -> dict:
    _need(len(body) >= 4, "jpeg_sos_truncated")
    count = body[0]
    _need(len(body) == 4 + 2 * count, "jpeg_sos_component_count_invalid")
    components = []
    for index in range(count):
        base = 1 + 2 * index
        selector = body[base + 1]
        components.append({"id": body[base], "dc": selector >> 4, "ac": selector & 0x0F})
    spectral_start = body[1 + 2 * count]
    spectral_end = body[2 + 2 * count]
    approximation = body[3 + 2 * count]
    ah, al = approximation >> 4, approximation & 0x0F
    _need(progressive_constraints(frame, spectral_start, spectral_end, ah, al), "jpeg_scan_parameters_invalid")
    ids = {component["id"] for component in frame["components"]}
    for component in components:
        _need(component["id"] in ids, "jpeg_scan_component_unknown")
    return {"components": components, "ss": spectral_start, "se": spectral_end,
            "ah": ah, "al": al, "frame_variant": frame["variant"]}


def progressive_constraints(frame: dict, ss: int, se: int, ah: int, al: int) -> bool:
    if not (0 <= ss <= 63 and 0 <= se <= 63 and ss <= se and ah <= 13 and al <= 13):
        return False
    if frame["variant"] == "progressive_dct":
        if ss == 0:
            return se == 0          # a DC scan covers DC only
        return ah == 0 or al == 0   # AC scans are initial or refinement
    return ss == 0 and se == 63 and ah == 0 and al == 0


def classify(structure: dict) -> dict:
    """Classify structure validity without claiming entropy correctness."""
    scans = structure["scans"]
    progressive = structure["frame"]["variant"] == "progressive_dct"
    dc_scans = [scan for scan in scans if scan["ss"] == 0 and scan["se"] == 0]
    ac_scans = [scan for scan in scans if scan["ss"] > 0]
    refinement = [scan for scan in scans if scan["ah"] > 0]
    result = {"sof2": progressive, "scans": len(scans), "dc_scans": len(dc_scans),
              "ac_scans": len(ac_scans), "refinement_scans": len(refinement),
              "restart_interval": structure["restart_interval"],
              "entropy_bytes": sum(scan["entropy_bytes"] for scan in scans),
              "entropy_verified": "NOT_VERIFIED",
              "structure": "VALID_PROGRESSIVE_STRUCTURE" if progressive and dc_scans and ac_scans
                           else ("BASELINE_CONTROL" if not progressive else "STRUCTURE_INVALID")}
    return result
