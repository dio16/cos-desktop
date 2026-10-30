#!/usr/bin/env python3
"""Measure retained Dock captures; reuse measure_runs' standard-library PNG reader.

Coordinates are image pixels, not desktop coordinates. Equal-colour runs are
not automatically a single stroke: compare row interiors, gaps and the focused
ring's different colour. This script observes no window and proves no HWND.
"""
import hashlib
import json
from pathlib import Path

from measure_runs import decode, runs

BASE = Path(__file__).resolve().parent
COLUMNS = (200, 250, 300, 480, 700, 950, 1200)


def measure(path: Path, rect: dict) -> tuple[dict, str]:
    width, height, rows = decode(path)
    channels = len(rows[0]) // width
    x, y, w, h = (int(rect[k]) for k in ("x", "y", "width", "height"))

    def pixel(xx: int, yy: int) -> tuple:
        return tuple(rows[yy][xx * channels:xx * channels + 3])

    def scan(points: list[tuple[int, int]]) -> dict:
        colours = [pixel(xx, yy) for xx, yy in points]
        return {"at": list(points[0]), "runs_inward": runs(colours)}

    edges = {
        "top": [scan([(cx, y + d) for d in range(6)]) for cx in COLUMNS],
        "bottom": [scan([(cx, y + h - 1 - d) for d in range(6)]) for cx in COLUMNS],
        "left": [scan([(x + d, cy) for d in range(6)]) for cy in (y + 8, y + 20, y + h - 8)],
        "right": [scan([(x + w - 1 - d, cy) for d in range(6)]) for cy in (y + 8, y + 20, y + h - 8)],
    }
    # The interior digest EXCLUDES the outer two pixels on all four sides.
    # Its equality checks the retained viewport/content, not scroll APIs.
    interior = hashlib.sha256()
    for line in rows[y + 2:y + h - 2]:
        interior.update(line[(x + 2) * channels:(x + w - 2) * channels])
    boundary_rows = []
    for yy in (y, y + 1, y + 2, y + h - 3, y + h - 2, y + h - 1):
        at = x
        segments = []
        for colour, length in runs([pixel(xx, yy) for xx in range(x, min(x + w, 1220))]):
            segments.append({"x": at, "length": length, "rgb": colour})
            at += length
        boundary_rows.append({"y": yy, "segments": segments})
    return {
        "image_size": [width, height],
        "rect": rect,
        "edge_scans": edges,
        "boundary_rows": boundary_rows,
        "interior_sha256": interior.hexdigest(),
    }, interior.hexdigest()


def main() -> int:
    legacy, _ = measure(BASE / "screenshot.png", {"x": 24, "y": 1262, "width": 3408, "height": 58})
    states = {}
    geometries = []
    interiors = []
    for state in ("unfocused", "focused", "blurred"):
        record = json.loads((BASE / f"ring-{state}.json").read_text(encoding="utf-8"))
        image = BASE / record["screenshot"]["path"]
        actual = "sha256:" + hashlib.sha256(image.read_bytes()).hexdigest()
        if actual != record["screenshot"]["digest"]:
            raise ValueError(f"capture digest mismatch: {state}")
        dock = next(e for e in record["layout"] if e["name"] == "Candidate Dock")
        geometry = [{k: e[k] for k in ("name", "x", "y", "width", "height")} for e in record["layout"]]
        measured, inner = measure(image, {k: dock[k] for k in ("x", "y", "width", "height")})
        measured["focus"] = record["focused_element"]
        measured["no_candidate_viewed"] = record["no_candidate_viewed"]
        states[state] = measured
        geometries.append(geometry)
        interiors.append(inner)
    print(json.dumps({
        "historical": legacy,
        "states": states,
        "normalized_dock_and_row_geometry_equal": geometries[0] == geometries[1] == geometries[2],
        "interior_pixels_equal": len(set(interiors)) == 1,
    }, indent=2, ensure_ascii=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
