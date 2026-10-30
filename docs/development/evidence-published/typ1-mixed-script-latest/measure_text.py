#!/usr/bin/env python3
"""Measure this retained capture's ink footprints, not font metrics or glyph coverage.

The existing PNG reader is reused only after matching the accepted image digest.
UIA rectangles locate fields; bright-pixel bands are not baselines or line boxes.
Exact-pixel crops are written only so a reviewer can see the retained ink.
No OCR, glyph identity, tofu detector, font resolution or general image API.
"""
import hashlib
import importlib.util
import json
import pathlib
import struct
import zlib

BASE = pathlib.Path(__file__).resolve().parent


def main() -> int:
    native = json.loads((BASE / "native.json").read_text(encoding="utf-8"))
    image = BASE / "screenshot.png"
    if "sha256:" + hashlib.sha256(image.read_bytes()).hexdigest() != native["screenshot"]["digest"]:
        raise ValueError("capture digest mismatch")
    spec = importlib.util.spec_from_file_location(
        "retained_png", BASE.parent / "typ1-dpi-ladder-latest" / "measure_runs.py"
    )
    if spec is None or spec.loader is None:
        raise ValueError("retained PNG reader unavailable")
    reader = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(reader)
    width, height, rows = reader.decode(image)
    if (width, height) != (1296, 839):
        raise ValueError("capture geometry changed")
    channels = len(rows[0]) // width
    targets = {
        "context_title": [67, 43, 1033, 20],
        "context_latin_ink_sample": [67, 43, 58, 20],
        "context_japanese_ink_sample": [130, 43, 118, 20],
        "task_rail_item": [24, 403, 208, 106],
        "inspector_goal": [984, 389, 288, 50],
        "task_status": [28, 67, 403, 40],
        "iteration": [865, 67, 403, 40],
        "artifact_availability": [28, 115, 403, 40],
        "dock_candidate_a": [26, 695, 220, 54],
        "dock_candidate_b": [258, 695, 220, 54],
    }
    result = {
        "image_sha256": native["screenshot"]["digest"],
        "coordinate_space": "image pixels, half-open rectangles; not desktop coordinates",
        "method": "min(R,G,B) >= 160; rows separated by at least four empty rows form separate bands",
        "limits": "Thresholded bright ink only: faint antialias pixels excluded; bands are not font metrics, baselines, complete glyph bounds, clipping or tofu proof.",
        "targets": {},
    }
    for name, rect in targets.items():
        x, y, w, h = rect
        pixels = [
            (px, py)
            for py in range(y, y + h)
            for px in range(x, x + w)
            if min(rows[py][px * channels:px * channels + 3]) >= 160
        ]
        occupied = sorted({py for _, py in pixels})
        bands = []
        for py in occupied:
            if not bands or py - bands[-1][1] >= 4:
                bands.append([py, py + 1])
            else:
                bands[-1][1] = py + 1
        result["targets"][name] = {
            "rect": rect,
            "bright_ink_bbox": [min(px for px, _ in pixels), min(py for _, py in pixels),
                                max(px for px, _ in pixels) + 1, max(py for _, py in pixels) + 1],
            "bright_ink_y_bands": bands,
        }
    write_crop(rows, channels, (67, 40, 190, 26), BASE / "context-title.png")
    write_crop(rows, channels, (24, 403, 208, 106), BASE / "task-rail-item.png")
    write_crop(rows, channels, (984, 389, 288, 50), BASE / "inspector-goal.png")
    print(json.dumps(result, indent=2))
    return 0


def write_crop(rows: list[bytes], channels: int, rect: tuple[int, int, int, int],
               path: pathlib.Path) -> None:
    """Write the exact retained pixels of one rectangle as an 8-bit RGB PNG."""
    x, y, w, h = rect
    raw = b"".join(
        b"\x00" + b"".join(rows[py][px * channels:px * channels + 3] for px in range(x, x + w))
        for py in range(y, y + h)
    )

    def chunk(kind: bytes, payload: bytes) -> bytes:
        return (struct.pack(">I", len(payload)) + kind + payload
                + struct.pack(">I", zlib.crc32(kind + payload) & 0xFFFFFFFF))

    path.write_bytes(b"\x89PNG\r\n\x1a\n"
                     + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
                     + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


if __name__ == "__main__":
    raise SystemExit(main())
