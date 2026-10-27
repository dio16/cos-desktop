"""Bounded real-WIC CLI observations, not a product decoder or memory SLO.

Boundary holds expose real retained allocations for sampling; their duration is
instrumentation, never decoder latency. Process counters do not reveal scratch.
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import time

from w2_resource_verify import (BOUNDS, FIXTURES, MEMORY_NAMES, SCENARIOS, SOURCE_FILES,
                                canonical, file_sha, raster_sha, redact_paths, sha, verify, verify_file)

ROOT = Path(__file__).resolve().parents[4]
PROBE = ROOT / SOURCE_FILES["probe"]
# Narrow experimental overrides, not a second copy of ratified product defaults.
OVERRIDES = {"max_dimension": 1024, "max_pixels": 1024 * 1024, "max_bit_depth": 8,
             "max_decoded_bytes": 4 * 1024 * 1024, "max_scratch_bytes": 8 * 1024 * 1024}


def _w1(pwsh):
    spec = importlib.util.spec_from_file_location("cos_img2_w1", ROOT / SOURCE_FILES["w1_generator"])
    if spec is None or spec.loader is None:
        raise ValueError("w1_generator_missing")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    setattr(module, "PWSH", str(pwsh))
    return module


def _safe_path(path):
    p = Path(path)
    if not p.is_absolute() or len(p.drive) != 2 or p.drive[1] != ":":
        raise ValueError("absolute_local_disk_path_required")
    if any(x.is_symlink() or x.is_junction() for x in (p, *p.parents)):
        raise ValueError("reparse_path_refused")
    return p


def _source_identity(pwsh, harness):
    files = {}
    for name, rel in SOURCE_FILES.items():
        data = (ROOT / rel).read_bytes()
        files[name] = {"path": rel, "bytes": len(data), "raw_sha256": sha(data),
                       "lf_sha256": sha(data.replace(b"\r\n", b"\n").replace(b"\r", b"\n")),
                       "unchanged_after_run": False}
    return {"files": files, "pwsh_sha256": file_sha(pwsh), "harness_sha256": file_sha(harness),
            "lf_normalization": "CRLF and CR to LF for explicitly named Git replay only; raw Native bytes retained separately"}


def _fixtures(work, pwsh):
    w1 = _w1(pwsh)
    cases = work / "cases"
    cases.mkdir()
    result = []
    for cid, (name, fmt, width, height, color) in FIXTURES.items():
        path = cases / name
        if cid == "small_jpeg":
            w1.jpeg_via_wpf(path, width, height)
            method = "W1 jpeg_via_wpf, managed WPF encoder, quality90"
        else:
            if cid == "oversized_png":
                data = w1.truncated_png()
                method = "W1 truncated_png, huge declaration only; never decoded"
            elif cid == "unsupported":
                data = b"not-an-image\n" * 8
                method = "known unsupported literal"
            elif fmt == "bmp":
                data = w1.bmp(width, height, color)
                method = "W1 bmp, solid24bitBGR"
            else:
                data = w1.png(width, height, color)
                method = "W1 png, solid8bitRGB, filter0/zlib"
            with path.open("xb") as stream:
                stream.write(data)
        result.append({"case_id": cid, "path": name, "format": fmt, "width": width,
                       "height": height, "bytes": path.stat().st_size,
                       "sha256": file_sha(path), "method": method})
    lines = []
    for cid, (name, _, w, h, color) in FIXTURES.items():
        line = name
        if color is not None:
            line += f"|expected:{w}x{h}|pixels:{raster_sha(cid)}|decoded"
        elif cid == "small_jpeg":
            line += f"|expected:{w}x{h}|decoded"
        elif cid == "oversized_png":
            line += "|oversize"
        else:
            line += "|unsupported"
        lines.append(line)
    (cases / "cases.txt").write_text("\n".join(lines) + "\n", encoding="utf-8")
    return result


def _work_bytes(work):
    paths = list(work.rglob("*"))
    if len(paths) > 256 or any(p.is_symlink() or p.is_junction() for p in paths):
        raise ValueError("work_entry_bound_or_reparse")
    return sum(p.stat().st_size for p in paths if p.is_file())


# All inputs to this script are owned PID/lifetime tuples, never media metadata.
SAMPLE_SCRIPT = r"""
$ErrorActionPreference='Stop'
$spec=[Console]::In.ReadToEnd()|ConvertFrom-Json
$begin=[Diagnostics.Stopwatch]::GetTimestamp()
$rows=@()
foreach($item in $spec.items) {
  $p=[Diagnostics.Process]::GetProcessById([int]$item.pid)
  try {
    $p.Refresh()
    $before=$p.StartTime.ToUniversalTime().Ticks
    $aliveBefore=-not $p.HasExited
    $exe=$p.MainModule.FileName
    if($before -ne [long]$item.start_utc_ticks -or -not $aliveBefore -or
       -not [StringComparer]::OrdinalIgnoreCase.Equals($exe,[string]$spec.exe)) {throw 'identity_before'}
    $ws=$p.WorkingSet64
    $private=$p.PrivateMemorySize64
    $p.Refresh()
    $after=$p.StartTime.ToUniversalTime().Ticks
    $aliveAfter=-not $p.HasExited
    if($after -ne $before -or -not $aliveAfter) {throw 'identity_after'}
    $rows+=@{case_id=$item.case_id;pid=$p.Id;start_utc_ticks=$before;identity_after_ticks=$after;
      exe_verified=$true;alive_before=$aliveBefore;alive_after=$aliveAfter;
      working_set_bytes=$ws;private_bytes=$private}
  } finally {$p.Dispose()}
}
@{samples=@($rows);qpc_start=$begin;qpc_end=[Diagnostics.Stopwatch]::GetTimestamp();
 qpc_frequency=[Diagnostics.Stopwatch]::Frequency}|ConvertTo-Json -Compress -Depth 5
"""


STOP_STATES = ("not_requested", "already_exited", "stop_confirmed", "unverified_race")


def cancel_record(kind):
    """Initial stop evidence; every field is filled by an observed action or stays unknown."""
    return {"requested_ns": None, "observed_ns": None, "waited": False,
            "last_observed_stage": None, "method": None,
            "reachability": "NOT_MEASURED" if kind == "onload_attempt" else "not_requested",
            "state": "not_requested", "alive_at_request": None,
            "exit_before_request": None, "returncode_before_request": None,
            "kill_issued": False, "kill_error": None, "reap_confirmed": False,
            "exit_after_request": None, "hold_established": False, "cause": "not_requested"}


def stop_outcome(cancel, result):
    """Closed outcome from the observed stop lifecycle, never from a request time alone."""
    state = cancel["state"]
    if state in ("not_requested", "already_exited"):
        return result.get("outcome", "error") if isinstance(result, dict) else "error"
    if state == "stop_confirmed":
        return "timeout" if cancel.get("reason") == "timeout" else "cancelled"
    return "not_verified"


class Worker:
    def __init__(self, spec, work, pwsh, fixtures, limits, roi=None, max_roi_output=None):
        self.cid, self.fid, self.kind, self.hold = spec
        self.fixture = fixtures[self.fid]
        self.work, self.pwsh = work, pwsh
        self.root = work / "workers" / self.cid
        self.root.mkdir(parents=True)
        self.observe = self.root / "observe"
        self.observe.mkdir()
        self.output = self.root / "output.png"
        self.stdout_path, self.stderr_path = self.root / "stdout.json", self.root / "stderr.txt"
        self.stdout, self.stderr = self.stdout_path.open("xb"), self.stderr_path.open("xb")
        argv = [str(pwsh), "-NoProfile", "-File", str(PROBE), "-InputPath",
                str(work / "cases" / self.fixture["path"]), "-OutputPath", str(self.output)]
        for key, value in limits.items():
            argv += ["-" + {"max_pixels": "MaxPixels", "max_dimension": "MaxDimension",
                           "max_bit_depth": "MaxBitDepth", "max_decoded_bytes": "MaxDecodedBytes",
                           "max_scratch_bytes": "MaxScratchBytes"}[key], str(value)]
        argv += ["-ObservationRoot", str(self.observe), "-TraceMaxRecords", str(BOUNDS["max_trace_records"]),
                 "-TraceMaxBytes", str(BOUNDS["max_trace_bytes"])]
        if roi is not None:
            argv += ["-Roi", roi, "-MaxRoiOutputBytes", str(max_roi_output)]
        if self.hold:
            argv += ["-PauseAtStage", self.hold, "-PauseTimeoutMs", "10000"]
        self.started = time.monotonic_ns()
        try:
            self.proc = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=self.stdout, stderr=self.stderr)
        except OSError:
            self.stdout.close()
            self.stderr.close()
            raise
        self.identity_verified = False
        self.cancel = cancel_record(self.kind)
        self.released = False
        self.rows = []

    def read_trace(self):
        path = self.observe / "trace.jsonl"
        if not path.exists():
            return []
        if path.stat().st_size > BOUNDS["max_trace_bytes"]:
            raise ValueError("trace_cap_exceeded")
        data = path.read_bytes()
        prefix = data[:data.rfind(b"\n") + 1]
        self.rows = [json.loads(line) for line in prefix.splitlines()]
        if len(self.rows) > BOUNDS["max_trace_records"]:
            raise ValueError("trace_record_cap_exceeded")
        return self.rows

    def identity(self):
        self.read_trace()
        if not self.rows or self.rows[0].get("pid") != self.proc.pid:
            raise ValueError("worker_identity_unavailable")
        return {"case_id": self.cid, "pid": self.proc.pid,
                "start_utc_ticks": self.rows[0]["process_start_utc_ticks"]}

    def at(self, stage):
        return stage in [r.get("stage") for r in self.read_trace()]

    def release(self):
        if not self.released:
            with (self.observe / "continue").open("xb"):
                pass
            self.released = True

    def stop(self, reason):
        """Request termination and record what was actually observed.

        A child that has already exited keeps its own result: the request is not
        a cancellation. A successful ``kill()`` says only that a termination was
        requested, so a stop is confirmed as cancellation evidence only when the
        child was parked in an operator boundary hold at the request: there it
        cannot publish or finish on its own. Anything else, including an
        unattributable kill/natural-exit race, stays ``unverified_race``.
        """
        if self.cancel["requested_ns"] is not None or self.cancel["state"] == "already_exited":
            return
        self.read_trace()
        before = self.proc.poll()
        last = self.rows[-1] if self.rows else None
        hold_established = bool(self.hold and last and last.get("stage") == self.hold
                                and last.get("artificial_instrumentation_hold") is True)
        self.cancel.update(last_observed_stage=last["stage"] if last else None, reason=reason,
                           hold_established=hold_established,
                           cause="boundary_hold" if hold_established else "unattributable")
        if before is not None:
            self.cancel.update(state="already_exited", alive_at_request=False,
                               exit_before_request=True, returncode_before_request=before,
                               reap_confirmed=True, waited=True)
            return
        self.cancel.update(state="unverified_race", alive_at_request=True, exit_before_request=False,
                           requested_ns=time.monotonic_ns(), method="direct_child_terminate_wait")
        try:
            self.proc.kill()
        except OSError as exc:
            self.cancel.update(kill_error=type(exc).__name__)
        else:
            self.cancel.update(kill_issued=True)
        self.proc.wait(timeout=5)
        confirmed = (self.cancel["kill_issued"] and hold_established
                     and self.proc.returncode not in (None, 0))
        self.cancel.update(observed_ns=time.monotonic_ns(), waited=True,
                           exit_after_request=self.proc.returncode,
                           reap_confirmed=self.proc.returncode is not None,
                           state="stop_confirmed" if confirmed else "unverified_race",
                           reachability="NOT_MEASURED" if self.kind == "onload_attempt"
                           else ("reached" if confirmed else "not_requested"))

    def finish(self, exe_sha):
        self.proc.wait(timeout=5)
        exited = self.cancel["observed_ns"] or time.monotonic_ns()
        self.stdout.close()
        self.stderr.close()
        self.read_trace()
        trace = self.observe / "trace.jsonl"
        data = trace.read_bytes() if trace.exists() else b""
        stdout = self.stdout_path.read_bytes()
        try:
            result = json.loads(stdout.decode("utf-8")) if stdout else None
        except (UnicodeError, ValueError):
            result = None
        stopped = self.cancel["state"] == "stop_confirmed"
        removed = []
        if stopped:
            for path in (self.output, self.observe / "pending.png", self.observe / "pending.bmp"):
                if path.exists():
                    path.unlink()
                    removed.append(str(path.relative_to(self.work)))
        output_present = self.output.is_file()
        outcome = stop_outcome(self.cancel, result)
        identity = self.identity()
        return {"case_id": self.cid, "fixture_id": self.fid, "kind": self.kind, "hold_stage": self.hold,
                "pid": self.proc.pid, "start_utc_ticks": identity["start_utc_ticks"],
                "exe_sha256": exe_sha, "input_sha256": self.fixture["sha256"],
                "identity_verified": self.identity_verified,
                "clock_domain": "python_monotonic_ns", "cross_process_clock_subtraction": False,
                "started_ns": self.started, "exited_ns": exited,
                "exit_code": self.proc.returncode, "reaped": True, "alive_after_wait": self.proc.poll() is None,
                "stages": self.rows, "trace_path": str(trace.relative_to(self.work)),
                "trace_sha256": sha(data), "trace_bytes": len(data), "trace_records": len(self.rows),
                "trace_partial_bytes": len(data) - (data.rfind(b"\n") + 1),
                "trace_complete": bool(self.rows and self.rows[-1].get("stage") == "probe_exit"
                                       and self.rows[-1].get("complete") is True),
                "probe_result": result, "outcome": outcome, "cancellation": self.cancel,
                "output_present": output_present, "output_bytes": self.output.stat().st_size if output_present else 0,
                "output_sha256": file_sha(self.output) if output_present else None,
                "output_path": str(self.output.relative_to(self.work)),
                "published_success": outcome == "decoded" and not stopped and output_present,
                "removed_owned_output_paths": removed, "stdout_bytes": len(stdout),
                "stderr_bytes": self.stderr_path.stat().st_size,
                "boundary_hold_is_instrumentation": bool(self.hold)}


def _snapshot(workers, pwsh):
    started = time.monotonic_ns()
    items = [w.identity() for w in workers]
    if any(w.proc.poll() is not None for w in workers):
        raise ValueError("sample_worker_already_exited")
    result = subprocess.run([str(pwsh), "-NoProfile", "-Command", SAMPLE_SCRIPT],
                            input=json.dumps({"exe": str(pwsh), "items": items}),
                            capture_output=True, text=True, timeout=3)
    ended = time.monotonic_ns()
    if result.returncode or len(result.stdout.encode("utf-8")) > BOUNDS["max_capture_bytes"]:
        raise ValueError("sampler_unavailable")
    observed = json.loads(result.stdout)
    samples = observed["samples"]
    if {(s["pid"], s["start_utc_ticks"]) for s in samples} != {(i["pid"], i["start_utc_ticks"]) for i in items}:
        raise ValueError("process_identity_unverified")
    if any(w.proc.poll() is not None for w in workers):
        raise ValueError("sampling_incomplete")
    for w in workers:
        w.identity_verified = True
    totals = {key: sum(s[key] for s in samples) for key in ("working_set_bytes", "private_bytes")}
    if totals["private_bytes"] > BOUNDS["max_observed_private_bytes"]:
        raise ValueError("observed_private_budget_exceeded")
    return {"case_ids": [w.cid for w in workers], "start_ns": started, "end_ns": ended,
            "clock_domain": "python_monotonic_ns", "observer": "parent plus managed Process sampler",
            "inner_observer_clock": "windows_qpc", "inner_observer_qpc_start": observed["qpc_start"],
            "inner_observer_qpc_end": observed["qpc_end"], "inner_observer_qpc_frequency": observed["qpc_frequency"],
            "complete": True, "status": "sampled_lower_bound" if len(workers) == 1 else "derived_window_sum", "atomic_snapshot": False,
            "aggregation": "same_collection_window", "samples": samples, "totals": totals,
            "limitations": ["sequential counter reads within a bounded window, not an atomic OS snapshot",
                            "explicit post-OnLoad or pre-cancel boundary hold; not decoder execution time",
                            "whole managed worker process includes runtime, libraries and observer effects"]}


def _guard(work, started, workers):
    if time.monotonic_ns() - started > BOUNDS["max_total_seconds"] * 1_000_000_000:
        raise ValueError("total_deadline_exceeded")
    if _work_bytes(work) > BOUNDS["max_work_bytes"]:
        raise ValueError("work_cap_exceeded")
    for w in workers:
        for p in (w.stdout_path, w.stderr_path):
            if p.stat().st_size > BOUNDS["max_capture_bytes"]:
                raise ValueError("capture_cap_exceeded")
        if w.output.exists() and w.output.stat().st_size > BOUNDS["max_output_bytes"]:
            raise ValueError("output_cap_exceeded")
        if time.monotonic_ns() - w.started > BOUNDS["max_worker_seconds"] * 1_000_000_000:
            raise ValueError("worker_deadline_exceeded")


def _run_group(specs, work, pwsh, inventory, limits, run_started, cohorts):
    workers = []
    try:
        for spec in specs:
            workers.append(Worker(spec, work, pwsh, inventory, limits))
        if specs[0][2] != "refusal":
            while not all(w.at(w.hold) for w in workers):
                _guard(work, run_started, workers)
                if any(w.proc.poll() is not None for w in workers):
                    raise ValueError("worker_exited_before_required_boundary")
                time.sleep(.002)
            _guard(work, run_started, workers)
            cohorts.append(_snapshot(workers, pwsh))
            for w in workers:
                if w.kind == "cancel":
                    w.stop("cancel")
                elif w.kind != "timeout":
                    w.release()
        while any(w.proc.poll() is None for w in workers):
            _guard(work, run_started, workers)
            for w in workers:
                if w.proc.poll() is not None:
                    continue
                if w.kind == "timeout" and time.monotonic_ns() - w.started >= 5_000_000_000:
                    w.stop("timeout")
                elif w.kind == "onload_attempt":
                    names = [r["stage"] for r in w.read_trace()]
                    if "onload_entered" in names and "onload_returned" not in names:
                        w.stop("cancel")
            time.sleep(.002)
        return [w.finish(file_sha(pwsh)) for w in workers]
    finally:
        for w in workers:
            if w.proc.poll() is None:
                w.proc.kill()
            w.proc.wait(timeout=5)
            w.stdout.close()
            w.stderr.close()


def _metric(value, source, method, status, count, complete, limitations):
    return {"value": value, "unit": "bytes", "source": source, "method": method,
            "status": status, "sample_count": count, "complete": complete, "limitations": limitations}


def _memory(workers, cohorts):
    admissions = [next(s for s in w["stages"] if s["stage"] == "admitted")
                  for w in workers if w["kind"] == "cohort"]
    fields = {"encoded_input_bytes": "input_bytes", "admitted_decoded_bytes": "decoded_bytes_required",
              "bgra32_destination_bytes": "bgra32_bytes_required", "declared_scratch_bytes": "declared_scratch_bytes"}
    memory = {name: _metric(sum(a[field] for a in admissions), "three concurrent fixture/admission records",
                           "sum of explicit known requirements for this declared cohort", "derived", 3, True,
                           ["logical required bytes, not a measured reservation or allocator peak"])
              for name, field in fields.items()}
    memory["explicit_known_requirements_bytes"] = _metric(sum(x["value"] for x in memory.values()),
        "encoded + source raster + BGRA destination + explicit scratch", "checked integer sum", "derived", 3, True,
        ["not an upper bound on WIC-internal allocations or managed runtime memory"])
    for name in ("working_set_bytes", "private_bytes", "concurrent_working_set_bytes", "concurrent_private_bytes"):
        selected = [c for c in cohorts if len(c["case_ids"]) == (3 if name.startswith("concurrent_") else 1)]
        key = "private_bytes" if "private" in name else "working_set_bytes"
        memory[name] = _metric(max(c["totals"][key] for c in selected), "System.Diagnostics.Process counters",
            "maximum complete same-collection-window total, never sum of independent maxima",
            "derived" if name.startswith("concurrent_") else "sampled_lower_bound", len(selected), True,
            ["not true peak or an instantaneous aggregate bound; sequential window; explicit holds; includes runtime/shared pages"])
    for name in ("decoder_internal_scratch_peak", "actually_proven_peak_concurrent_allocation"):
        memory[name] = _metric(None, "WIC internals", "not observable through whole-process counters", "not_measured", 0, False,
                               ["no internal allocation attribution or true concurrent allocation peak endpoint"])
    memory["explicit_known_reservations_bytes"] = _metric(None, "no separately owned allocator reservation ledger",
        "logical admission requirements are not allocator reservations", "not_measured", 0, False,
        ["do not infer actual reservations from raster arithmetic or process counters"])
    if set(memory) != set(MEMORY_NAMES):
        raise ValueError("memory_contract_mismatch")
    return memory


def run_matrix(work_root, out, pwsh, harness):
    work, report_path = _safe_path(work_root), _safe_path(out)
    pwsh, harness = _safe_path(pwsh), _safe_path(harness)
    if work.exists() or report_path.exists() or not pwsh.is_file() or not harness.is_file():
        raise ValueError("fresh_work_and_output_and_real_executables_required")
    if report_path.is_relative_to(work) or any(x in str(work) for x in ("'", "\r", "\n")):
        raise ValueError("unsafe_report_location_or_fixture_path")
    if len(SCENARIOS) > BOUNDS["max_cases"]:
        raise ValueError("matrix_case_cap")
    started = time.monotonic_ns()
    source = _source_identity(pwsh, harness)
    work.mkdir(parents=True, exist_ok=False)
    fixtures = _fixtures(work, pwsh)
    inventory = {x["case_id"]: x for x in fixtures}
    consumer_path = work / "consumer.json"
    argv = [str(harness), "--cases", str(work / "cases"), "--out", str(consumer_path),
            "--candidate", f"wic=pwsh:{PROBE.as_posix()}", "--output-policy", "canonical-png",
            "--max-cases", "8", "--max-observations", "8", "--timeout-ms", "45000",
            "--max-output-file-bytes", str(BOUNDS["max_output_bytes"]), "--sample-interval-ms", "0"]
    for name, value in OVERRIDES.items():
        argv += ["--" + name.replace("_", "-"), str(value)]
    env = dict(os.environ)
    env["PATH"] = str(pwsh.parent) + os.pathsep + env.get("PATH", "")
    with (work / "consumer.stdout").open("xb") as stdout, (work / "consumer.stderr").open("xb") as stderr:
        completed = subprocess.run(argv, stdout=stdout, stderr=stderr, env=env, timeout=120)
    if completed.returncode or not consumer_path.is_file() or consumer_path.stat().st_size > BOUNDS["max_report_bytes"]:
        raise ValueError("real_consumer_failed_or_record_capped")
    consumer = json.loads(consumer_path.read_text(encoding="utf-8"))
    budget = consumer["decode_budget"]
    limits = {k: budget[k] for k in OVERRIDES}
    cohorts, workers = [], []
    singles = [s for s in SCENARIOS if s[2] != "cohort"]
    for spec in singles:
        workers.extend(_run_group([spec], work, pwsh, inventory, limits, started, cohorts))
    workers.extend(_run_group([s for s in SCENARIOS if s[2] == "cohort"], work, pwsh, inventory, limits, started, cohorts))
    after = _source_identity(pwsh, harness)
    for key, item in source["files"].items():
        item["unchanged_after_run"] = item["raw_sha256"] == after["files"][key]["raw_sha256"]
    if source["pwsh_sha256"] != after["pwsh_sha256"] or source["harness_sha256"] != after["harness_sha256"]:
        raise ValueError("executable_changed_during_run")
    public_consumer = redact_paths(consumer, [(work, "<work-root>"), (ROOT, "<repository>")])
    record = {"schema_version": 1, "run_id": work.name, "bounds": BOUNDS, "source_identity": source,
              "fixtures": fixtures, "expected_case_ids": sorted(FIXTURES),
              "decode_limits": {"limits": limits, "narrow_overrides": OVERRIDES,
                                "source": "actual existing decode_compare DecodeBudget::resolve report",
                                "basis": "ratified defaults declared/not measured; this experiment uses narrower overrides"},
              "consumer": {"exit_code": completed.returncode, "complete": True, "record": public_consumer,
                           "canonical_record_sha256": sha(canonical(public_consumer)),
                           "original_canonical_sha256": sha(canonical(consumer)),
                           "raw_record_sha256": file_sha(consumer_path),
                           "representation": "owned absolute path prefixes redacted; all measurements/integers unchanged"},
              "workers": workers, "cohorts": cohorts, "memory": _memory(workers, cohorts),
              "cleanup": {"complete": all(w["reaped"] and not w["alive_after_wait"] for w in workers),
                          "live_direct_children": [], "cancelled_output_paths_remaining": [w["output_path"] for w in workers
                              if w["cancellation"]["state"] == "stop_confirmed" and w["output_present"]]},
              "work_bytes": _work_bytes(work), "duration_ns": time.monotonic_ns() - started,
              "reproducibility": {"structural_oracle": "fixed fixtures/cases/admission arithmetic/consumer pixels",
                  "nondeterministic": ["OS counters", "process identity", "timestamps", "inside-OnLoad attempt reachability"],
                  "unmeasured": ["WIC internal scratch peak", "true concurrent allocation peak", "cooperative WIC cancellation",
                                 "16K full decode", "Bgr565 Native", "region/sequential/tiled/colour/orientation", "JPEG absolute raster"]}}
    data = canonical(record)
    if len(data) > BOUNDS["max_report_bytes"]:
        raise ValueError("report_cap_exceeded")
    report_path.parent.mkdir(parents=True, exist_ok=True)
    with report_path.open("xb") as stream:
        stream.write(data)
    return record


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="op", required=True)
    run = commands.add_parser("run")
    for flag in ("work-root", "out", "pwsh", "harness"):
        run.add_argument("--" + flag, required=True)
    check = commands.add_parser("verify")
    check.add_argument("--report", required=True)
    check.add_argument("--work-root")
    check.add_argument("--source-root")
    args = parser.parse_args(argv)
    try:
        if args.op == "run":
            record = run_matrix(args.work_root, args.out, args.pwsh, args.harness)
            result = verify(record, args.work_root, ROOT)
        else:
            result = verify_file(args.report, args.work_root, args.source_root)
        print(json.dumps(result, sort_keys=True))
        return 0 if result["ok"] else 1
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as exc:
        # Never copy private paths, child stderr or arbitrary metadata to stdout.
        token = str(exc)
        reason = token if re.fullmatch(r"[a-z_]{1,80}", token) else type(exc).__name__
        print(json.dumps({"ok": False, "reason": reason}))
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
