"""Independent JPEG structure parser and DC-only absolute pixel reference.

The reference is derived from the normative baseline semantics (quantization,
Huffman decoding, DC prediction, level shift), never from a decoder's output.
Only constant-block grayscale sources qualify for an absolute reference, because
there the inverse DCT is exactly DC/8 and no AC or rounding ambiguity remains.
"""

from __future__ import annotations

import hashlib

_SOF = {0xC0: "baseline_dct", 0xC1: "extended_sequential", 0xC2: "progressive_dct",
        0xC3: "lossless", 0xC5: "differential_sequential", 0xC6: "differential_progressive",
        0xC7: "differential_lossless"}
_STANDALONE = set(range(0xD0, 0xD8)) | {0x01, 0xD8, 0xD9}


def _need(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def parse(data: bytes) -> dict:
    """Parse the marker structure of a JPEG byte stream."""
    _need(isinstance(data, bytes) and len(data) >= 4, "jpeg_input_invalid")
    _need(data[:2] == b"\xff\xd8", "jpeg_soi_missing")
    offset = 2
    segments = []
    frame = None
    quant = {}
    huffman = {}
    scan = None
    restart_interval = 0
    entropy = b""
    while offset < len(data):
        _need(data[offset] == 0xFF, "jpeg_marker_expected")
        while offset < len(data) and data[offset] == 0xFF:
            offset += 1
        _need(offset < len(data), "jpeg_marker_truncated")
        marker = data[offset]
        offset += 1
        if marker in _STANDALONE:
            segments.append({"marker": marker, "length": 0, "offset": offset - 2})
            if marker == 0xD9:
                break
            continue
        _need(offset + 2 <= len(data), "jpeg_length_truncated")
        length = int.from_bytes(data[offset:offset + 2], "big")
        _need(length >= 2 and offset + length <= len(data), "jpeg_segment_out_of_bounds")
        body = data[offset + 2:offset + length]
        segments.append({"marker": marker, "length": length, "offset": offset - 2})
        offset += length
        if marker in _SOF:
            frame = _parse_sof(marker, body)
        elif marker == 0xDB:
            _parse_dqt(body, quant)
        elif marker == 0xC4:
            _parse_dht(body, huffman)
        elif marker == 0xDD:
            _need(len(body) == 2, "jpeg_dri_invalid")
            restart_interval = int.from_bytes(body, "big")
        elif marker == 0xDA:
            scan = _parse_sos(body)
            start = offset
            index = start
            while index + 1 < len(data):
                if data[index] == 0xFF:
                    nxt = data[index + 1]
                    if nxt == 0x00 or 0xD0 <= nxt <= 0xD7:
                        index += 2
                        continue
                    break
                index += 1
            entropy = data[start:index]
            offset = index
            break
    _need(frame is not None, "jpeg_sof_missing")
    _need(scan is not None, "jpeg_sos_missing")
    assert frame is not None and scan is not None
    return {"frame": frame, "quant": quant, "huffman": huffman, "scan": scan,
            "restart_interval": restart_interval, "entropy": entropy,
            "segments": segments, "bytes": len(data),
            "progressive": frame["variant"] == "progressive_dct",
            "trailing_bytes": len(data) - (offset + 2 if data[offset:offset + 2] == b"\xff\xd9" else offset)}


def _parse_sof(marker: int, body: bytes) -> dict:
    _need(len(body) >= 6, "jpeg_sof_truncated")
    precision, height, width, count = body[0], int.from_bytes(body[1:3], "big"), int.from_bytes(body[3:5], "big"), body[5]
    _need(count >= 1 and len(body) == 6 + 3 * count, "jpeg_sof_component_count_invalid")
    components = []
    for index in range(count):
        base = 6 + 3 * index
        components.append({"id": body[base], "sampling": body[base + 1],
                           "h": body[base + 1] >> 4, "v": body[base + 1] & 0x0F,
                           "quant_table": body[base + 2]})
    _need(width > 0 and height > 0, "jpeg_sof_dimensions_invalid")
    return {"marker": marker, "variant": _SOF[marker], "precision": precision,
            "width": width, "height": height, "components": components,
            "component_count": count}


def _parse_dqt(body: bytes, quant: dict) -> None:
    index = 0
    while index < len(body):
        spec = body[index]
        table = spec & 0x0F
        precision = spec >> 4
        size = 128 if precision else 64
        _need(index + 1 + size <= len(body), "jpeg_dqt_truncated")
        values = list(body[index + 1:index + 1 + size])
        if precision:
            values = [int.from_bytes(bytes(values[i:i + 2]), "big") for i in range(0, size, 2)]
        quant[table] = {"precision": precision, "values": values}
        index += 1 + size


def _parse_dht(body: bytes, huffman: dict) -> None:
    index = 0
    while index < len(body):
        spec = body[index]
        table_class, table_id = spec >> 4, spec & 0x0F
        _need(index + 17 <= len(body), "jpeg_dht_truncated")
        counts = list(body[index + 1:index + 17])
        total = sum(counts)
        _need(index + 17 + total <= len(body), "jpeg_dht_symbols_truncated")
        symbols = list(body[index + 17:index + 17 + total])
        huffman[(table_class, table_id)] = {"counts": counts, "symbols": symbols}
        index += 17 + total


def _parse_sos(body: bytes) -> dict:
    _need(len(body) >= 4, "jpeg_sos_truncated")
    count = body[0]
    _need(len(body) == 4 + 2 * count, "jpeg_sos_component_count_invalid")
    components = []
    for index in range(count):
        base = 1 + 2 * index
        components.append({"id": body[base], "dc": body[base + 1] >> 4, "ac": body[base + 1] & 0x0F})
    return {"components": components, "ss": body[1 + 2 * count],
            "se": body[2 + 2 * count], "ah": body[3 + 2 * count] >> 4,
            "al": body[3 + 2 * count] & 0x0F}


def _codes(table: dict) -> dict:
    counts = table["counts"]
    symbols = table["symbols"]
    code = 0
    index = 0
    mapping = {}
    for length, count in enumerate(counts, 1):
        for _ in range(count):
            mapping[(length, code)] = symbols[index]
            index += 1
            code += 1
        code <<= 1
    return mapping


def _bits(entropy: bytes):
    """Bit reader over stuffed entropy bytes: FF 00 carries FF, then continues."""
    index = 0
    while index < len(entropy):
        byte = entropy[index]
        if byte == 0xFF:
            _need(index + 1 < len(entropy) and entropy[index + 1] == 0x00,
                  "jpeg_entropy_stuffing_invalid")
            index += 1
        for shift in range(7, -1, -1):
            yield (byte >> shift) & 1
        index += 1


class _Reader:
    def __init__(self, entropy: bytes):
        self.bits = list(_bits(entropy))
        self.index = 0

    def bit(self) -> int:
        _need(self.index < len(self.bits), "jpeg_entropy_truncated")
        value = self.bits[self.index]
        self.index += 1
        return value

    def receive(self, count: int) -> int:
        value = 0
        for _ in range(count):
            value = (value << 1) | self.bit()
        return value

    def decode(self, mapping: dict) -> int:
        code = 0
        for length in range(1, 17):
            code = (code << 1) | self.bit()
            if (length, code) in mapping:
                return mapping[(length, code)]
        raise ValueError("jpeg_huffman_code_invalid")


def _extend(value: int, size: int) -> int:
    if size == 0:
        return 0
    return value if value >= (1 << (size - 1)) else value - (1 << size) + 1


def dc_blocks(structure: dict) -> list[int]:
    """Decode the DC coefficient of every block, in scan order."""
    scan = structure["scan"]
    _need(len(scan["components"]) == 1, "dc_reference_requires_single_component")
    component = scan["components"][0]
    table = structure["huffman"].get((0, component["dc"]))
    _need(table is not None, "jpeg_dc_table_missing")
    mapping = _codes(table)
    ac_table = structure["huffman"].get((1, component["ac"]))
    _need(ac_table is not None, "jpeg_ac_table_missing")
    ac_mapping = _codes(ac_table)
    reader = _Reader(structure["entropy"])
    blocks = []
    predictor = 0
    total = ((structure["frame"]["width"] + 7) // 8) * ((structure["frame"]["height"] + 7) // 8)
    for _ in range(total):
        size = reader.decode(mapping)
        _need(size <= 16, "jpeg_dc_category_invalid")
        difference = _extend(reader.receive(size), size)
        predictor += difference
        blocks.append(predictor)
        _skip_ac(reader, ac_mapping)
    return blocks


def _skip_ac(reader: "_Reader", mapping: dict) -> None:
    """Consume one block's AC coefficients; a DC-only source emits EOB at once."""
    while True:
        symbol = reader.decode(mapping)
        if symbol == 0x00:
            return
        run, size = symbol >> 4, symbol & 0x0F
        _need(size > 0, "jpeg_ac_size_zero_invalid")
        reader.receive(size)
        if run == 0 and size == 0:
            return


def absolute_rgba(structure: dict) -> dict:
    """Exact RGBA for a constant-block grayscale source, from parsed tables only."""
    frame = structure["frame"]
    _need(frame["component_count"] == 1, "absolute_reference_requires_one_component")
    _need(frame["variant"] == "baseline_dct", "absolute_reference_requires_baseline")
    component = frame["components"][0]
    _need(component["h"] == 1 and component["v"] == 1, "absolute_reference_requires_unit_sampling")
    quant = structure["quant"].get(component["quant_table"])
    _need(quant is not None, "jpeg_quant_table_missing")
    dc_step = quant["values"][0]
    blocks = dc_blocks(structure)
    width, height = frame["width"], frame["height"]
    _need(len(blocks) == ((width + 7) // 8) * ((height + 7) // 8), "jpeg_block_count_mismatch")
    row_bytes = bytearray()
    digest = hashlib.sha256()
    pixels = []
    for y in range(height):
        row = bytearray(width * 4)
        for x in range(width):
            block = blocks[(y // 8) * ((width + 7) // 8) + (x // 8)]
            # Inverse DCT of a constant block: F(0,0)/8 + 128, quantized DC scaled.
            value = block * dc_step / 8.0 + 128.0
            level = int(round(value))
            level = 0 if level < 0 else (255 if level > 255 else level)
            row[4 * x] = level
            row[4 * x + 1] = level
            row[4 * x + 2] = level
            row[4 * x + 3] = 0xFF
        digest.update(row)
        row_bytes.extend(row)
    for value in {level for level in _levels(blocks, dc_step)}:
        pixels.append(value)
    return {"rgba_sha256": "sha256:" + digest.hexdigest(), "pixels": width * height,
            "width": width, "height": height, "dc_step": dc_step,
            "distinct_levels": sorted(pixels), "blocks": len(blocks)}


def _levels(blocks, dc_step):
    for block in blocks:
        value = block * dc_step / 8.0 + 128.0
        level = int(round(value))
        yield max(0, min(255, level))
