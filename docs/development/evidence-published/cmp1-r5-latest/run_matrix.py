"""Run the fixed MP-CMP1 R5 5-run x 4-variant reuse/fairness matrix.

Read-only with respect to the product source. Each run is the existing
`examples/compare.exe --fixture --trace [variant]` harness under an isolated
LOCALAPPDATA/APPDATA/TEMP tree, serialized by the shared native lock. Raw stdout
(the trace ledger line(s)) stays in target/ as private scratch; only per-run
summaries and their digest are published.
"""
import hashlib
import json
import os
import shutil
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

REPO = Path(__file__).resolve().parents[4]  # docs/development/evidence-published/cmp1-r5-latest
EXE = REPO / "target/release/examples/compare.exe"
SCRATCH = REPO / "target/cmp1-r5"
RAW = SCRATCH / "raw"
PROFILES = SCRATCH / "profiles"
EVIDENCE = REPO / "docs/development/evidence-published/cmp1-r5-latest"
LOCK = Path(r"C:\tmp\cos-desktop-native.lock")
VARIANT_FLAGS = [("shared", ["--shared"]), ("independent", ["--independent"]),
                 ("distinct", ["--distinct"]), ("windows2", ["--windows2"])]
RUNS = 5
TIMEOUT_S = 180
MAX_ERR = 64 * 1024


def sha256_bytes(data):
    return "sha256:" + hashlib.sha256(data).hexdigest()


def acquire_lock():
    owner = f"impl-r0a {os.getpid()}"
    deadline = time.monotonic() + 1800
    while True:
        try:
            LOCK.mkdir(parents=False, exist_ok=False)
            break
        except FileExistsError:
            age = time.time() - LOCK.stat().st_mtime
            pid = None
            owner_file = LOCK / "OWNER.txt"
            if owner_file.is_file():
                for token in owner_file.read_text(encoding="utf-8", errors="replace").split():
                    if token.startswith("pid=") and token[4:].isdigit():
                        pid = int(token[4:])
            alive = False
            if pid is not None:
                listed = subprocess.run(["tasklist", "/FI", f"PID eq {pid}", "/NH"],
                                        capture_output=True, text=True,
                                        creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
                alive = str(pid) in listed.stdout
            if age > 300 and not alive:
                shutil.rmtree(LOCK, ignore_errors=True)
                continue
            if time.monotonic() > deadline:
                raise SystemExit("native lock is held by another run")
            time.sleep(5)
        except OSError as error:
            raise SystemExit(f"cannot create native lock: {error}")
    (LOCK / "OWNER.txt").write_text(
        f"owner=impl-r0a pid={os.getpid()} at={datetime.now(timezone.utc).isoformat()}\n",
        encoding="utf-8")


def release_lock():
    if LOCK.is_dir():
        shutil.rmtree(LOCK, ignore_errors=True)


def run_once(variant, run):
    profile = PROFILES / f"{variant}-{run}"
    if profile.exists():
        raise SystemExit(f"profile already exists: {profile}")
    for name in ("local", "roaming", "temp"):
        (profile / name).mkdir(parents=True, exist_ok=False)
    env = os.environ.copy()
    env.update({
        "LOCALAPPDATA": str(profile / "local"),
        "APPDATA": str(profile / "roaming"),
        "TEMP": str(profile / "temp"),
        "TMP": str(profile / "temp"),
        "PYTHONIOENCODING": "utf-8",
    })
    command = [str(EXE), "--fixture", "--trace", *dict(VARIANT_FLAGS)[variant]]
    started = datetime.now(timezone.utc)
    timed_out = False
    try:
        result = subprocess.run(command, cwd=REPO, env=env, capture_output=True,
                                timeout=TIMEOUT_S,
                                creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
        stdout, stderr, code = result.stdout, result.stderr, result.returncode
    except subprocess.TimeoutExpired as error:
        timed_out = True
        stdout = error.stdout or b""
        stderr = error.stderr or b""
        code = None
    elapsed_ms = int((datetime.now(timezone.utc) - started).total_seconds() * 1000)
    raw_path = RAW / f"{variant}-run{run}.jsonl"
    raw_path.write_bytes(stdout)
    (RAW / f"{variant}-run{run}.stderr.txt").write_bytes(stderr[:MAX_ERR])
    lines = [line for line in stdout.splitlines() if line.strip()]
    parsed = []
    parse_errors = 0
    for line in lines:
        try:
            value = json.loads(line)
        except ValueError:
            parse_errors += 1
            continue
        if isinstance(value, dict) and value.get("scenario") == "cmp1_native_fixed_trace":
            parsed.append(value)
    summary = {
        "variant": variant,
        "run": run,
        "command": ["examples/compare.exe", "--fixture", "--trace",
                    *dict(VARIANT_FLAGS)[variant]],
        "started_utc": started.isoformat(),
        "elapsed_ms": elapsed_ms,
        "exit_code": code,
        "timed_out": timed_out,
        "trace_lines": len(parsed),
        "raw_lines": len(lines),
        "parse_errors": parse_errors,
        "stderr_bytes": len(stderr),
        "raw_sha256": sha256_bytes(stdout),
        "raw_bytes": len(stdout),
        "traces": [],
    }
    for trace in parsed:
        summary["traces"].append({
            "trace_variant": trace.get("variant"),
            "ready": trace.get("ready"),
            "warmup_ms": trace.get("warmup_ms"),
            "samples": trace.get("samples"),
            "refusals": trace.get("refusals"),
            "settled": trace.get("settled"),
            "cancel_probe_stage": trace.get("cancel_probe_stage"),
            "decode_calls": trace.get("decode_calls"),
            "reuse_calls": trace.get("reuse_calls"),
            "retained_frames": trace.get("retained_frames"),
            "consumers": trace.get("consumers"),
            "packed_retained_bytes": trace.get("packed_retained_bytes"),
            "cpu_retained_bytes": trace.get("cpu_retained_bytes"),
            "resident_reservation_bytes": trace.get("resident_reservation_bytes"),
            "queue_wait_ns": trace.get("queue_wait_ns"),
            "queue_rejections": trace.get("queue_rejections"),
            "queue_cancellations": trace.get("queue_cancellations"),
            "sampled_peak_cpu_retained_bytes": trace.get("sampled_peak_cpu_retained_bytes"),
            "sampled_peak_packed_bytes": trace.get("sampled_peak_packed_bytes"),
            "sampled_peak_reservation_bytes": trace.get("sampled_peak_reservation_bytes"),
            "input_decoded_rgba_sha256": trace.get("input_decoded_rgba_sha256"),
            "present": trace.get("present"),
            "gpu": trace.get("gpu"),
            "ledger_rows": len(trace.get("ledger") or []),
        })
    return summary


def main():
    if not EXE.is_file():
        raise SystemExit("release compare.exe is absent; build it first")
    EVIDENCE.mkdir(parents=True, exist_ok=True)
    RAW.mkdir(parents=True, exist_ok=True)
    PROFILES.mkdir(parents=True, exist_ok=True)
    runs_path = EVIDENCE / "runs.json"
    if runs_path.exists():
        raise SystemExit("runs.json already exists; refusing to overwrite evidence")
    summaries = []
    acquire_lock()
    try:
        for variant, _ in VARIANT_FLAGS:
            for run in range(1, RUNS + 1):
                summary = run_once(variant, run)
                summaries.append(summary)
                print(f"{variant} run {run}: exit={summary['exit_code']} "
                      f"lines={summary['trace_lines']} samples="
                      f"{[trace['samples'] for trace in summary['traces']]} "
                      f"decode={[trace['decode_calls'] for trace in summary['traces']]} "
                      f"retained={[trace['retained_frames'] for trace in summary['traces']]} "
                      f"consumers={[trace['consumers'] for trace in summary['traces']]}",
                      flush=True)
                runs_path.write_text(json.dumps(
                    {"task": "MP-CMP1", "slice": "R5 reuse/fairness matrix", "runs": summaries},
                    indent=2) + "\n", encoding="utf-8")
                time.sleep(1)
    finally:
        release_lock()
    print("MATRIX_DONE", len(summaries))


if __name__ == "__main__":
    main()
