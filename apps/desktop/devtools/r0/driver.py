"""MP-R0A-S2A: dev-only dedicated-process lifecycle driver (Python stdlib only).

Only an executable listed in the committed `approved-executables.json` whose
`product_commit` matches the manifest repository SHA may be launched. There is no
test-only executable path. Visual/frame endpoints stay NOT_MEASURED; launcher facts
live in a sidecar ledger. The driver never touches another process, never reuses an
output directory and never publishes an unvalidated result.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
import threading
import queue
import re
from pathlib import Path

import contracts
from windows_owner import Job, Pin, known_folder

DEFAULT_TIMEOUT_MS = 15_000
MAX_TIMEOUT_MS = 120_000
MAX_LOG_BYTES = 64 * 1024
DRAIN_TIMEOUT_S = 5.0
APPROVAL_FILE = Path(__file__).resolve().with_name("approved-executables.json")


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return "sha256:" + digest.hexdigest()


def validate_approval(document: object) -> list[dict]:
    fields = {
        "product_commit", "executable_sha256", "helper_sha256",
        "helper_source_sha256", "helper_protocol", "helper_relative_path",
    }
    if not isinstance(document, dict) or set(document) != {"schema_version", "approved"} or document["schema_version"] != 2:
        raise RuntimeError("approval record schema is invalid")
    entries = document["approved"]
    if not isinstance(entries, list) or not entries:
        raise RuntimeError("approval record is empty")
    for entry in entries:
        if not isinstance(entry, dict) or set(entry) != fields:
            raise RuntimeError("approval entry schema is invalid")
        if not isinstance(entry["product_commit"], str) or not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", entry["product_commit"]):
            raise RuntimeError("approval source identity is invalid")
        for key in ("executable_sha256", "helper_sha256", "helper_source_sha256"):
            if not isinstance(entry[key], str) or not re.fullmatch(r"sha256:[0-9a-f]{64}", entry[key]):
                raise RuntimeError("approval artifact identity is invalid")
        if type(entry["helper_protocol"]) is not int or entry["helper_protocol"] != 2 or entry["helper_relative_path"] != "target/debug/examples/r0_owner.exe":
            raise RuntimeError("approval helper identity is invalid")
    if len({entry["product_commit"] for entry in entries}) != len(entries):
        raise RuntimeError("approval source is ambiguous")
    return entries


def approved_digests() -> list[dict]:
    return validate_approval(contracts.load_document(str(APPROVAL_FILE), contracts.MAX_MANIFEST_BYTES))


def endpoint_matrix(launcher_available: bool, reason: str) -> list[dict]:
    visual = [
        "app-main-entry",
        "model-mutation",
        "invalidate-notify",
        "frame-request",
        "frame-start",
        "scene-prepared",
        "gpu-submission",
        "input-received",
        "present-call",
        "present-return",
        "first-frame",
        "useful-shell",
        "present",
        "displayed",
        "input-to-photon",
    ]
    matrix = [
        {
            "id": "process-launch",
            "status": "OBSERVED" if launcher_available else "NOT_MEASURED",
            "reason": "" if launcher_available else reason,
        }
    ]
    for name in visual:
        matrix.append(
            {
                "id": name,
                "status": "NOT_MEASURED",
                "reason": "no GPUI/Windows probe exists in S2A; deferred to S2B",
            }
        )
    return matrix


def result_document(manifest: dict, evidence: list[str], status: str, reason: str) -> dict:
    workload = manifest["workload"]
    endpoints = [
        {
            "id": item["id"],
            "status": "NOT_MEASURED",
            "reason": item["reason"] or "launcher-only observation; not a visual endpoint",
            "provider": {"status": "NOT_IMPLEMENTED", "reason": "no visual probe"},
            "version": {"status": "NOT_IMPLEMENTED", "reason": "no visual probe"},
            "unit": {"status": "NOT_IMPLEMENTED", "reason": "no visual probe"},
            "clock": {"status": "NOT_IMPLEMENTED", "reason": "no visual probe"},
            "correlation": {"status": "NOT_IMPLEMENTED", "reason": "no visual probe"},
        }
        for item in endpoint_matrix(True, "")
    ]
    return {
        "result_version": 1,
        "manifest": {
            "scenario_id": manifest["scenario"]["id"],
            "scenario_version": manifest["scenario"]["version"],
            "benchmark_revision": manifest["scenario"]["benchmark_revision"],
            "definition_hash": manifest["identity"]["definition_hash"],
        },
        "runs": [
            {
                "run_id": workload["run_id"],
                "pair_id": workload["pair_id"],
                "scenario_id": manifest["scenario"]["id"],
                "variant": workload["variant"],
                "status": status,
                "reason": reason,
                "endpoints": endpoints,
                "metrics": [
                    {
                        "id": "startup-launch",
                        "unit": "ms",
                        "method": "launcher-monotonic",
                        "sample_unit": "launch",
                        "counts": {"eligible": 0, "observed": 0, "failed": 0, "censored": 0, "excluded": 0},
                        "percentiles": {"min": None, "max": None, "p50": None, "p95": None, "p99": None},
                        "estimator": "none",
                        "validity": "launcher facts live in the sidecar ledger; no estimator in S2A",
                        "spread": "not-estimated",
                    }
                ],
                "resources": {
                    "process": {"private_bytes": None, "working_set_bytes": None, "status": "NOT_MEASURED", "reason": "no process-memory probe"},
                    "media": {"decoded_bytes": None, "scratch_bytes": None, "staging_bytes": None, "cache_bytes": None, "status": "NOT_MEASURED", "reason": "no media path"},
                    "gpu": {"dedicated_bytes": None, "shared_bytes": None, "committed_bytes": None, "status": "NOT_MEASURED", "reason": "no GPU probe"},
                    "devices": {"count": None, "generations": None, "status": "NOT_MEASURED", "reason": "no device probe"},
                    "sampling": "sidecar ledger only",
                },
                "frames": {
                    "requested": None,
                    "processed": None,
                    "submitted": None,
                    "present": None,
                    "displayed": None,
                    "missed": None,
                    "dropped": None,
                    "stalled": None,
                    "eligibility": None,
                    "idle": None,
                    "rule": "no frame counters are observed in S2A",
                    "reason": "frame probes are deferred to S2B",
                },
                "instrumentation": {
                    "tool": "r0a-s2a",
                    "revision": "2",
                    "config": "approved dedicated fixture process",
                    "probes": {"supported": ["launcher"], "missing": ["gpui-frame", "present", "displayed", "gpu"]},
                    "overhead": "not measured",
                    "loss": "not measured",
                    "floor": "not measured",
                },
                "failures": [reason] if status == "FAILED" else [],
                "outliers": [],
                "evidence": evidence,
            }
        ],
        "evidence": evidence,
    }


def parse_message(line: bytes) -> dict:
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError("duplicate field")
            result[key] = value
        return result

    if len(line) > MAX_LOG_BYTES or not line.endswith(b"\n"):
        raise RuntimeError("helper message is incomplete or oversized")
    try:
        value = json.loads(line.decode("utf-8"), object_pairs_hook=unique)
    except (ValueError, UnicodeError, RecursionError):
        raise RuntimeError("helper message is invalid") from None
    if not isinstance(value, dict):
        raise RuntimeError("helper message is not an object")
    return value


def validate_ledger(value: dict, binding: dict, execution: str, fixture: str, timeout_ms: int) -> dict:
    fields = {
        "schema_version", "execution_id", "pid", "exit_code", "timed_out",
        "approved", "executable_sha256", "executable_bytes", "fixture_sha256",
        "fixture_bytes", "elapsed_ms", "child_reaped", "termination_status",
        "cleanup", "publication",
    }
    if set(value) != fields:
        raise RuntimeError("helper ledger fields are invalid")
    if type(value["schema_version"]) is not int or value["schema_version"] != 2 or value["execution_id"] != execution:
        raise RuntimeError("helper ledger invocation mismatch")
    for key in ("pid", "executable_bytes", "fixture_bytes"):
        if type(value[key]) is not int or not 0 < value[key] < 1 << 63:
            raise RuntimeError("helper ledger integer is invalid")
    if type(value["exit_code"]) is not int or not -(1 << 31) <= value["exit_code"] < 1 << 32:
        raise RuntimeError("helper exit status is invalid")
    if type(value["elapsed_ms"]) is not int or not 0 <= value["elapsed_ms"] <= timeout_ms + 5000:
        raise RuntimeError("helper elapsed time is invalid")
    if type(value["timed_out"]) is not bool or value["approved"] is not True or value["child_reaped"] is not True:
        raise RuntimeError("helper completion is unconfirmed")
    if value["executable_sha256"] != binding["executable_sha256"] or value["fixture_sha256"] != fixture:
        raise RuntimeError("helper content binding mismatch")
    if value["cleanup"] != "complete" or value["publication"] != "driver-owned":
        raise RuntimeError("helper cleanup or publication is unconfirmed")
    expected = "requested" if value["timed_out"] else "not-requested"
    if value["termination_status"] != expected or (value["timed_out"] and value["elapsed_ms"] < timeout_ms):
        raise RuntimeError("helper timeout disposition is contradictory")
    return value


def read_message(pipe, timeout_s: float) -> bytes:
    replies = queue.Queue(maxsize=1)

    def receive():
        try:
            replies.put(pipe.readline(MAX_LOG_BYTES + 1))
        except (OSError, ValueError):
            replies.put(b"")

    threading.Thread(target=receive, daemon=True).start()
    try:
        return replies.get(timeout=timeout_s)
    except queue.Empty:
        raise RuntimeError("helper response deadline exceeded; cleanup unconfirmed") from None


def exclusive_write(path: Path, document: object) -> None:
    """Write, flush, sync and close before the separate publication point."""
    with path.open("x", encoding="utf-8", newline="\n") as handle:
        json.dump(document, handle, indent=2)
        handle.write("\n")
        handle.flush()
        os.fsync(handle.fileno())


SOURCE_FILES = (
    "apps/desktop/examples/r0_owner.rs",
    "apps/desktop/examples/r0_owner/sha256.rs",
    "apps/desktop/examples/r0_owner/ownership.rs",
)


def helper_source_digest() -> str:
    digest = hashlib.sha256()
    for name in sorted(SOURCE_FILES):
        digest.update(name.encode() + b"\0")
        digest.update((Path(__file__).resolve().parents[4] / name).read_bytes())
    return "sha256:" + digest.hexdigest()


def require_empty_directory(path: Path) -> None:
    if not path.is_dir() or any(path.iterdir()):
        raise RuntimeError("an existing empty output directory is required")


def require_per_run_profile(profile: Path, execution: str) -> None:
    home = known_folder("profile")
    # Resolve Windows short-name aliases before comparing protected ancestry;
    # acquisition still rejects every reparse component independently.
    candidate = os.path.normcase(os.path.realpath(profile))
    protected = os.path.normcase(os.path.realpath(home))
    if candidate == protected or candidate.startswith(protected + os.sep) or protected.startswith(candidate + os.sep):
        raise RuntimeError("the Windows user profile is not a dedicated run profile")
    if profile.name != execution:
        raise RuntimeError("profile directory name must equal the run identity")
    children = {entry.name for entry in profile.iterdir()}
    if children != {"roaming", "temp"} or any(any((profile / name).iterdir()) for name in children):
        raise RuntimeError("profile must contain only empty roaming/temp directories")


def consume_identity(registry: Path, execution: str, identity: tuple[int, int]) -> None:
    """Exclusive, persistent claims; failed attempts never release a claim."""
    if not re.fullmatch(r"[0-9a-f]{32}", execution):
        raise RuntimeError("run identity is invalid")
    with Pin(registry, directory=True):
        (registry / ("run-" + execution)).mkdir()
        (registry / f"profile-{identity[0]:x}-{identity[1]:x}").mkdir()


def claim_run(profile_pin: Pin, execution: str) -> None:
    local = known_folder("local_appdata")
    with Pin(local, directory=True):
        registry = local / "cos-desktop-r0"
        registry.mkdir(exist_ok=True)
        # Registry state is independent of caller-supplied, clearable profiles.
        consume_identity(registry, execution, profile_pin.identity())


def read_artifact(output: Path, name: str) -> bytes:
    with Pin(output / name) as pin:
        if pin.file is None:
            raise RuntimeError("artifact is not a regular file")
        if os.fstat(pin.file.fileno()).st_size > MAX_LOG_BYTES:
            raise RuntimeError("artifact exceeds the bounded size")
        return pin.file.read()


def publication_contents(output: Path, manifest: dict, timeout_ms: int) -> tuple[dict, str]:
    """Validate the complete S2A contract, including unmeasured-state invariants."""
    completion_bytes = read_artifact(output, "completion.json")
    completion = parse_message(completion_bytes)
    if set(completion) != {"schema_version", "execution_id", "artifacts"} or type(completion["schema_version"]) is not int or completion["schema_version"] != 1:
        raise RuntimeError("completion record is invalid")
    execution = completion["execution_id"]
    if not isinstance(execution, str) or not re.fullmatch(r"[0-9a-f]{32}", execution):
        raise RuntimeError("completion run identity is invalid")
    expected = {"ledger.json", "endpoint-availability.json", "result.json"}
    if not isinstance(completion["artifacts"], dict) or set(completion["artifacts"]) != expected:
        raise RuntimeError("completion record artifact set is invalid")
    raw = {}
    for name in expected:
        data = read_artifact(output, name)
        if completion["artifacts"][name] != "sha256:" + hashlib.sha256(data).hexdigest():
            raise RuntimeError("completion record digest mismatch")
        raw[name] = data
    ledger = validate_ledger(
        parse_message(raw["ledger.json"]),
        {"executable_sha256": manifest["identity"]["executable_sha256"]},
        execution, manifest["workload"]["source"]["sha256"], timeout_ms,
    )
    # Reuse duplicate-field rejection without accepting a list as a helper reply.
    matrix = parse_message(b'{"matrix":' + raw["endpoint-availability.json"].rstrip() + b'}\n')["matrix"]
    expected_ids = {item["id"] for item in endpoint_matrix(True, "")}
    if not isinstance(matrix, list) or len(matrix) != len(expected_ids):
        raise RuntimeError("endpoint matrix is incomplete")
    seen = set()
    for item in matrix:
        if not isinstance(item, dict) or set(item) != {"id", "status", "reason"} or not isinstance(item["id"], str):
            raise RuntimeError("endpoint record is invalid")
        name = item["id"]
        if name not in expected_ids or name in seen:
            raise RuntimeError("endpoint identity is unknown or duplicated")
        seen.add(name)
        if item["status"] != ("OBSERVED" if name == "process-launch" else "NOT_MEASURED") or not isinstance(item["reason"], str):
            raise RuntimeError("endpoint observation is unsupported")
        if name != "process-launch" and not item["reason"].strip():
            raise RuntimeError("unmeasured endpoint requires a reason")
    document = contracts.validate_result(parse_message(raw["result.json"]))
    contracts.check_identity(manifest, document)
    expected_status = "FAILED" if ledger["timed_out"] or ledger["exit_code"] != 0 else "NOT_MEASURED"
    if len(document["runs"]) != 1 or document["runs"][0]["status"] != expected_status:
        raise RuntimeError("ledger and result disposition disagree")
    run = document["runs"][0]
    if len(run["endpoints"]) != len(expected_ids) or {item["id"] for item in run["endpoints"]} != expected_ids or any(item["status"] != "NOT_MEASURED" for item in run["endpoints"]):
        raise RuntimeError("S2A result cannot contain measured endpoints")
    if any(value != 0 for metric in run["metrics"] for value in metric["counts"].values()):
        raise RuntimeError("S2A has no measured metric samples")
    if any(run["frames"][name] is not None for name in ("requested", "processed", "submitted", "present", "displayed", "missed", "dropped", "stalled", "eligibility", "idle")):
        raise RuntimeError("S2A has no measured frame counters")
    if any(run["resources"][name]["status"] != "NOT_MEASURED" for name in ("process", "media", "gpu", "devices")):
        raise RuntimeError("S2A has no measured resource counters")
    return completion, "committed-" + hashlib.sha256(completion_bytes).hexdigest()


def verify_publication(output: Path, manifest: dict, timeout_ms: int) -> dict:
    """Require the atomic directory commit, not the obsolete commit.json file."""
    with Pin(output, directory=True) as root:
        completion, marker = publication_contents(output, manifest, timeout_ms)
        with Pin(output / marker, directory=True) as committed:
            if any((output / marker).iterdir()):
                raise RuntimeError("publication commit is not empty")
            committed.verify()
        root.verify()
    return completion


def publish(output: Path, manifest: dict, timeout_ms: int, ledger: dict, document: dict) -> None:
    """CreateDirectory is the sole commit point, after every file sync/close.

    A prior file/create/write/flush/fsync/close failure leaves no commit directory.
    Interruption after mkdir is committed: all prerequisites already succeeded.
    This guarantees publication visibility, not power-loss durability.
    """
    with Pin(output, directory=True) as root:
        completion = {"schema_version": 1, "execution_id": ledger["execution_id"], "artifacts": {}}
        for name, content in (
            ("ledger.json", ledger),
            ("endpoint-availability.json", endpoint_matrix(True, "")),
            ("result.json", document),
        ):
            exclusive_write(output / name, content)
            completion["artifacts"][name] = "sha256:" + hashlib.sha256(read_artifact(output, name)).hexdigest()
        exclusive_write(output / "completion.json", completion)
        _, marker = publication_contents(output, manifest, timeout_ms)
        root.verify()
        (output / marker).mkdir()


def launch(args: argparse.Namespace) -> int:
    manifest = contracts.validate_manifest(
        contracts.load_document(args.manifest, contracts.MAX_MANIFEST_BYTES)
    )
    exe, fixture, helper = map(Path, (args.executable, args.fixture, args.helper))
    output, profile = Path(args.output_dir), Path(args.profile)
    if not (1 <= args.timeout_ms <= MAX_TIMEOUT_MS):
        raise RuntimeError("timeout outside the bounded range")
    require_empty_directory(output)
    execution = profile.name
    if not re.fullmatch(r"[0-9a-f]{32}", execution):
        raise RuntimeError("profile name must be a 32-character execution identity")
    require_per_run_profile(profile, execution)
    if exe.name != "cos-desktop.exe":
        raise RuntimeError("executable name is not the approved product binary")
    source = manifest["workload"]["source"]["sha256"]
    if not isinstance(source, str):
        raise RuntimeError("a real fixture digest is required in the workload manifest")
    entries = [item for item in approved_digests() if item["product_commit"] == manifest["identity"]["repository"]["sha"]]
    if len(entries) != 1:
        raise RuntimeError("product source is not approved")
    binding = entries[0]
    expected_helper = Path(__file__).resolve().parents[4] / binding["helper_relative_path"]
    if os.path.normcase(os.path.abspath(helper)) != os.path.normcase(str(expected_helper)):
        raise RuntimeError("helper location is not approved")
    if manifest["identity"]["executable_sha256"] != binding["executable_sha256"]:
        raise RuntimeError("manifest executable identity does not match approval")
    if helper_source_digest() != binding["helper_source_sha256"]:
        raise RuntimeError("helper source is not the approved source")
    process, job = None, None
    try:
        with Pin(exe) as exe_pin, Pin(helper) as helper_pin, Pin(fixture) as fixture_pin, \
                Pin(output, directory=True) as output_pin, Pin(profile, directory=True) as profile_pin, \
                Pin(profile / "roaming", directory=True) as roaming_pin, \
                Pin(profile / "temp", directory=True) as temp_pin:
            if any(output.iterdir()) or any(any((profile / n).iterdir()) for n in ("roaming", "temp")):
                raise RuntimeError("output/profile changed before acquisition completed")
            exe_digest, exe_bytes = exe_pin.digest()
            helper_digest, _ = helper_pin.digest()
            fixture_digest, fixture_bytes = fixture_pin.digest()
            if exe_digest != binding["executable_sha256"] or helper_digest != binding["helper_sha256"]:
                raise RuntimeError("approved artifact digest mismatch")
            if fixture_digest != source:
                raise RuntimeError("fixture digest does not match the workload")
            claim_run(profile_pin, execution)
            (output / "run-id").write_text(execution, encoding="ascii")
            (profile / "run-id").write_text(execution, encoding="ascii")
            job = Job()
            process = subprocess.Popen(
                [
                    str(helper), "run", "--root", str(output), "--profile", str(profile),
                    "--exe", str(exe), "--approved-sha256", binding["executable_sha256"],
                    "--fixture", str(fixture), "--fixture-sha256", source,
                    "--execution-id", execution, "--timeout-ms", str(args.timeout_ms),
                ],
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                env={"SystemRoot": os.environ["SystemRoot"]},
            )
            job.assign(process)
            stdin, stdout = process.stdin, process.stdout
            if stdin is None or stdout is None:
                raise RuntimeError("helper pipes are unavailable")
            stdin.write(b"go-v2\n")
            stdin.flush()
            ledger = validate_ledger(
                parse_message(read_message(stdout, args.timeout_ms / 1000 + 10)),
                binding, execution, source, args.timeout_ms,
            )
            if (ledger["executable_bytes"], ledger["fixture_bytes"]) != (exe_bytes, fixture_bytes):
                raise RuntimeError("helper sizes do not match the pinned artifacts")
            status = "FAILED" if ledger["timed_out"] or ledger["exit_code"] != 0 else "NOT_MEASURED"
            reason = (
                "launcher timeout; owned child terminated and reaped"
                if ledger["timed_out"] else
                "child exited nonzero" if ledger["exit_code"] != 0 else
                "process exited 0; visual/frame endpoints were not measured"
            )
            document = result_document(manifest, ["ledger.json", "endpoint-availability.json"], status, reason)
            contracts.check_identity(manifest, contracts.validate_result(document))
            output_pin.verify()
            stdin.write(b"release-v2\n")
            stdin.close()
            process.wait(timeout=DRAIN_TIMEOUT_S)
            if process.returncode != 0 or stdout.read(1) or not job.empty():
                raise RuntimeError("helper or owned descendant completion is unconfirmed")
            output_pin.verify()
            exe_pin.verify()
            helper_pin.verify()
            fixture_pin.verify()
            profile_pin.verify()
            roaming_pin.verify()
            temp_pin.verify()
            publish(output, manifest, args.timeout_ms, ledger, document)
            verify_publication(output, manifest, args.timeout_ms)
    finally:
        if process is not None:
            if process.poll() is None:
                process.kill()
                try:
                    process.wait(timeout=DRAIN_TIMEOUT_S)
                except subprocess.TimeoutExpired:
                    pass
            for pipe in (process.stdin, process.stdout):
                if pipe is not None:
                    pipe.close()
        if job is not None:
            # Closing the only handle terminates any remaining owned descendant.
            job.close()
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="MP-R0A-S2A approved launcher")
    parser.add_argument("--manifest", required=True)
    parser.add_argument("--executable", required=True)
    parser.add_argument("--fixture", required=True)
    parser.add_argument("--output-dir", required=True)
    parser.add_argument("--helper", required=True)
    parser.add_argument("--profile", required=True)
    parser.add_argument("--timeout-ms", type=int, default=DEFAULT_TIMEOUT_MS)
    return launch(parser.parse_args(argv))


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (RuntimeError, OSError, subprocess.SubprocessError, contracts.ContractError):
        # Native exceptions can contain private paths; only the category is public.
        print("S2A failed closed; helper/child cleanup is unconfirmed unless a validated ledger was published", file=sys.stderr)
        sys.exit(1)
