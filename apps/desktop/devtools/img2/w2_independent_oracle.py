"""Independent stdlib oracle for the retained MP-IMG2-W2 records.

Reconstructs fixture and decoded-output rasters from the tracked archive with
CRC/inflate/filter code of its own and compares them with the declared solid
colours. Imports nothing from the runner or the verifier, so agreement is not a
copy of the producer's own arithmetic.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import struct
import zipfile
import zlib

COLORS = {"small_png": (17, 34, 51), "small_bmp": (17, 34, 51), "preview_png": (17, 34, 51),
          "preview_png_2": (5, 19, 37), "detail_png": (17, 34, 51)}
PNG_FIXTURES = ("small_png", "preview_png", "preview_png_2", "detail_png")
LIMITS = ("max_cases", "max_workers", "max_worker_seconds", "max_total_seconds", "max_output_bytes",
          "max_work_bytes", "max_trace_records", "max_trace_bytes", "max_samples",
          "max_capture_bytes", "max_report_bytes", "max_observed_private_bytes")
KNOWN = ("encoded_input_bytes", "admitted_decoded_bytes", "bgra32_destination_bytes",
         "declared_scratch_bytes", "explicit_known_requirements_bytes")


def sha(data: bytes) -> str:
    return "sha256:" + hashlib.sha256(data).hexdigest()


def png_pixels(data: bytes):
    """Strict PNG decode: CRCs, one IDAT stream, no interlacing, RGBA rows."""
    assert len(data) <= 16 * 1024 * 1024 and data[:8] == b"\x89PNG\r\n\x1a\n"
    offset, payload, header = 8, [], None
    while offset < len(data):
        length = struct.unpack(">I", data[offset:offset + 4])[0]
        tag = data[offset + 4:offset + 8]
        body = data[offset + 8:offset + 8 + length]
        crc = struct.unpack(">I", data[offset + 8 + length:offset + 12 + length])[0]
        assert zlib.crc32(tag + body) & 0xFFFFFFFF == crc
        offset += 12 + length
        if tag == b"IHDR":
            header = struct.unpack(">IIBBBBB", body)
        if tag == b"IDAT":
            payload.append(body)
        if tag == b"IEND":
            break
    assert offset == len(data) and header is not None
    width, height, depth, color, compression, filter_method, interlace = header
    assert 0 < width <= 1024 and 0 < height <= 1024 and depth == 8 and color in (2, 6)
    assert (compression, filter_method, interlace) == (0, 0, 0)
    bpp = 3 if color == 2 else 4
    stride = width * bpp
    expected = (stride + 1) * height
    stream = zlib.decompressobj()
    raw = stream.decompress(b"".join(payload), expected + 1)
    assert len(raw) == expected and stream.eof and not stream.unused_data and not stream.unconsumed_tail
    previous = bytearray(stride)
    rgba = bytearray()
    for y in range(height):
        start = y * (stride + 1)
        kind = raw[start]
        row = bytearray(raw[start + 1:start + 1 + stride])
        assert 0 <= kind <= 4
        for x in range(stride):
            left = row[x - bpp] if x >= bpp else 0
            above = previous[x]
            upper_left = previous[x - bpp] if x >= bpp else 0
            if kind == 1:
                predictor = left
            elif kind == 2:
                predictor = above
            elif kind == 3:
                predictor = (left + above) // 2
            elif kind == 4:
                p = left + above - upper_left
                distances = (abs(p - left), abs(p - above), abs(p - upper_left))
                predictor = (left, above, upper_left)[distances.index(min(distances))]
            else:
                predictor = 0
            row[x] = (row[x] + predictor) & 255
        for x in range(0, stride, bpp):
            rgba.extend(row[x:x + 3])
            rgba.append(row[x + 3] if bpp == 4 else 255)
        previous = row
    return width, height, bytes(rgba)


def structural(record):
    """Fields that must reproduce across two independent runs."""
    return {"fixtures": [(f["case_id"], f["bytes"], f["sha256"]) for f in record["fixtures"]],
            "limits": {k: record["bounds"][k] for k in LIMITS},
            "decode_limits": record["decode_limits"]["limits"],
            "consumer": [{k: row[k] for k in
                          ("case_id", "observed_outcome", "validity", "pixel_digest", "pixel_verified")}
                         for row in record["consumer"]["record"]["observations"]],
            "known_requirements": {k: record["memory"][k]["value"] for k in KNOWN}}


def run_report(record, label, archive, manifest):
    decoded = []
    brackets = []
    dims = {f["case_id"]: (f["width"], f["height"]) for f in record["fixtures"]}
    for worker in record["workers"]:
        stage = {s["stage"]: s for s in worker["stages"]}
        for name in ("onload", "conversion", "copy_pixels", "encode"):
            if name + "_entered" in stage and name + "_returned" in stage:
                first, last = stage[name + "_entered"], stage[name + "_returned"]
                assert first["qpc_frequency"] == last["qpc_frequency"]
                assert first["qpc_ticks"] <= last["qpc_ticks"]
                brackets.append({"case_id": worker["case_id"], "stage": name,
                                 "bracketed_ns": (last["qpc_ticks"] - first["qpc_ticks"])
                                 * 1_000_000_000 // first["qpc_frequency"]})
        if worker["outcome"] != "decoded" or worker["fixture_id"] == "small_jpeg":
            continue
        name = label + "/" + worker["output_path"].replace("\\", "/")
        assert name in manifest, name
        data = archive.read(name)
        assert len(data) == worker["output_bytes"] and sha(data) == worker["output_sha256"]
        width, height, rgba = png_pixels(data)
        color = COLORS[worker["fixture_id"]]
        assert (width, height) == dims[worker["fixture_id"]]
        assert rgba == bytes((*color, 255)) * (width * height)
        decoded.append({"case_id": worker["case_id"], "width": width, "height": height,
                        "rgba_sha256": hashlib.sha256(rgba).hexdigest()})
    cohort = next(c for c in record["cohorts"] if len(c["case_ids"]) == 3)
    workers = {w["case_id"]: w for w in record["workers"]}
    for sample in cohort["samples"]:
        worker = workers[sample["case_id"]]
        assert sample["pid"] == worker["pid"] and sample["start_utc_ticks"] == worker["start_utc_ticks"]
        assert worker["started_ns"] <= cohort["start_ns"] <= cohort["end_ns"] <= worker["exited_ns"]
        assert next(s for s in worker["stages"] if s["stage"] == "after_onload")["artificial_instrumentation_hold"]
    assert cohort["totals"] == {k: sum(s[k] for s in cohort["samples"])
                                for k in ("working_set_bytes", "private_bytes")}
    return {"record": label, "decoded_worker_rasters": decoded, "bracketed_stage_durations": brackets,
            "cohort": {"case_ids": cohort["case_ids"], "totals": cohort["totals"],
                       "atomic_snapshot": False, "status": "derived_window_sum",
                       "instantaneous_peak": "NOT_MEASURED"}}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--matrix", required=True)
    parser.add_argument("--repeat", required=True)
    parser.add_argument("--rasters", required=True)
    parser.add_argument("--out", required=True)
    args = parser.parse_args(argv)
    records = [(label, json.loads(Path(path).read_text(encoding="utf-8")))
               for label, path in (("img2-w2-run1", args.matrix), ("img2-w2-run2", args.repeat))]
    assert structural(records[0][1]) == structural(records[1][1])
    results = []
    with zipfile.ZipFile(args.rasters) as archive:
        manifest = json.loads(archive.read("raster-manifest.json"))["files"]
        index = {item["path"]: item for item in manifest}
        for label, record in records:
            for fixture in record["fixtures"]:
                name = label + "/cases/" + fixture["path"]
                data = archive.read(name)
                assert len(data) == fixture["bytes"] and sha(data) == fixture["sha256"]
                assert index[name]["sha256"] == fixture["sha256"]
                if fixture["case_id"] in PNG_FIXTURES:
                    width, height, rgba = png_pixels(data)
                    color = COLORS[fixture["case_id"]]
                    assert (width, height) == (fixture["width"], fixture["height"])
                    assert rgba == bytes((*color, 255)) * (width * height)
            results.append(run_report(record, label, archive, index))
    report = {"schema_version": 1, "ok": True,
              "method": "independent stdlib PNG CRC/inflate/filter reconstruction and known solid "
                        "RGBA over the tracked raster archive; no runner/verifier import",
              "stage_duration_meaning": "same-process QPC brackets include trace-entry flush and call "
                                        "overhead; explicit boundary holds excluded from paired stages, "
                                        "not a WIC-only cost or SLO",
              "jpeg": "structure only, no absolute raster expectation",
              "structural_repeats_identical": True,
              "nondeterministic_fields_excluded": ["process lifetimes", "timestamps", "OS counters",
                                                   "inside-OnLoad attempts"],
              "runs": results}
    data = json.dumps(report, indent=2, sort_keys=False) + "\n"
    Path(args.out).write_text(data, encoding="utf-8")
    print(json.dumps({"ok": True, "runs": len(results),
                      "decoded_worker_rasters": [len(x["decoded_worker_rasters"]) for x in results],
                      "structural_repeats_identical": True, "sha256": sha(data.encode("utf-8"))}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
