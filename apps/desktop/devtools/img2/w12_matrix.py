"""Serial, bounded real-WIC observations for the W12 matched-raster JPEG/PNG experiment.

Six fixed scenarios run as six separate worker lifetimes, never overlapping in
time. A decode worker is parked at the after_onload boundary, sampled once,
then released and its retained ROI PNG is verified against the independent
DC/block-coordinate expectation of the matched raster. A refusal worker is
parked at before_admission, sampled once, released, and its pre-OnLoad refusal
plus absent output are observed. Every monitored bound, trace, capture, process
identity and reaping record is fail-closed: an unobserved or incomplete case
never becomes a successful case.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import time

from w2_resource_matrix import Worker, file_sha, sha
from w6_memory_sample import SAMPLE_SCRIPT, capture_window
from w11_jpeg_fixtures import FIXTURES as JPEG_FIXTURES, write_fixture
from w11_jpeg_oracle import source_geometry, verify_fixture
from w12_png_fixture import FIXTURES as PNG_FIXTURES, write_png
from w12_png_oracle import compare_all_pixels, compare_roi, verify_png
from w12_jpeg_verify import (
    ALLOCATION_SCOPE,
    BOUNDS,
    LIMITS,
    SCENARIOS,
    SOURCE_FILES,
    UNMEASURED,
    verify,
)

ROOT = Path(__file__).resolve().parents[4]
_CASE_COUNT = 6
_JPEG_ID = "jpeg_gray_4k"
_PNG_ID = "png_gray_matched_4k"
_ROLES = ("decode", "refuse", "refusal")
_JPEG_METHOD = (
    "streamed DC-only grayscale baseline JPEG; 64 KiB entropy chunks; Q=8; "
    "block level=48+20*((3*bx+5*by+(bx^by)) mod 9); no full-frame raster"
)
_PNG_METHOD = (
    "streamed 8-bit grayscale PNG: one filter-0 row (4096 bytes) at a time, chunked "
    "deflate with a fixed producer level, CRC32 per chunk, IHDR/IDAT/IEND only; "
    "no full-frame raster buffer; exclusive create"
)
_NORMALIZATION = "Raw Native hashes plus explicitly declared LF-normalized Git correspondence"
_MEMORY_CLASS = "OBSERVED_WHOLE_PROCESS_DIFFERENTIAL"
_MEMORY_FORBIDDEN = (
    "WIC-only allocation delta",
    "decoder internal scratch",
    "allocator reservations",
    "true concurrent allocation peak",
    "same-process before/after allocation delta",
    "causal codec memory advantage",
    "product memory SLO",
)


def _is_reparse(path: Path) -> bool:
    return path.is_symlink() or (hasattr(path, "is_junction") and path.is_junction())


def _safe_path(path) -> Path:
    candidate = Path(path)
    if (not candidate.is_absolute() or len(candidate.drive) != 2 or candidate.drive[1] != ":"
            or ".." in candidate.parts):
        raise ValueError("absolute_local_disk_path_required")
    if any(_is_reparse(part) for part in (candidate, *candidate.parents)):
        raise ValueError("reparse_path_refused")
    resolved = candidate.resolve(strict=False)
    if any(_is_reparse(part) for part in (resolved, *resolved.parents)):
        raise ValueError("reparse_path_refused")
    return resolved


def _git(*args) -> bytes:
    executable = os.environ.get("GIT_EXECUTABLE") or shutil.which("git")
    if not executable:
        raise ValueError("git_executable_unavailable")
    completed = subprocess.run(
        [executable, *args], cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        check=True, timeout=15,
    )
    if len(completed.stdout) > BOUNDS["max_report_bytes"] or len(completed.stderr) > BOUNDS["max_capture_bytes"]:
        raise ValueError("source_identity_capture_cap")
    return completed.stdout


def _source_identity(pwsh: Path) -> dict:
    commit = _git("rev-parse", "HEAD").decode("ascii").strip()
    if re.fullmatch(r"[0-9a-f]{40}", commit) is None:
        raise ValueError("source_commit_invalid")
    files = {}
    for relative in SOURCE_FILES:
        rel = Path(relative)
        if rel.is_absolute() or ".." in rel.parts:
            raise ValueError("source_path_invalid")
        path = _safe_path(ROOT / rel)
        data = path.read_bytes()
        normalized = data.replace(b"\r\n", b"\n").replace(b"\r", b"\n")
        blob = _git("show", f"{commit}:{relative}")
        if normalized != blob.replace(b"\r\n", b"\n").replace(b"\r", b"\n"):
            raise ValueError("experimental_source_not_pinned")
        files[relative] = {
            "bytes": len(data),
            "raw_sha256": sha(data),
            "lf_sha256": sha(normalized),
            "git_blob_sha256": sha(blob),
            "unchanged_after_run": False,
        }
    return {
        "source_commit": commit,
        "files": files,
        "pwsh_sha256": file_sha(pwsh),
        "normalization": _NORMALIZATION,
    }


def _same_source(before: dict, after: dict) -> bool:
    if (before["source_commit"] != after["source_commit"]
            or before["pwsh_sha256"] != after["pwsh_sha256"]
            or before["normalization"] != after["normalization"]
            or before["files"].keys() != after["files"].keys()):
        return False
    return all(
        {k: v for k, v in before["files"][name].items() if k != "unchanged_after_run"}
        == {k: v for k, v in after["files"][name].items() if k != "unchanged_after_run"}
        for name in before["files"]
    )


def _is_decode(case: dict) -> bool:
    role = case.get("role")
    if role not in _ROLES:
        raise ValueError("scenario_role_invalid")
    return role == "decode"


def _hold_name(case: dict) -> tuple[str, ...]:
    return ("after_onload",) if _is_decode(case) else ("before_admission",)


def _fixture_specs() -> tuple[dict, dict]:
    jpeg = next((spec for spec in JPEG_FIXTURES if spec["id"] == _JPEG_ID), None)
    png = next((spec for spec in PNG_FIXTURES if spec["id"] == _PNG_ID), None)
    if jpeg is None or png is None:
        raise ValueError("fixture_spec_missing")
    for spec, codec in ((jpeg, "jpeg"), (png, "png")):
        if (spec["codec"] != codec or spec["width"] != 4096 or spec["height"] != 4096):
            raise ValueError("fixture_spec_invalid")
    if {case["fixture_id"] for case in SCENARIOS} != {_JPEG_ID, _PNG_ID}:
        raise ValueError("scenario_fixture_missing")
    return jpeg, png


def _work_bytes(work: Path) -> int:
    total = 0
    entries = 0
    for path in work.rglob("*"):
        entries += 1
        if entries > 256 or _is_reparse(path):
            raise ValueError("work_entry_bound_or_reparse")
        try:
            info = path.stat()
        except FileNotFoundError:
            parts = path.relative_to(work).parts
            owned_publish = (
                len(parts) == 4 and parts[0] == "workers" and parts[2:] == ("observe", "pending.png")
                and any(parts[1] == case["id"] + "_" + hold
                        for case in SCENARIOS for hold in _hold_name(case))
            )
            if not owned_publish:
                raise
            # The probe atomically renames this owned file to output.png. A stale
            # enumeration must reserve its full cap, not undercount or retry it.
            total += BOUNDS["max_output_bytes"]
            continue
        if stat.S_ISREG(info.st_mode):
            total += info.st_size
        elif not stat.S_ISDIR(info.st_mode):
            raise ValueError("work_nonregular_entry")
    return total


def _guard(work: Path, started: int, workers=()) -> int:
    now = time.monotonic_ns()
    if now - started > BOUNDS["max_total_seconds"] * 1_000_000_000:
        raise ValueError("total_deadline_exceeded")
    if len(workers) > BOUNDS["max_workers"]:
        raise ValueError("worker_cap_exceeded")
    work_bytes = _work_bytes(work)
    if work_bytes > BOUNDS["max_work_bytes"]:
        raise ValueError("work_cap_exceeded")
    for worker in workers:
        if now - worker.started > BOUNDS["max_worker_seconds"] * 1_000_000_000:
            raise ValueError("worker_deadline_exceeded")
        trace = worker.observe / "trace.jsonl"
        if trace.exists() and (_is_reparse(trace) or not trace.is_file()
                               or trace.stat().st_size > BOUNDS["max_trace_bytes"]):
            raise ValueError("trace_cap_or_path_invalid")
        worker.read_trace()
        if trace.exists() and len(worker.rows) > BOUNDS["max_trace_records"]:
            raise ValueError("trace_record_cap_exceeded")
        for path in (worker.stdout_path, worker.stderr_path):
            if _is_reparse(path) or not path.is_file() or path.stat().st_size > BOUNDS["max_capture_bytes"]:
                raise ValueError("capture_cap_or_path_invalid")
        for path in (worker.output, worker.observe / "pending.png", worker.observe / "pending.bmp"):
            if _is_reparse(path):
                raise ValueError("output_path_invalid")
            try:
                info = path.stat()
            except FileNotFoundError:
                continue
            if not stat.S_ISREG(info.st_mode):
                raise ValueError("output_path_invalid")
            if info.st_size > BOUNDS["max_output_bytes"]:
                raise ValueError("output_cap_exceeded")
    return work_bytes


def _platform(pwsh: Path) -> dict:
    script = (
        "[ordered]@{powershell=$PSVersionTable.PSVersion.ToString();"
        "dotnet=[Runtime.InteropServices.RuntimeInformation]::FrameworkDescription;"
        "os=[Environment]::OSVersion.VersionString;"
        "windows=[OperatingSystem]::IsWindows()}|ConvertTo-Json -Compress"
    )
    result = subprocess.run(
        [str(pwsh), "-NoProfile", "-Command", script], stdout=subprocess.PIPE,
        stderr=subprocess.PIPE, timeout=5, check=True,
    )
    if (len(result.stdout) > BOUNDS["max_capture_bytes"]
            or len(result.stderr) > BOUNDS["max_capture_bytes"]):
        raise ValueError("platform_capture_cap")
    try:
        platform = json.loads(result.stdout.decode("utf-8"))
    except (UnicodeError, json.JSONDecodeError) as exc:
        raise ValueError("platform_record_invalid") from exc
    if (not isinstance(platform, dict) or platform.get("windows") is not True
            or any(not isinstance(platform.get(key), str) or not platform[key]
                   for key in ("powershell", "dotnet", "os"))):
        raise ValueError("windows_platform_required")
    return platform


def _jpeg_fixture(jpeg_spec: dict, path: Path) -> tuple[dict, bytes, dict]:
    generated = write_fixture(jpeg_spec["id"], path)
    if (generated.get("case_id") != jpeg_spec["id"] or generated.get("bytes") != path.stat().st_size
            or _is_reparse(path) or not path.is_file()):
        raise ValueError("fixture_write_identity_invalid")
    if generated["bytes"] <= 0 or generated["bytes"] > BOUNDS["max_encoded_input_bytes"]:
        raise ValueError("encoded_input_over_bound")
    data = path.read_bytes()
    if len(data) != generated["bytes"] or sha(data) != "sha256:" + generated["sha256"]:
        raise ValueError("fixture_digest_mismatch")
    geometry = source_geometry(data)
    independent = verify_fixture(jpeg_spec["id"], data)
    if (independent.get("fixture_id") != jpeg_spec["id"] or independent.get("bytes") != len(data)
            or independent.get("sha256") != sha(data) or independent.get("geometry") != geometry
            or (geometry.get("width"), geometry.get("height"))
            != (jpeg_spec["width"], jpeg_spec["height"])):
        raise ValueError("fixture_oracle_mismatch")
    record = {
        "case_id": jpeg_spec["id"],
        "path": path.name,
        "codec": "jpeg",
        "width": jpeg_spec["width"],
        "height": jpeg_spec["height"],
        "bytes": len(data),
        "sha256": sha(data),
        "method": _JPEG_METHOD,
        "producer": "w11_jpeg_fixtures.write_fixture",
        "geometry": geometry,
    }
    oracle = {
        "fixture_id": jpeg_spec["id"],
        "codec": "jpeg",
        "basis": "dc_only_entropy_scan_block_predictor",
        "blocks_verified": geometry["blocks"],
        "pixels_covered": geometry["blocks"] * 64,
        "expected_sha256": independent["sha256"],
        "ok": True,
    }
    return record, data, oracle


def _png_fixture(png_spec: dict, path: Path) -> tuple[dict, bytes, dict]:
    generated = write_png(path)
    if (generated.get("case_id") != png_spec["id"] or generated.get("bytes") != path.stat().st_size
            or _is_reparse(path) or not path.is_file()):
        raise ValueError("fixture_write_identity_invalid")
    if generated["bytes"] <= 0 or generated["bytes"] > BOUNDS["max_encoded_input_bytes"]:
        raise ValueError("encoded_input_over_bound")
    data = path.read_bytes()
    if len(data) != generated["bytes"] or sha(data) != "sha256:" + generated["sha256"]:
        raise ValueError("fixture_digest_mismatch")
    geometry = verify_png(data)
    if (geometry.get("width"), geometry.get("height")) != (png_spec["width"], png_spec["height"]):
        raise ValueError("fixture_oracle_mismatch")
    if (geometry.get("depth") != 8 or geometry.get("color_type") != 0
            or geometry.get("bytes") != len(data) or geometry.get("sha256") != sha(data)):
        raise ValueError("fixture_oracle_mismatch")
    comparison = compare_all_pixels(data)
    if (comparison.get("ok") is not True
            or comparison.get("compared_pixels") != png_spec["width"] * png_spec["height"]):
        raise ValueError("fixture_pixel_match_failed")
    record = {
        "case_id": png_spec["id"],
        "path": path.name,
        "codec": "png",
        "width": png_spec["width"],
        "height": png_spec["height"],
        "bytes": len(data),
        "sha256": sha(data),
        "method": _PNG_METHOD,
        "producer": "w12_png_fixture.write_png",
        "geometry": geometry,
    }
    oracle = {
        "fixture_id": png_spec["id"],
        "codec": "png",
        "basis": "streamed_full_raster_against_fixed_formula",
        "compared_pixels": comparison["compared_pixels"],
        "first_mismatch": comparison.get("first_mismatch"),
        "expected_sha256": comparison.get("expected_sha256"),
        "actual_sha256": comparison.get("actual_sha256"),
        "ok": True,
    }
    return record, data, oracle


def _fixture_records(work: Path, started: int):
    cases_dir = work / "cases"
    cases_dir.mkdir(exist_ok=False)
    jpeg_spec, png_spec = _fixture_specs()
    fixture_inventory = {}
    fixture_data = {}
    records = []
    oracles = []
    for spec, writer in (
        (jpeg_spec, _jpeg_fixture),
        (png_spec, _png_fixture),
    ):
        path = cases_dir / (spec["id"] + "." + spec["codec"])
        record, data, oracle = writer(spec, path)
        records.append(record)
        oracles.append(oracle)
        fixture_inventory[spec["id"]] = {"path": record["path"], "sha256": record["sha256"]}
        fixture_data[spec["id"]] = data
        _guard(work, started)
    if [record["codec"] for record in records] != ["jpeg", "png"]:
        raise ValueError("fixture_order_invalid")
    if oracles[0]["pixels_covered"] != oracles[1]["compared_pixels"]:
        raise ValueError("matched_raster_extent_mismatch")
    return records, fixture_inventory, fixture_data, oracles


def _strict_trace(worker: Worker) -> tuple[bytes, str]:
    path = worker.observe / "trace.jsonl"
    if _is_reparse(path) or not path.is_file():
        raise ValueError("trace_missing")
    if path.stat().st_size > BOUNDS["max_trace_bytes"]:
        raise ValueError("trace_cap_exceeded")
    data = path.read_bytes()
    if not data or not data.endswith(b"\n"):
        raise ValueError("trace_incomplete")
    lines = data.splitlines()
    if len(lines) > BOUNDS["max_trace_records"]:
        raise ValueError("trace_record_cap_exceeded")
    try:
        rows = [json.loads(line.decode("utf-8")) for line in lines]
        text = data.decode("utf-8")
    except (UnicodeError, json.JSONDecodeError) as exc:
        raise ValueError("trace_invalid") from exc
    if (rows != worker.rows or not rows or rows[-1].get("stage") != "probe_exit"
            or rows[-1].get("complete") is not True):
        raise ValueError("trace_terminal_invalid")
    return data, text


def _decoder_observation(result: dict, fixture: dict) -> dict:
    """Record the decoder and pixel format the probe actually reported, never a guess."""
    decoder = result.get("decoder")
    pixel_format = result.get("pixel_format")
    if not isinstance(decoder, str) or not decoder or not isinstance(pixel_format, str) or not pixel_format:
        raise ValueError("decoder_observation_missing")
    for key in ("width", "height", "bits_per_channel", "bits_per_pixel"):
        if type(result.get(key)) is not int:
            raise ValueError("decoder_observation_incomplete")
    if (result["width"], result["height"]) != (fixture["width"], fixture["height"]):
        raise ValueError("decoder_geometry_contradiction")
    if result["bits_per_channel"] != 8:
        raise ValueError("decoder_depth_contradiction")
    return {
        "decoder": decoder,
        "pixel_format": pixel_format,
        "width": result["width"],
        "height": result["height"],
        "bits_per_channel": result["bits_per_channel"],
        "bits_per_pixel": result["bits_per_pixel"],
        "source_format": result.get("source_format"),
        "declared_container": fixture["codec"],
    }


def _memory_window(worker: dict, case: dict) -> dict:
    """One bounded observation window for one worker lifetime, kept as observed."""
    sample = worker["sample"]
    if (sample.get("index") != 0 or sample.get("gap_ns") is not None
            or sample.get("case_ids") != [worker["case_id"]]
            or sample.get("complete") is not True or sample.get("status") != "complete"
            or sample.get("failure") is not None
            or sample.get("classification") != "DERIVED_WINDOW_SUM"
            or sample.get("atomic_snapshot") is not False
            or sample.get("aggregation") != "same_collection_window"):
        raise ValueError("sample_window_invalid")
    totals = sample.get("totals")
    samples = sample.get("samples")
    if not isinstance(totals, dict) or not isinstance(samples, list) or len(samples) != 1:
        raise ValueError("sample_totals_invalid")
    if any(type(totals.get(key)) is not int for key in ("working_set_bytes", "private_bytes")):
        raise ValueError("sample_totals_invalid")
    if totals["private_bytes"] > BOUNDS["max_observed_private_bytes"]:
        raise ValueError("observed_private_bound_exceeded")
    return {
        "case_id": worker["case_id"],
        "fixture_id": case["fixture_id"],
        "pid": worker["pid"],
        "start_utc_ticks": worker["start_utc_ticks"],
        "exe_sha256": worker["exe_sha256"],
        "input_sha256": worker["input_sha256"],
        "stage": worker["hold_stage"],
        "window_start_ns": sample["start_ns"],
        "window_end_ns": sample["end_ns"],
        "window_ns": sample["end_ns"] - sample["start_ns"],
        "observer_clock": sample["clock_domain"],
        "observer_qpc_start": sample.get("qpc_start"),
        "observer_qpc_end": sample.get("qpc_end"),
        "observer_qpc_frequency": sample.get("qpc_frequency"),
        "working_set_bytes": totals["working_set_bytes"],
        "private_bytes": totals["private_bytes"],
        "peak_working_set_bytes": sample["samples"][0].get("peak_working_set_bytes"),
        "peak_status": sample["samples"][0].get("peak_status"),
        "capture_complete": sample["observer"]["capture_complete"],
        "lifetime_started_ns": worker["started_ns"],
        "lifetime_exited_ns": worker["exited_ns"],
        "classification": _MEMORY_CLASS,
        "aggregation": "single_process_window",
    }


def _run_worker(case: dict, hold: str, work: Path, pwsh: Path, fixture_inventory: dict,
                fixture_by_id: dict, jpeg_data: bytes, started: int,
                executable_sha: str) -> dict:
    worker_id = case["id"] + "_" + hold
    decode = _is_decode(case)
    kind = "decode" if decode else "oversize"
    limits = dict(LIMITS, max_dimension=case["max_dimension"])
    roi = ",".join(str(value) for value in case["roi"]) if case["roi"] is not None else None
    _guard(work, started)
    worker = None
    try:
        worker = Worker(
            (worker_id, case["fixture_id"], kind, hold), work, pwsh,
            fixture_inventory, limits, roi=roi,
            max_roi_output=BOUNDS["max_roi_output_bytes"],
        )
        while not worker.at(hold):
            _guard(work, started, [worker])
            if worker.proc.poll() is not None:
                raise ValueError("worker_exited_before_required_boundary")
            time.sleep(0.002)
        _guard(work, started, [worker])
        sample = capture_window([worker], pwsh, 0)
        _guard(work, started, [worker])
        if sample.get("complete") is not True or sample.get("status") != "complete":
            raise ValueError("sampler_incomplete")
        totals = sample.get("totals")
        if not isinstance(totals, dict) or type(totals.get("private_bytes")) is not int:
            raise ValueError("observed_private_bytes_invalid")
        if totals["private_bytes"] > BOUNDS["max_observed_private_bytes"]:
            raise ValueError("observed_private_bound_exceeded")
        worker.identity_verified = True
        worker.release()
        while worker.proc.poll() is None:
            _guard(work, started, [worker])
            time.sleep(0.002)
        worker.proc.wait(timeout=5)
        _guard(work, started, [worker])
        result = worker.finish(executable_sha)
        trace_bytes, trace_utf8 = _strict_trace(worker)
        stdout_bytes = worker.stdout_path.read_bytes()
        stderr_bytes = worker.stderr_path.read_bytes()
        if (len(stdout_bytes) > BOUNDS["max_capture_bytes"]
                or len(stderr_bytes) > BOUNDS["max_capture_bytes"]):
            raise ValueError("capture_cap_exceeded")
        stdout_utf8 = stdout_bytes.decode("utf-8")
        stderr_utf8 = stderr_bytes.decode("utf-8")
        if (not result["reaped"] or result["alive_after_wait"] or result["exit_code"] is None
                or result["cancellation"]["state"] != "not_requested"
                or result["trace_complete"] is not True or result["trace_partial_bytes"] != 0
                or result["trace_bytes"] != len(trace_bytes)
                or result["trace_sha256"] != sha(trace_bytes)
                or result["stdout_bytes"] != len(stdout_bytes)
                or result["stderr_bytes"] != len(stderr_bytes)):
            raise ValueError("worker_finish_incomplete")
        if result["probe_result"] != json.loads(stdout_utf8):
            raise ValueError("probe_capture_mismatch")
        observation = _decoder_observation(result["probe_result"], fixture_by_id[case["fixture_id"]])
        if decode:
            if (result["exit_code"] != 0 or result["outcome"] != "decoded"
                    or not result["output_present"] or not result["published_success"]):
                raise ValueError("decode_worker_failed")
            if any((worker.observe / name).exists() for name in ("pending.png", "pending.bmp")):
                raise ValueError("decode_pending_output_remaining")
            output_path = worker.output
            if (not output_path.is_file() or _is_reparse(output_path)
                    or output_path.stat().st_size > BOUNDS["max_output_bytes"]):
                raise ValueError("output_cap_or_path_invalid")
            output_bytes = output_path.read_bytes()
            if len(output_bytes) != result["output_bytes"]:
                raise ValueError("output_size_mismatch")
            roi_coordinates = case["roi"]
            if roi_coordinates is None:
                raise ValueError("decode_roi_missing")
            # One stored oracle value: the independent PNG-side ROI comparison of
            # the retained output. Both codecs' outputs are held to the same
            # fixed DC/block-coordinate raster, so no codec gets its own oracle.
            oracle = compare_roi(output_bytes, list(roi_coordinates))
            if oracle.get("ok") is not True:
                raise ValueError("roi_oracle_mismatch")
            output_hex = output_bytes.hex()
            encoded_output = True
        else:
            if (result["exit_code"] != 4 or result["outcome"] != "oversize"
                    or result["probe_result"].get("admission_status") != "refused"
                    or result["probe_result"].get("materialization") != "not_requested"
                    or result["output_present"] or result["published_success"]
                    or any((worker.observe / name).exists() for name in ("pending.png", "pending.bmp"))):
                raise ValueError("refusal_worker_failed")
            output_hex = None
            oracle = None
            encoded_output = False
        result.update({
            "limits": limits,
            "hold_stage": hold,
            "trace_utf8": trace_utf8,
            "stdout_utf8": stdout_utf8,
            "stderr_utf8": stderr_utf8,
            "output_hex": output_hex,
            "oracle": oracle,
            "decoder_observation": observation,
            "oracle_basis": {
                "fixture_id": _JPEG_ID,
                "sha256": sha(jpeg_data),
                "expectation": "fixed DC/block-coordinate raster shared by both codecs",
                "codec_under_test": case["fixture_id"],
                "output_encoded_as_png": encoded_output,
            },
            "sample": sample,
        })
        return result
    finally:
        if worker is not None:
            proc = worker.proc
            cleanup_error = None
            try:
                if proc.poll() is None:
                    proc.kill()
                proc.wait(timeout=5)
            except BaseException as exc:
                cleanup_error = exc
                try:
                    if proc.poll() is None:
                        proc.kill()
                    proc.wait(timeout=5)
                except BaseException:
                    pass
            for stream in (worker.stdout, worker.stderr):
                try:
                    stream.close()
                except BaseException as exc:
                    if cleanup_error is None:
                        cleanup_error = exc
            if cleanup_error is not None:
                raise ValueError("worker_cleanup_failed") from cleanup_error


def _decode_groups(rows) -> list[tuple[tuple[int, ...], dict[str, dict]]]:
    """Pair the decode rows by ROI, keeping the fixed serial scenario order."""
    grouped: dict[tuple[int, ...], dict[str, dict]] = {}
    for row, case in zip(rows, SCENARIOS):
        if not _is_decode(case):
            continue
        if case["roi"] is None:
            raise ValueError("decode_roi_missing")
        grouped.setdefault(tuple(case["roi"]), {})[row["fixture_id"]] = row
    for group in grouped.values():
        if set(group) != {_JPEG_ID, _PNG_ID}:
            raise ValueError("codec_pair_incomplete")
        if len(group[_JPEG_ID]["workers"]) != 1 or len(group[_PNG_ID]["workers"]) != 1:
            raise ValueError("decode_worker_count_invalid")
    if len(grouped) != 2:
        raise ValueError("roi_pair_count_invalid")
    return list(grouped.items())


def _memory_comparison(rows, windows) -> dict:
    """Keep observed whole-process counters side by side without promoting them."""
    by_worker = {window["case_id"]: window for window in windows}
    if len(by_worker) != len(windows):
        raise ValueError("memory_window_duplicate")
    observations = []
    contrasts = {}
    for (roi, group), label in zip(_decode_groups(rows), ("A", "B")):
        jpeg_window = by_worker[group[_JPEG_ID]["workers"][0]["case_id"]]
        png_window = by_worker[group[_PNG_ID]["workers"][0]["case_id"]]
        observations.append({
            "comparison": label,
            "roi": list(roi),
            "jpeg_case_id": jpeg_window["case_id"],
            "png_case_id": png_window["case_id"],
            "jpeg": {"working_set_bytes": jpeg_window["working_set_bytes"],
                     "private_bytes": jpeg_window["private_bytes"]},
            "png": {"working_set_bytes": png_window["working_set_bytes"],
                    "private_bytes": png_window["private_bytes"]},
            "observed_private_bytes_difference": jpeg_window["private_bytes"] - png_window["private_bytes"],
            "observed_working_set_bytes_difference": jpeg_window["working_set_bytes"] - png_window["working_set_bytes"],
            "interpretation": "WHOLE_PROCESS_COUNTER_DIFFERENCE_OF_SEPARATE_LIFETIMES",
        })
        for codec, window in (("jpeg", jpeg_window), ("png", png_window)):
            contrasts.setdefault(codec, {})[str(roi[2] * roi[3])] = window
    # C: ROI-size contrast inside one codec, still separate lifetimes.
    roi_size_observations = []
    for codec, by_pixels in contrasts.items():
        if set(by_pixels) != {"1", "65536"}:
            raise ValueError("roi_size_contrast_incomplete")
        small, large = by_pixels["1"], by_pixels["65536"]
        roi_size_observations.append({
            "comparison": "C",
            "codec": codec,
            "one_pixel_worker": small["case_id"],
            "roi_256_worker": large["case_id"],
            "observed_private_bytes_difference": large["private_bytes"] - small["private_bytes"],
            "observed_working_set_bytes_difference": large["working_set_bytes"] - small["working_set_bytes"],
            "interpretation": "WHOLE_PROCESS_COUNTER_DIFFERENCE_OF_SEPARATE_LIFETIMES",
        })
    return {
        "classification": _MEMORY_CLASS,
        "boundary": "after_onload",
        "basis": "one worker lifetime per case; no same-process before/after delta",
        "pairs": observations,
        "roi_size_contrast": roi_size_observations,
        "forbidden_interpretations": list(_MEMORY_FORBIDDEN),
    }


def _roi_pairs(rows) -> list[dict]:
    """Explicit codec equality for the two matched ROI sizes."""
    pairs = []
    for roi, group in _decode_groups(rows):
        jpeg, png = group[_JPEG_ID], group[_PNG_ID]
        jpeg_worker, png_worker = jpeg["workers"][0], png["workers"][0]
        jpeg_oracle, png_oracle = jpeg_worker["oracle"], png_worker["oracle"]
        if jpeg_worker["output_hex"] != png_worker["output_hex"]:
            raise ValueError("matched_pair_bytes_differ")
        if jpeg_oracle != png_oracle:
            raise ValueError("matched_pair_oracle_differs")
        pairs.append({
            "roi": list(roi),
            "jpeg_case_id": jpeg["case_id"],
            "png_case_id": png["case_id"],
            "compared_pixels": jpeg_oracle.get("compared_pixels"),
            "first_mismatch": jpeg_oracle.get("first_mismatch"),
            "expected_rgba_sha256": jpeg_oracle.get("expected_sha256"),
            "actual_rgba_sha256": jpeg_oracle.get("actual_sha256"),
            "output_sha256": jpeg_worker["output_sha256"],
            "png_output_sha256": png_worker["output_sha256"],
            "output_bytes_equal": True,
            "output_bytes": [jpeg_worker["output_bytes"], png_worker["output_bytes"]],
        })
    return pairs


def run(work_root, out, pwsh) -> dict:
    work, report_path, pwsh = map(_safe_path, (work_root, out, pwsh))
    if (work.exists() or report_path.exists() or report_path.is_relative_to(work)
            or work.is_relative_to(report_path)):
        raise ValueError("fresh_work_and_external_output_required")
    if not pwsh.is_file():
        raise ValueError("real_powershell_executable_required")
    if any(character in str(work) for character in ("'", "\r", "\n")):
        raise ValueError("unsafe_work_path")
    if len(SCENARIOS) != _CASE_COUNT:
        raise ValueError("fixed_matrix_shape_invalid")
    if sum(len(_hold_name(case)) for case in SCENARIOS) != _CASE_COUNT:
        raise ValueError("serial_lifetime_shape_invalid")
    before = _source_identity(pwsh)
    started = time.monotonic_ns()
    work.mkdir(parents=True, exist_ok=False)
    if _is_reparse(work):
        raise ValueError("work_root_reparse")
    (work / "workers").mkdir(exist_ok=False)
    fixture_records, fixture_inventory, fixture_data, fixture_oracles = _fixture_records(work, started)
    fixture_by_id = {record["case_id"]: record for record in fixture_records}
    jpeg_data = fixture_data[_JPEG_ID]
    platform = _platform(pwsh)
    _guard(work, started)
    rows = []
    all_workers = []
    windows = []
    seen_lifetimes = set()
    for case in SCENARIOS:
        if case["fixture_id"] not in fixture_inventory:
            raise ValueError("scenario_fixture_missing")
        limits = dict(LIMITS, max_dimension=case["max_dimension"])
        worker_records = []
        for hold in _hold_name(case):
            worker_record = _run_worker(
                case, hold, work, pwsh, fixture_inventory, fixture_by_id,
                jpeg_data, started, before["pwsh_sha256"],
            )
            lifetime = (worker_record["pid"], worker_record["start_utc_ticks"])
            if lifetime in seen_lifetimes:
                raise ValueError("worker_lifetime_reused")
            seen_lifetimes.add(lifetime)
            all_workers.append(worker_record)
            worker_records.append(worker_record)
            windows.append(_memory_window(worker_record, case))
        rows.append({
            "case_id": case["id"],
            "fixture_id": case["fixture_id"],
            "codec": case["codec"],
            "role": case["role"],
            "roi": case["roi"],
            "limits": limits,
            "failure": None,
            "classification": "ROI_OUTPUT_VERIFIED" if _is_decode(case) else "REFUSED_FULL_FRAME",
            "workers": worker_records,
        })
        _guard(work, started)
    if len(all_workers) != _CASE_COUNT:
        raise ValueError("worker_run_count_invalid")
    for previous, current in zip(all_workers, all_workers[1:]):
        if current["started_ns"] < previous["exited_ns"]:
            raise ValueError("simultaneous_workers")
    roi_pairs = _roi_pairs(rows)
    memory_comparison = _memory_comparison(rows, windows)
    after = _source_identity(pwsh)
    unchanged = _same_source(before, after)
    for relative, item in before["files"].items():
        item["unchanged_after_run"] = unchanged and (
            {k: v for k, v in item.items() if k != "unchanged_after_run"}
            == {k: v for k, v in after["files"][relative].items() if k != "unchanged_after_run"}
        )
    if not unchanged or not all(item["unchanged_after_run"] for item in before["files"].values()):
        raise ValueError("source_or_executable_changed_during_run")
    if any(worker["reaped"] is not True or worker["alive_after_wait"] is not False for worker in all_workers):
        raise ValueError("worker_cleanup_incomplete")
    _guard(work, started)
    work_bytes = _work_bytes(work)
    if work_bytes > BOUNDS["max_work_bytes"]:
        raise ValueError("work_cap_exceeded")
    record = {
        "schema_version": 1,
        "complete": True,
        "run_id": work.name,
        "bounds": BOUNDS,
        "decode_limits": LIMITS,
        "source_identity": before,
        "platform": platform,
        "fixtures": fixture_records,
        "fixture_oracles": fixture_oracles,
        "cases": rows,
        "roi_pairs": roi_pairs,
        "memory_windows": windows,
        "memory_comparison": memory_comparison,
        "sampler_script_sha256": sha(SAMPLE_SCRIPT.encode("utf-8")),
        "allocation_scope": ALLOCATION_SCOPE,
        "unmeasured": list(UNMEASURED),
        "duration_ns": time.monotonic_ns() - started,
        "work_bytes": work_bytes,
        "cleanup": {"complete": True, "live_direct_children": []},
        "work_root_disposable": True,
        "limitations": [
            "Each case is a separate worker lifetime; the six lifetimes are serial and process counters are not same-process allocation deltas.",
            "The PNG expectation is the fixed DC/block-coordinate raster that the W11 JPEG oracle already derives, not a re-encode of any decoded output.",
            "Process counters include the worker runtime and observer effects; WIC-internal scratch and true allocation peaks are not measured.",
            "Boundary holds are artificial instrumentation and are not decoder latency.",
            "The existing OnLoad WIC path materializes the full frame before CopyPixels; equal ROI output does not verify partial decode or identical internal codec work.",
            "Refusal cases are refused before OnLoad, so their counters are admission-path observations and not 4K decode memory.",
            "No decoder adoption, product memory SLO or codec memory advantage follows from these observations.",
        ],
    }
    if record["duration_ns"] > BOUNDS["max_total_seconds"] * 1_000_000_000:
        raise ValueError("total_deadline_exceeded")
    from w2_resource_verify import canonical
    data = canonical(record)
    if len(data) > BOUNDS["max_report_bytes"]:
        raise ValueError("report_cap_exceeded")
    report_path.parent.mkdir(parents=True, exist_ok=True)
    if any(_is_reparse(part) for part in (report_path.parent, *report_path.parent.parents)):
        raise ValueError("report_parent_reparse")
    with report_path.open("xb") as stream:
        stream.write(data)
    if report_path.stat().st_size != len(data) or report_path.stat().st_size > BOUNDS["max_report_bytes"]:
        raise ValueError("report_write_invalid")
    return record


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="operation", required=True)
    execute = commands.add_parser("run")
    for name in ("work-root", "out", "pwsh"):
        execute.add_argument("--" + name, required=True)
    check = commands.add_parser("verify")
    check.add_argument("--report", required=True)
    check.add_argument("--source-root")
    args = parser.parse_args(argv)
    try:
        if args.operation == "run":
            record = run(args.work_root, args.out, args.pwsh)
            result = verify(record, ROOT)
            success = result.get("ok") is True and result.get("closure_eligible") is True
        else:
            report = Path(args.report)
            if not report.is_file() or report.stat().st_size > BOUNDS["max_report_bytes"]:
                raise ValueError("report_missing_or_over_bound")
            record = json.loads(report.read_bytes().decode("utf-8"))
            result = verify(record, args.source_root)
            success = result.get("ok") is True
        print(json.dumps(result, sort_keys=True, separators=(",", ":")))
        return 0 if success else 1
    except Exception as exc:
        token = str(exc)
        reason = token if re.fullmatch(r"[a-z_]{1,80}", token) else type(exc).__name__
        print(json.dumps({"ok": False, "reason": reason}, sort_keys=True, separators=(",", ":")))
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
