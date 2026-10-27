"""Bounded real-WIC Bgr565 experiment; no product decoder or budget defaults."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import time

from w3_fixtures import generate
from w3_oracle import compare
from w3_verify import BOUNDS, POSITIVE, SOURCE_FILES, scenarios, verify
from w2_resource_matrix import Worker, _safe_path, _snapshot, _work_bytes
from w2_resource_verify import canonical, file_sha, redact_paths, sha

ROOT = Path(__file__).resolve().parents[4]
OVERRIDES = {"max_dimension": 32, "max_pixels": 1024, "max_bit_depth": 6,
             "max_decoded_bytes": 2048, "max_scratch_bytes": 4096}


def source_identity(pwsh, harness):
    files = {}
    for name in SOURCE_FILES:
        data = (ROOT / name).read_bytes()
        files[name] = {"bytes": len(data), "raw_sha256": sha(data),
                       "lf_sha256": sha(data.replace(b"\r\n", b"\n").replace(b"\r", b"\n"))}
    return {"files": files, "pwsh_sha256": file_sha(pwsh), "harness_sha256": file_sha(harness)}


def guard(work, started, worker=None):
    if time.monotonic_ns() - started > BOUNDS["max_total_seconds"] * 1_000_000_000:
        raise ValueError("total_deadline_exceeded")
    if _work_bytes(work) > BOUNDS["max_work_bytes"]:
        raise ValueError("work_byte_cap")
    if worker:
        for path in (worker.stdout_path, worker.stderr_path):
            if path.stat().st_size > BOUNDS["max_capture_bytes"]:
                raise ValueError("capture_cap")
        worker.read_trace()
        if worker.output.exists() and worker.output.stat().st_size > BOUNDS["max_output_bytes"]:
            raise ValueError("output_cap")
        if time.monotonic_ns() - worker.started > BOUNDS["max_worker_seconds"] * 1_000_000_000:
            raise ValueError("worker_deadline_exceeded")


def run_worker(spec, work, pwsh, inventory, started):
    # Reuse W2's closed stop state machine, exact argv transport and trace bounds.
    adapted = {k: dict(v, case_id=k, sha256="sha256:" + v["sha256"]) for k, v in inventory.items()}
    kind = spec["kind"]
    hold = "before_publish" if kind == "cancel" else "before_admission"
    worker = Worker((spec["id"], spec["fixture"], kind, hold), work, pwsh, adapted, spec["limits"])
    sample = None
    try:
        while worker.proc.poll() is None and not worker.at(hold):
            guard(work, started, worker)
            time.sleep(.002)
        if worker.proc.poll() is None:
            sample = _snapshot([worker], pwsh)
            if kind == "timeout":
                while time.monotonic_ns() - worker.started < 5_000_000_000:
                    guard(work, started, worker)
                    if worker.proc.poll() is not None:
                        raise ValueError("worker_exited_before_operator_timeout")
                    time.sleep(.002)
            if kind in ("cancel", "timeout"):
                worker.stop(kind)
            else:
                worker.release()
        while worker.proc.poll() is None:
            guard(work, started, worker)
            time.sleep(.002)
        row = worker.finish(file_sha(pwsh))
        row["limits"] = spec["limits"]
        row["os_identity_sample"] = sample
        row["trace_utf8"] = (worker.observe / "trace.jsonl").read_text(encoding="utf-8")
        row["stdout_utf8"] = worker.stdout_path.read_text(encoding="utf-8")
        row["output_hex"] = worker.output.read_bytes().hex() if worker.output.exists() else None
        row["oracle"] = compare((work / "cases" / inventory[spec["fixture"]]["path"]).read_bytes(),
                                worker.output.read_bytes()) if spec["fixture"] in POSITIVE and row["outcome"] == "decoded" else None
        return row
    finally:
        if worker.proc.poll() is None:
            worker.stop("cleanup")
        worker.stdout.close()
        worker.stderr.close()


def run(work, out, pwsh, harness):
    work, out, pwsh, harness = map(_safe_path, (work, out, pwsh, harness))
    if work.exists() or out.exists() or out.is_relative_to(work):
        raise ValueError("fresh_external_work_and_report_required")
    if not pwsh.is_file() or not harness.is_file():
        raise ValueError("real_executables_required")
    started = time.monotonic_ns()
    identity = source_identity(pwsh, harness)
    work.mkdir(parents=True, exist_ok=False)
    fixtures = generate(work / "cases")
    inventory = {f["id"]: f for f in fixtures}
    for f in fixtures:
        f["input_hex"] = (work / "cases" / f["path"]).read_bytes().hex()
    # The ordinary consumer receives only positive cases. Direct instrumented
    # workers below exercise malformed/control/budget/stop cases independently.
    consumer_cases = work / "consumer-cases"
    consumer_cases.mkdir()
    manifest = []
    for cid in POSITIVE:
        f = inventory[cid]
        (consumer_cases / f["path"]).write_bytes(bytes.fromhex(f["input_hex"]))
        manifest.append(f'{f["path"]}|expected:{f["width"]}x{f["height"]}|pixels:{f["expected_rgba_sha256"]}|decoded')
    (consumer_cases / "cases.txt").write_text("\n".join(manifest) + "\n", encoding="utf-8")
    consumer_file = work / "consumer.json"
    argv = [str(harness), "--cases", str(consumer_cases), "--candidate",
            "wic=pwsh:" + (ROOT / "apps/desktop/examples/decode_compare/wic_probe.ps1").as_posix(),
            "--out", str(consumer_file), "--output-policy", "canonical-png",
            "--max-cases", "5", "--max-observations", "5", "--timeout-ms", "45000",
            "--max-output-file-bytes", str(BOUNDS["max_output_bytes"]), "--sample-interval-ms", "0"]
    for key, value in OVERRIDES.items():
        argv += ["--" + key.replace("_", "-"), str(value)]
    env = dict(os.environ)
    env["PATH"] = str(pwsh.parent) + os.pathsep + env.get("PATH", "")
    with (work / "consumer.stdout").open("xb") as stdout, (work / "consumer.stderr").open("xb") as stderr:
        completed = subprocess.run(argv, stdout=stdout, stderr=stderr, env=env, timeout=120)
    if completed.returncode or not consumer_file.is_file() or consumer_file.stat().st_size > BOUNDS["max_report_bytes"]:
        raise ValueError("real_consumer_failed")
    for name in ("consumer.stdout", "consumer.stderr"):
        if (work / name).stat().st_size > BOUNDS["max_capture_bytes"]:
            raise ValueError("consumer_capture_cap")
    raw_consumer = consumer_file.read_bytes()
    consumer = json.loads(raw_consumer)
    resolved = {key: consumer["decode_budget"][key] for key in OVERRIDES}
    workers = [run_worker(spec, work, pwsh, inventory, started) for spec in scenarios(resolved)]
    guard(work, started)
    if identity != source_identity(pwsh, harness):
        raise ValueError("source_or_executable_changed")
    record = {"schema_version": 1, "complete": True, "run_id": work.name, "bounds": BOUNDS,
              "source_identity": identity, "fixtures": fixtures, "decode_limits": resolved,
              "budget_source": "actual decode_compare DecodeBudget::resolve; narrow overrides, not copied defaults",
              "consumer": {"exit_code": completed.returncode, "record": redact_paths(consumer, [(work, "<work-root>"), (ROOT, "<repository>")]),
                           "raw_record_sha256": sha(raw_consumer),
                           "raster_retention": "consumer removes its own outputs; retained independent raw PNG bytes below come from separately instrumented workers, not consumer output copies"},
              "workers": workers, "duration_ns": time.monotonic_ns() - started,
              "work_bytes": _work_bytes(work), "cleanup": {"complete": True, "live_direct_children": []},
              "work_root_disposable": True,
              "unmeasured": ["WIC internal stride/scratch", "true concurrent allocation peak", "actual reservations",
                             "cooperative in-decoder cancellation", "region/sequential/tiled decode", "ICC/orientation complete behavior",
                             "16K decode", "JPEG absolute raster", "full Decode exit"]}
    data = canonical(record)
    if len(data) > BOUNDS["max_report_bytes"]:
        raise ValueError("report_cap")
    out.parent.mkdir(parents=True, exist_ok=True)
    with out.open("xb") as stream:
        stream.write(data)
    return record


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="op", required=True)
    execute = sub.add_parser("run")
    for flag in ("work-root", "out", "pwsh", "harness"):
        execute.add_argument("--" + flag, required=True, type=Path)
    check = sub.add_parser("verify")
    check.add_argument("--report", required=True, type=Path)
    check.add_argument("--source-root", type=Path)
    check.add_argument("--source-commit", help="verify against this historical Git commit")
    args = parser.parse_args(argv)
    try:
        if args.op == "run":
            record = run(args.work_root, args.out, args.pwsh, args.harness)
            result = verify(record, ROOT)
        else:
            if args.report.stat().st_size > BOUNDS["max_report_bytes"]:
                raise ValueError("report_cap")
            source_root = args.source_root
            temporary = None
            if args.source_commit:
                import tempfile
                temporary = Path(tempfile.mkdtemp())
                for name in SOURCE_FILES:
                    target = temporary / name
                    target.parent.mkdir(parents=True, exist_ok=True)
                    import shutil
                    git = os.environ.get("GIT_EXECUTABLE") or shutil.which("git") or \
                        "C:/Program Files/Git/cmd/git.exe"
                    target.write_bytes(subprocess.run(
                        [git, "show", f"{args.source_commit}:{name}"],
                        cwd=ROOT, check=True, stdout=subprocess.PIPE).stdout)
                source_root = temporary
            try:
                result = verify(json.loads(args.report.read_bytes()), source_root)
            finally:
                if temporary is not None:
                    import shutil
                    shutil.rmtree(temporary, ignore_errors=True)
        print(json.dumps(result, sort_keys=True))
        return 0 if result["ok"] else 1
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as exc:
        print(json.dumps({"ok": False, "error": type(exc).__name__, "reason": str(exc)[:160]}))
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
