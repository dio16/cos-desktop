"""Bounded process-memory observations for the IMG2 WIC matrix.

This is an experiment observer, not a product memory budget or decoder API.
"""

from __future__ import annotations

import hashlib
import json
import subprocess
import threading
import time
from pathlib import Path
from typing import Any, TypeGuard


_CAPTURE_LIMIT = 64 * 1024
_OBSERVER_TIMEOUT_SECONDS = 3.0

# This is intentionally fixed trusted PowerShell. It receives only the worker
# PID/lifetime tuples captured by W2's Worker.identity() method.
SAMPLE_SCRIPT = r"""
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$spec = [Console]::In.ReadToEnd() | ConvertFrom-Json
$rows = [System.Collections.Generic.List[object]]::new()
$errors = [System.Collections.Generic.List[object]]::new()
$begin = [Diagnostics.Stopwatch]::GetTimestamp()
$expectedExe = [IO.Path]::GetFullPath([string]$spec.exe)
foreach ($item in @($spec.items)) {
  $p = $null
  $row = $null
  $stage = 'open_process'
  try {
    $p = [Diagnostics.Process]::GetProcessById([int]$item.pid)
    $stage = 'identity_before'
    $p.Refresh()
    $before = $p.StartTime.ToUniversalTime().Ticks
    $aliveBefore = -not $p.HasExited
    $actualExe = [IO.Path]::GetFullPath($p.MainModule.FileName)
    $exeVerified = [StringComparer]::OrdinalIgnoreCase.Equals($actualExe, $expectedExe)
    if ($before -ne [long]$item.start_utc_ticks -or -not $aliveBefore -or -not $exeVerified) {
      throw 'identity_before'
    }

    $row = [ordered]@{
      case_id = [string]$item.case_id
      pid = [int]$p.Id
      start_utc_ticks = [long]$before
      identity_after_ticks = $null
      exe_verified = [bool]$exeVerified
      alive_before = [bool]$aliveBefore
      alive_after = $null
      working_set_bytes = $null
      private_bytes = $null
      peak_working_set_bytes = $null
      peak_status = 'NOT_MEASURED'
      order = [int]$item.order
      qpc_start = $null
      qpc_end = $null
    }

    $stage = 'counters'
    $row.qpc_start = [long][Diagnostics.Stopwatch]::GetTimestamp()
    try {
      $row.working_set_bytes = [long]$p.WorkingSet64
      $row.private_bytes = [long]$p.PrivateMemorySize64
    } finally {
      $row.qpc_end = [long][Diagnostics.Stopwatch]::GetTimestamp()
    }
    try {
      $row.peak_working_set_bytes = [long]$p.PeakWorkingSet64
      $row.peak_status = 'MEASURED_PROCESS_LIFETIME_COUNTER'
    } catch {
      # PeakWorkingSet64 is optional and is not a window-local measure.
      $row.peak_working_set_bytes = $null
      $row.peak_status = 'NOT_MEASURED'
    }

    $stage = 'identity_after'
    try {
      $p.Refresh()
      $after = [long]$p.StartTime.ToUniversalTime().Ticks
      $aliveAfter = -not $p.HasExited
      $row.identity_after_ticks = $after
      $row.alive_after = [bool]$aliveAfter
      if ($after -ne $before -or -not $aliveAfter) {
        $errors.Add([ordered]@{case_id=[string]$item.case_id;pid=[int]$item.pid;stage='identity_after';error='lifetime_changed'})
      }
    } catch {
      $row.identity_after_ticks = $null
      $row.alive_after = $null
      $errors.Add([ordered]@{case_id=[string]$item.case_id;pid=[int]$item.pid;stage='identity_after';error='observation_failed'})
    }
  } catch {
    $errors.Add([ordered]@{case_id=[string]$item.case_id;pid=[int]$item.pid;stage=[string]$stage;error='observation_failed'})
  } finally {
    if ($null -ne $row) { $rows.Add($row) }
    if ($null -ne $p) { $p.Dispose() }
  }
}
$end = [Diagnostics.Stopwatch]::GetTimestamp()
$document = [ordered]@{
  samples = @($rows.ToArray())
  errors = @($errors.ToArray())
  qpc_start = [long]$begin
  qpc_end = [long]$end
  qpc_frequency = [long][Diagnostics.Stopwatch]::Frequency
}
[Console]::Out.WriteLine((ConvertTo-Json -InputObject $document -Compress -Depth 8))
if ($errors.Count -gt 0) { exit 1 }
exit 0
"""


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def _is_int(value: Any) -> TypeGuard[int]:
    return type(value) is int


def _capture_process(
    executable: Path, payload: bytes, started_at: float
) -> tuple[int | None, bytes, bytes, int, int, bool, bool, str | None]:
    stdout_buffer = bytearray()
    stderr_buffer = bytearray()
    counts = {"stdout": 0, "stderr": 0}
    capture_errors: list[str] = []

    def drain(stream: Any, name: str, buffer: bytearray) -> None:
        try:
            while True:
                block = stream.read(8192)
                if not block:
                    return
                counts[name] += len(block)
                available = _CAPTURE_LIMIT - len(buffer)
                if available > 0:
                    buffer.extend(block[:available])
        except OSError:
            capture_errors.append(name)

    try:
        process = subprocess.Popen(
            [str(executable), "-NoProfile", "-NonInteractive", "-Command", SAMPLE_SCRIPT],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    except OSError as exc:
        return None, b"", b"", 0, 0, False, False, f"observer_spawn_failed:{type(exc).__name__}"

    assert process.stdin is not None and process.stdout is not None and process.stderr is not None
    stdout_reader = threading.Thread(target=drain, args=(process.stdout, "stdout", stdout_buffer), daemon=True)
    stderr_reader = threading.Thread(target=drain, args=(process.stderr, "stderr", stderr_buffer), daemon=True)
    stdout_reader.start()
    stderr_reader.start()

    io_failure: str | None = None
    try:
        process.stdin.write(payload)
        process.stdin.close()
    except (BrokenPipeError, OSError) as exc:
        io_failure = f"observer_stdin_failed:{type(exc).__name__}"
        try:
            process.stdin.close()
        except OSError:
            pass

    timed_out = False
    try:
        elapsed = time.monotonic() - started_at
        remaining = max(0.001, _OBSERVER_TIMEOUT_SECONDS - elapsed)
        process.wait(timeout=remaining)
    except subprocess.TimeoutExpired:
        timed_out = True
        try:
            process.kill()
        except OSError:
            pass
        process.wait(timeout=1)

    stdout_reader.join(timeout=.5)
    stderr_reader.join(timeout=.5)
    if stdout_reader.is_alive() or stderr_reader.is_alive():
        capture_errors.append("reader_deadline")
    process.stdout.close()
    process.stderr.close()
    return (
        process.returncode,
        bytes(stdout_buffer),
        bytes(stderr_buffer),
        counts["stdout"],
        counts["stderr"],
        timed_out,
        not timed_out and not capture_errors,
        io_failure or ("observer_capture_failed" if capture_errors else None),
    )


def capture_window(
    workers: list[Any],
    pwsh: Path,
    index: int,
    previous_end_ns: int | None = None,
) -> dict[str, Any]:
    """Capture one bounded sequential Process-counter collection window."""
    started = time.monotonic_ns()
    case_ids = [worker.cid for worker in workers]
    failures: list[str] = []
    if not workers:
        failures.append("sample_window_empty")
    request_identities: list[dict[str, Any]] = []
    items: list[dict[str, Any]] = []
    parent_alive_before: list[bool] = []
    parent_alive_after: list[bool] = []

    identity_order_by_key: dict[tuple[str, int, int], int] = {}
    for worker_order, worker in enumerate(workers):
        case_id = worker.cid
        try:
            identity = worker.identity()
            identity_case_id = identity.get("case_id")
            pid = identity.get("pid")
            start_ticks = identity.get("start_utc_ticks")
            if not isinstance(identity_case_id, str) or not _is_int(pid) or pid <= 0 or not _is_int(start_ticks) or start_ticks <= 0:
                failures.append(f"worker_identity_invalid:{case_id}")
                continue
            requested = {"case_id": identity_case_id, "pid": pid, "start_utc_ticks": start_ticks}
            request_identities.append(requested)
            identity_order_by_key[(identity_case_id, pid, start_ticks)] = worker_order
            if identity_case_id != case_id or pid != worker.proc.pid:
                failures.append(f"worker_identity_invalid:{case_id}")
                continue
            items.append({**requested, "order": worker_order})
        except Exception as exc:
            failures.append(f"worker_identity_unavailable:{case_id}:{type(exc).__name__}")

    for worker in workers:
        try:
            alive = worker.proc.poll() is None
        except Exception as exc:
            alive = False
            failures.append(f"parent_poll_before_failed:{worker.cid}:{type(exc).__name__}")
        parent_alive_before.append(alive)
        if not alive:
            failures.append(f"worker_exited_before_sample:{worker.cid}")

    executable: Path | None = None
    exe_digest_before: str | None = None
    try:
        executable = Path(pwsh).resolve(strict=True)
        exe_digest_before = _sha256_file(executable)
    except (OSError, RuntimeError, TypeError) as exc:
        failures.append(f"observer_executable_unavailable:{type(exc).__name__}")

    observer_stdout = ""
    observer_stderr = ""
    stdout_bytes = 0
    stderr_bytes = 0
    capture_complete = False
    timed_out = False
    exit_code: int | None = None
    observed: dict[str, Any] | None = None
    observer_io_failure: str | None = None

    if executable is not None:
        try:
            payload = json.dumps({"exe": str(executable), "items": items}, separators=(",", ":")).encode("utf-8")
            started_at = time.monotonic()
            (
                exit_code,
                raw_stdout,
                raw_stderr,
                stdout_bytes,
                stderr_bytes,
                timed_out,
                capture_complete,
                observer_io_failure,
            ) = _capture_process(executable, payload, started_at)
            observer_stdout = raw_stdout.decode("utf-8", errors="replace")
            observer_stderr = raw_stderr.decode("utf-8", errors="replace")
            if timed_out:
                failures.append("observer_timeout")
            if observer_io_failure:
                failures.append(observer_io_failure)
            if stdout_bytes > _CAPTURE_LIMIT or stderr_bytes > _CAPTURE_LIMIT:
                capture_complete = False
                failures.append("observer_capture_truncated")
            if exit_code != 0:
                failures.append("observer_exit_nonzero")
            try:
                candidate = json.loads(observer_stdout)
                if isinstance(candidate, dict):
                    observed = candidate
                else:
                    failures.append("observer_json_invalid")
            except (json.JSONDecodeError, UnicodeError):
                failures.append("observer_json_invalid")
        except (OSError, ValueError, TypeError) as exc:
            failures.append(f"observer_launch_failed:{type(exc).__name__}")

    try:
        exe_digest_after = _sha256_file(executable) if executable is not None else None
    except OSError as exc:
        exe_digest_after = None
        failures.append(f"observer_executable_recheck_failed:{type(exc).__name__}")
    if exe_digest_before is not None and exe_digest_after != exe_digest_before:
        failures.append("observer_executable_changed")

    samples: list[Any] = []
    qpc_start: int | None = None
    qpc_end: int | None = None
    qpc_frequency: int | None = None
    if observed is not None:
        raw_samples = observed.get("samples")
        if isinstance(raw_samples, list):
            samples = raw_samples
        else:
            failures.append("observer_samples_invalid")
        for key, destination in (
            ("qpc_start", "start"),
            ("qpc_end", "end"),
            ("qpc_frequency", "frequency"),
        ):
            value = observed.get(key)
            if _is_int(value):
                if destination == "start":
                    qpc_start = value
                elif destination == "end":
                    qpc_end = value
                else:
                    qpc_frequency = value
            else:
                failures.append(f"observer_{key}_invalid")
        raw_errors = observed.get("errors")
        if not isinstance(raw_errors, list):
            failures.append("observer_errors_invalid")
        elif raw_errors:
            failures.append("observer_worker_error")

    # The returned sample list is the raw JSON sample list, preserving every
    # available partial row exactly as emitted by PowerShell.
    required_sample_fields = {
        "case_id", "pid", "start_utc_ticks", "identity_after_ticks", "exe_verified",
        "alive_before", "alive_after", "working_set_bytes", "private_bytes",
        "peak_working_set_bytes", "peak_status", "order", "qpc_start", "qpc_end",
    }
    for sample in samples:
        if not isinstance(sample, dict):
            failures.append("observer_sample_invalid")
            continue
        if set(sample) != required_sample_fields:
            failures.append("observer_sample_schema_invalid")
        sample_case = sample.get("case_id")
        sample_pid = sample.get("pid")
        sample_ticks = sample.get("start_utc_ticks")
        if isinstance(sample_case, str) and _is_int(sample_pid) and _is_int(sample_ticks):
            expected_order = identity_order_by_key.get((sample_case, sample_pid, sample_ticks))
        else:
            expected_order = None
        sample_order = sample.get("order")
        if not _is_int(sample_order) or expected_order is None or sample_order != expected_order:
            failures.append("observer_order_invalid")
        for key in ("pid", "start_utc_ticks", "identity_after_ticks", "working_set_bytes", "private_bytes", "qpc_start", "qpc_end"):
            value = sample.get(key)
            if value is not None and not _is_int(value):
                failures.append(f"observer_sample_{key}_invalid")
        if sample.get("peak_working_set_bytes") is not None and not _is_int(sample.get("peak_working_set_bytes")):
            failures.append("observer_sample_peak_invalid")
        for key in ("exe_verified", "alive_before", "alive_after"):
            if sample.get(key) is not None and type(sample.get(key)) is not bool:
                failures.append(f"observer_sample_{key}_invalid")
        if not isinstance(sample_case, str) or sample.get("peak_status") not in ("MEASURED_PROCESS_LIFETIME_COUNTER", "NOT_MEASURED"):
            failures.append("observer_sample_peak_status_invalid")

    if qpc_start is None or qpc_end is None or qpc_frequency is None or qpc_start <= 0 or qpc_end < qpc_start or qpc_frequency <= 0:
        failures.append("observer_qpc_invalid")
    expected_pairs = [(entry["case_id"], entry["pid"], entry["start_utc_ticks"]) for entry in request_identities]
    actual_pairs = [
        (sample.get("case_id"), sample.get("pid"), sample.get("start_utc_ticks"))
        for sample in samples
        if isinstance(sample, dict)
    ]
    if actual_pairs != expected_pairs:
        failures.append("observer_identity_set_mismatch")

    for sample in samples:
        start = sample.get("qpc_start")
        end = sample.get("qpc_end")
        if (
            not _is_int(start)
            or not _is_int(end)
            or start <= 0
            or end < start
            or qpc_start is None
            or qpc_end is None
            or start < qpc_start
            or end > qpc_end
        ):
            failures.append("observer_sample_qpc_invalid")
        if sample.get("identity_after_ticks") != sample.get("start_utc_ticks"):
            failures.append("observer_lifetime_changed")
        if sample.get("exe_verified") is not True:
            failures.append("observer_executable_identity_failed")
        if sample.get("alive_before") is not True or sample.get("alive_after") is not True:
            failures.append("observer_lifetime_failed")
        working_set = sample.get("working_set_bytes")
        private_bytes = sample.get("private_bytes")
        if not _is_int(working_set) or working_set <= 0:
            failures.append("observer_working_set_invalid")
        if not _is_int(private_bytes) or private_bytes <= 0:
            failures.append("observer_private_bytes_invalid")

    for worker in workers:
        try:
            alive = worker.proc.poll() is None
        except Exception as exc:
            alive = False
            failures.append(f"parent_poll_after_failed:{worker.cid}:{type(exc).__name__}")
        parent_alive_after.append(alive)
        if not alive:
            failures.append(f"worker_exited_after_sample:{worker.cid}")

    if len(parent_alive_before) != len(workers) or len(parent_alive_after) != len(workers):
        failures.append("parent_lifetime_observation_incomplete")

    end_ns = time.monotonic_ns()
    gap_ns = None if previous_end_ns is None else started - previous_end_ns
    if gap_ns is not None and gap_ns < 0:
        failures.append("parent_clock_order_invalid")
    if not capture_complete:
        failures.append("observer_capture_incomplete")

    def hash_label(hex_digest: str | None) -> str | None:
        return f"sha256:{hex_digest}" if hex_digest is not None else None

    complete = not failures
    totals = None
    if complete:
        totals = {
            "working_set_bytes": sum(sample["working_set_bytes"] for sample in samples),
            "private_bytes": sum(sample["private_bytes"] for sample in samples),
        }
    # Stable de-duplication keeps failure summaries bounded and readable.
    failure = ";".join(dict.fromkeys(failures)) if failures else None
    return {
        "index": index,
        "case_ids": case_ids,
        "start_ns": started,
        "end_ns": end_ns,
        "clock_domain": "python_monotonic_ns",
        "gap_ns": gap_ns,
        "atomic_snapshot": False,
        "aggregation": "same_collection_window",
        "classification": "DERIVED_WINDOW_SUM",
        "complete": complete,
        "status": "complete" if complete else "incomplete",
        "failure": failure,
        "observer": {
            "exit_code": exit_code,
            "exe_sha256": hash_label(exe_digest_before),
            "script_sha256": "sha256:" + hashlib.sha256(SAMPLE_SCRIPT.encode("utf-8")).hexdigest(),
            "stdout_utf8": observer_stdout,
            "stderr_utf8": observer_stderr,
            "stdout_bytes": stdout_bytes,
            "stderr_bytes": stderr_bytes,
            "capture_complete": capture_complete,
            "timeout": timed_out,
        },
        "request_identities": request_identities,
        "qpc_start": qpc_start,
        "qpc_end": qpc_end,
        "qpc_frequency": qpc_frequency,
        "samples": samples,
        "totals": totals,
    }
