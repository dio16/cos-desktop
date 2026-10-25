"""MP-IMG2-W1 real-WIC admission matrix. Generates bounded fixtures, runs the
probe directly for exact budget boundaries and the comparison harness for the
end-to-end pixel oracle. Nothing here is product behaviour or a measurement of
WIC-internal memory.
"""
from __future__ import annotations

import hashlib
import json
import struct
import subprocess
import sys
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[4]
PROBE = ROOT / "apps/desktop/examples/decode_compare/wic_probe.ps1"
HARNESS = ROOT / "target/debug/examples/decode_compare.exe"
PWSH = "pwsh"
WORK = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(
    "C:/dev/cos-desktop-verification/img2-w1-work")
CASES = WORK / "cases"
RESULTS = WORK / "results"
SOLID = (17, 34, 51)


def sha(data: bytes) -> str:
    return "sha256:" + hashlib.sha256(data).hexdigest()


def png(width: int, height: int, colour: tuple[int, int, int]) -> bytes:
    """Minimal 8-bit RGB PNG: one IDAT, filter 0 rows."""
    def chunk(kind: bytes, payload: bytes) -> bytes:
        return (struct.pack(">I", len(payload)) + kind + payload
                + struct.pack(">I", zlib.crc32(kind + payload) & 0xFFFFFFFF))
    rows = b"".join(b"\x00" + bytes(colour) * width for _ in range(height))
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr)
            + chunk(b"IDAT", zlib.compress(rows, 9)) + chunk(b"IEND", b""))


def truncated_png() -> bytes:
    """A declared-100000x100000 PNG whose IDAT is only an empty zlib stream."""
    def chunk(kind: bytes, payload: bytes) -> bytes:
        return (struct.pack(">I", len(payload)) + kind + payload
                + struct.pack(">I", zlib.crc32(kind + payload) & 0xFFFFFFFF))
    ihdr = struct.pack(">IIBBBBB", 100_000, 100_000, 8, 2, 0, 0, 0)
    empty = b"\x78\x9c\x03\x00\x00\x00\x00\x01"
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr)
            + chunk(b"IDAT", empty) + chunk(b"IEND", b""))


def truncated_idat(source: bytes) -> bytes:
    """Replace the IDAT payload with a two-byte truncated zlib header, keeping the
    chunk length and CRC self-consistent. The stream itself is then invalid."""
    offset = 8
    out = bytearray(source)
    while offset + 8 <= len(out):
        length = struct.unpack(">I", out[offset:offset + 4])[0]
        kind = bytes(out[offset + 4:offset + 8])
        start = offset + 8
        end = start + length
        if kind == b"IDAT" and length > 2:
            payload = b"\x78\x9c"
            crc = zlib.crc32(b"IDAT" + payload) & 0xFFFFFFFF
            return (bytes(out[:offset]) + struct.pack(">I", len(payload)) + b"IDAT"
                    + payload + struct.pack(">I", crc) + bytes(out[end + 4:]))
        offset = end + 4
    raise SystemExit("IDAT not found")


def bmp(width: int, height: int, colour: tuple[int, int, int]) -> bytes:
    row = bytes((colour[2], colour[1], colour[0])) * width
    padding = (4 - len(row) % 4) % 4
    pixels = (row + b"\x00" * padding) * height
    header = struct.pack("<IiiHHIIiiII", 40, width, height, 1, 24, 0,
                         len(pixels), 2835, 2835, 0, 0)
    file_header = struct.pack("<2sIHHI", b"BM", 14 + len(header) + len(pixels), 0, 0, 54)
    return file_header + header + pixels


def jpeg_via_wpf(path: Path, width: int = 32, height: int = 24) -> None:
    script = f"""
Add-Type -AssemblyName PresentationCore
$w = [System.Windows.Media.Imaging.WriteableBitmap]::new({width}, {height}, 96, 96,
    [System.Windows.Media.PixelFormats]::Bgra32, $null)
$pixels = [byte[]]::new({width * height * 4})
for ($i = 0; $i -lt $pixels.Length; $i += 4) {{
  $pixels[$i] = {SOLID[2]}; $pixels[$i + 1] = {SOLID[1]}; $pixels[$i + 2] = {SOLID[0]}; $pixels[$i + 3] = 255
}}
$w.WritePixels([System.Windows.Int32Rect]::new(0, 0, {width}, {height}), $pixels, {width * 4}, 0)
$encoder = [System.Windows.Media.Imaging.JpegBitmapEncoder]::new()
$encoder.QualityLevel = 90
$encoder.Frames.Add([System.Windows.Media.Imaging.BitmapFrame]::Create($w))
$stream = [System.IO.File]::Create('{path.as_posix()}')
try {{ $encoder.Save($stream) }} finally {{ $stream.Dispose() }}
"""
    subprocess.run([PWSH, "-NoProfile", "-Command", script], check=True,
                   capture_output=True, timeout=120)


def run_probe(name: str, image: Path, output: Path | None, limits: dict[str, int] | None,
              timeout: int = 60) -> dict:
    argv = [PWSH, "-NoProfile", "-File", str(PROBE), "-InputPath", str(image)]
    if output is not None:
        argv += ["-OutputPath", str(output)]
    for key, value in (limits or {}).items():
        argv += [f"-{key}", str(value)]
    started = __import__("time").monotonic()
    completed = subprocess.run(argv, capture_output=True, text=True, timeout=timeout)
    elapsed_ms = round((__import__("time").monotonic() - started) * 1000)
    try:
        document = json.loads(completed.stdout)
    except json.JSONDecodeError:
        document = {"parse_error": completed.stdout[:500]}
    return {"case": name, "argv_limits": limits, "exit_code": completed.returncode,
            "elapsed_ms": elapsed_ms, "stdout": document,
            "output_exists": bool(output and output.exists()),
            "output_bytes": output.stat().st_size if output and output.exists() else None,
            "stderr": completed.stderr[:500]}


def harness(cases: Path, out: Path, arguments: list[str]) -> dict:
    argv = [str(HARNESS), "--cases", str(cases), "--out", str(out),
            "--candidate", f"wic=pwsh:{PROBE.as_posix()}", "--output-policy", "canonical-png",
            *arguments]
    completed = subprocess.run(argv, capture_output=True, text=True, timeout=900)
    record = json.loads(out.read_text(encoding="utf-8")) if out.exists() else None
    return {"exit_code": completed.returncode, "record": record,
            "stderr": completed.stderr[-800:]}


def main() -> int:
    # Refuse to reuse a directory: this runner only ever deletes or overwrites files it
    # created itself inside a fresh, exclusively created work root.
    if WORK.exists():
        print(json.dumps({"refused": "work_dir_exists",
                          "work": "<W1_WORK>"}))
        return 2
    WORK.mkdir(parents=True, exist_ok=False)
    for directory in (CASES, RESULTS):
        directory.mkdir(parents=True, exist_ok=True)

    fixtures = {
        "ok.png": png(16, 16, SOLID),
        "at17.png": png(17, 17, SOLID),
        "ok.bmp": bmp(16, 16, SOLID),
        "huge.png": truncated_png(),
        "junk.bin": b"not-an-image\n" * 8,
        "zero.png": png(16, 16, SOLID)[:16] + struct.pack(">II", 0, 16)
                    + png(16, 16, SOLID)[24:],
    }
    fixtures["truncated.png"] = truncated_idat(fixtures["ok.png"])
    fixtures["truncated-file.png"] = fixtures["ok.png"][:40]
    inventory = []
    for name, data in fixtures.items():
        (CASES / name).write_bytes(data)
        inventory.append({"name": name, "bytes": len(data), "sha256": sha(data)})
    jpeg = CASES / "ok.jpg"
    jpeg_via_wpf(jpeg)
    inventory.append({"name": "ok.jpg", "bytes": jpeg.stat().st_size,
                      "sha256": sha(jpeg.read_bytes())})

    rgba = bytes((SOLID[0], SOLID[1], SOLID[2], 255)) * 16 * 16
    pixels16 = hashlib.sha256(rgba).hexdigest()
    (CASES / "cases.txt").write_text(
        f"ok.png|expected:16x16|pixels:{pixels16}\n"
        f"ok.bmp|expected:16x16|pixels:{pixels16}\n"
        "ok.jpg|expected:32x24\n"
        "junk.bin\n"
        "truncated.png\n"
        "truncated-file.png\n"
        "huge.png\n"
        "zero.png\n", encoding="utf-8")

    identity = subprocess.run([PWSH, "-NoProfile", "-File", str(PROBE), "-Version"],
                              capture_output=True, text=True, timeout=60)
    baseline = harness(CASES, RESULTS / "budget-default.json", [
        "--max-dimension", "16384", "--max-bit-depth", "16",
        "--max-decoded-bytes", "268435456", "--max-scratch-bytes", "268435456",
        "--max-pixels", "268435456"])
    rerun = harness(CASES, RESULTS / "budget-default-rerun.json", [
        "--max-dimension", "16384", "--max-bit-depth", "16",
        "--max-decoded-bytes", "268435456", "--max-scratch-bytes", "268435456",
        "--max-pixels", "268435456"])

    narrow_dir = WORK / "narrow"
    narrow_dir.mkdir(exist_ok=True)
    for name in ("ok.png", "ok.bmp", "at17.png"):
        (narrow_dir / name).write_bytes((CASES / name).read_bytes())
    (narrow_dir / "cases.txt").write_text(
        f"ok.png|expected:16x16|pixels:{pixels16}\n"
        f"ok.bmp|expected:16x16|pixels:{pixels16}\n"
        "at17.png\n", encoding="utf-8")
    narrow_run = harness(narrow_dir, RESULTS / "budget-narrow.json", [
        "--max-dimension", "16", "--max-bit-depth", "16",
        "--max-decoded-bytes", "1048576", "--max-scratch-bytes", "1048576",
        "--max-pixels", "65536"])

    ok = CASES / "ok.png"
    generous = {"MaxPixels": 4096, "MaxDimension": 64, "MaxBitDepth": 8,
                "MaxDecodedBytes": 65536, "MaxScratchBytes": 65536}

    def limits(**changes: int) -> dict[str, int]:
        value = dict(generous)
        value.update(changes)
        return value

    boundaries = []
    for label, settings in [
        ("max_pixels_at", limits(MaxPixels=256)),
        ("max_pixels_over", limits(MaxPixels=255)),
        ("max_dimension_at", limits(MaxDimension=16)),
        ("max_dimension_over", limits(MaxDimension=15)),
        ("max_decoded_at", limits(MaxDecodedBytes=768)),
        ("max_decoded_over", limits(MaxDecodedBytes=767)),
        ("max_scratch_at", limits(MaxScratchBytes=1024)),
        ("max_scratch_over", limits(MaxScratchBytes=1023)),
        ("max_bit_depth_at", limits(MaxBitDepth=8)),
        ("max_bit_depth_over", limits(MaxBitDepth=7)),
        ("legacy_max_pixels_over", {"MaxPixels": 255}),
        ("legacy_max_pixels_at", {"MaxPixels": 256}),
    ]:
        boundaries.append(run_probe(label, ok, RESULTS / f"{label}.png", settings))

    edge_cases = [
        run_probe("partial_budget", ok, None, {"MaxPixels": 256, "MaxDimension": 16}),
        run_probe("explicit_zero_dimension", ok, None, {"MaxDimension": 0}),
        run_probe("scratch_only", ok, None, {"DeclaredScratchBytes": 1}),
        run_probe("negative_budget", ok, None, {"MaxPixels": 256, "MaxDimension": -1,
                                                "MaxBitDepth": 8, "MaxDecodedBytes": 768,
                                                "MaxScratchBytes": 1024}),
        run_probe("declared_scratch_over", ok, None,
                  limits(MaxScratchBytes=1024, DeclaredScratchBytes=1)),
        run_probe("missing_input", CASES / "absent.png", None, None),
        run_probe("huge_declared", CASES / "huge.png", RESULTS / "huge-out.png", limits()),
        run_probe("unknown_format", CASES / "junk.bin", RESULTS / "junk-out.png", limits()),
        run_probe("truncated_idat", CASES / "truncated.png",
                  RESULTS / "truncated-out.png", limits()),
        run_probe("truncated_container", CASES / "truncated-file.png",
                  RESULTS / "truncated-file-out.png", limits()),
        run_probe("zero_dimension", CASES / "zero.png", RESULTS / "zero-out.png", limits()),
    ]
    repeat = [run_probe("repeat_a", ok, RESULTS / "repeat-a.png", limits()),
              run_probe("repeat_b", ok, RESULTS / "repeat-b.png", limits())]
    cancelled = harness(CASES, RESULTS / "cancelled.json",
                        ["--max-dimension", "16384", "--max-bit-depth", "16",
                         "--max-decoded-bytes", "268435456",
                         "--max-scratch-bytes", "268435456",
                         "--max-pixels", "268435456", "--cancel-after-ms", "1"])

    def observations(run: dict) -> dict:
        record = run.get("record") or {}
        return {row["case_id"]: {key: row[key] for key in
                                 ("observed_outcome", "candidate_reason", "validity",
                                  "pixel_digest", "output_bytes", "exit_code")}
                for row in record.get("observations", [])}

    repeatable = observations(baseline) == observations(rerun)
    probe_repeatable = (repeat[0]["stdout"] == repeat[1]["stdout"]
                        and repeat[0]["exit_code"] == repeat[1]["exit_code"])

    report = {
        "schema_version": 1,
        "scope": "real Windows WIC probe admission, bounded fixtures, exploratory",
        "probe_identity": identity.stdout.strip(),
        "probe_identity_exit": identity.returncode,
        "pwsh": subprocess.run([PWSH, "-NoProfile", "-Command",
                                "$PSVersionTable.PSVersion.ToString()"],
                               capture_output=True, text=True).stdout.strip(),
        "windows": subprocess.run([PWSH, "-NoProfile", "-Command",
                                   "(Get-CimInstance Win32_OperatingSystem).BuildNumber"],
                                  capture_output=True, text=True).stdout.strip(),
        "fixtures": inventory,
        "fixture_generation": "generated by run_matrix.py: manual PNG/BMP/zlib and "
                              "WPF JpegBitmapEncoder; no seed-based randomness used",
        "expected_rgba_sha256": pixels16,
        "harness_baseline": baseline,
        "harness_rerun_observations_identical": repeatable,
        "harness_narrow_boundary": narrow_run,
        "probe_boundaries": boundaries,
        "probe_edge_cases": edge_cases,
        "probe_repeat_identical": probe_repeatable,
        "probe_repeat_pair": repeat,
        "cancellation_run": cancelled,
        "observed_interpretation": {
            "truncated_idat": "WIC accepted a self-consistent PNG whose IDAT decompresses to a "
                              "truncated zlib stream and produced a deterministic raster; this is "
                              "measured WIC tolerance, not a fail-closed corrupt result",
            "truncated_container": "a file cut inside its first IDAT was rejected as an unknown "
                                   "image format (exit 2), not as stream corruption",
            "zero_dimension": "a zero-width IHDR was rejected by WIC itself as BADHEADER before "
                              "the probe metadata check could run",
            "corrupt_png": "no PNG fixture produced the corrupt class through IDAT or CRC damage; "
                           "the corrupt class is represented by the zero-dimension header",
        },
        "unmeasured": ["WIC internal scratch/conversion peak",
                       "concurrent resident memory ceiling",
                       "in-decoder cancellation granularity",
                       "region/sequential/tiled decode capability",
                       "colour management, ICC, orientation",
                       "JPEG absolute raster equality",
                       "percentiles or performance distribution"],
    }
    expectations = {
        "ok.png": ("decoded", "pixels_verified", "not_measured"),
        "ok.bmp": ("decoded", "pixels_verified", "not_measured"),
        "ok.jpg": ("decoded", "structure_verified", "not_measured"),
        "at17.png": ("decoded", "presence_only", "not_measured"),
        "huge.png": ("oversize", "not_measured", "max_dimension_exceeded"),
        "junk.bin": ("unsupported", "not_measured", "win_component_not_found"),
        "truncated.png": ("decoded", "presence_only", "not_measured"),
        "truncated-file.png": ("unsupported", "not_measured", "win_unknown_image_format"),
        "zero.png": ("corrupt", "not_measured", "win_bad_header"),
    }
    mismatches = []
    measured_rows = observations(baseline)
    for case, (outcome, validity, reason) in expectations.items():
        row = measured_rows.get(case)
        if row is None:
            mismatches.append({"case": case, "problem": "missing"})
            continue
        if (row["observed_outcome"], row["validity"], row["candidate_reason"]) != \
                (outcome, validity, reason):
            mismatches.append({"case": case, "observed": row})
    boundary_expectations = {
        "max_pixels_at": (0, None), "max_pixels_over": (4, "max_pixels_exceeded"),
        "max_dimension_at": (0, None), "max_dimension_over": (4, "max_dimension_exceeded"),
        "max_decoded_at": (0, None), "max_decoded_over": (4, "max_decoded_bytes_exceeded"),
        "max_scratch_at": (0, None), "max_scratch_over": (4, "max_scratch_bytes_exceeded"),
        "max_bit_depth_at": (0, None), "max_bit_depth_over": (4, "max_bit_depth_exceeded"),
        "legacy_max_pixels_over": (4, "max_pixels_exceeded"),
        "legacy_max_pixels_at": (0, None),
    }
    for row in boundaries:
        expected = boundary_expectations[row["case"]]
        if (row["exit_code"], row["stdout"].get("reason_code")) != expected:
            mismatches.append({"case": row["case"], "observed": [row["exit_code"],
                                                                 row["stdout"].get("reason_code")]})
    edge_expectations = {
        "partial_budget": (1, "invalid_limits"),
        "explicit_zero_dimension": (1, "invalid_limits"),
        "scratch_only": (1, "invalid_limits"),
        "negative_budget": (1, "invalid_limits"),
        "declared_scratch_over": (4, "max_scratch_bytes_exceeded"),
        "missing_input": (1, "input_missing"),
        "huge_declared": (4, "max_dimension_exceeded"),
        "unknown_format": (2, "win_component_not_found"),
        "truncated_idat": (0, None),
        "truncated_container": (2, "win_unknown_image_format"),
        "zero_dimension": (3, "win_bad_header"),
    }
    for row in edge_cases:
        expected = edge_expectations[row["case"]]
        if (row["exit_code"], row["stdout"].get("reason_code")) != expected:
            mismatches.append({"case": row["case"], "observed": [row["exit_code"],
                                                                 row["stdout"].get("reason_code")]})
    if not repeatable:
        mismatches.append({"problem": "harness_rerun_differs"})
    if not probe_repeatable:
        mismatches.append({"problem": "probe_rerun_differs"})
    if not all(row["observed_outcome"] == "cancelled"
               for row in cancelled.get("record", {}).get("observations", [])):
        mismatches.append({"problem": "cancellation_not_observed"})
    verification = {"ok": not mismatches, "mismatches": mismatches,
                    "record_generation": "succeeded"}
    report["verification"] = verification
    (RESULTS / "matrix.json").write_text(json.dumps(report, indent=2, sort_keys=True) + "\n",
                                         encoding="utf-8")
    print(json.dumps({"cases": len(inventory), "baseline_exit": baseline["exit_code"],
                      "repeatable": repeatable, "probe_repeatable": probe_repeatable,
                      "verification_ok": verification["ok"],
                      "mismatches": mismatches,
                      "boundaries": [(row["case"], row["exit_code"],
                                      row["stdout"].get("reason_code"))
                                     for row in boundaries]}, indent=2))
    return 0 if verification["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
