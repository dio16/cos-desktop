#!/usr/bin/env python3
"""Live RT3/RT8 campaign: discovery, restart reauthorization and epoch change.

What it does, once per run label:

1. Copies the Runtime production database through a read-only SQLite handle and
   starts the Runtime on 127.0.0.1:8124 with an isolated config, a per-run
   ``ORCHESTRATOR_AUTH_SECRET`` and a discovery descriptor redirected into the
   run work directory (``COS_RUNTIME_DISCOVERY_PATH``).
2. Waits for the atomically published descriptor and validates it independently
   (schema, loopback origin, server identity, epoch, protocol/schema ranges,
   pid, readiness).
3. Launches the Desktop release binary with **no** ``COS_DESKTOP_RUNTIME_ENDPOINT``
   and **no** ``COS_DESKTOP_RUNTIME_TOKEN``; only the operator issuance secret
   and the redirected discovery path are in the child environment. It must
   discover the Runtime and render the real projected task (UIA + screenshot).
4. Restarts the Runtime (new process, new observation epoch, same DB copy).
   The *same* Desktop process must recover its projection without a relaunch,
   the descriptor must reappear with a new epoch, and the app stderr must show
   the bounded reauthorization line.
5. Closes the Desktop cleanly, proves the Runtime (and its descriptor) keeps
   serving after Desktop exit, then stops the Runtime and proves the descriptor
   is removed.

Tokens and the issuance secret are passed to children through the environment
only and never written to evidence. The shared native lock
``C:/tmp/cos-desktop-native.lock`` is honoured for the whole run.

Usage (from the repository root):

    python apps/desktop/devtools/runtime_integration/run_rt3_rt8.py --runs 1
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import json
import os
import secrets
import shutil
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import run_integration as base  # noqa: E402 - local harness helpers, stdlib only

DESKTOP_ROOT = base.DESKTOP_ROOT
RUNTIME_ROOT = base.RUNTIME_ROOT
RUNTIME_PYTHON = base.RUNTIME_PYTHON
SOURCE_DB = base.SOURCE_DB
DESKTOP_EXE = base.DESKTOP_EXE
PROBE = base.PROBE
PORT = base.PORT
ENDPOINT = base.ENDPOINT
EVIDENCE_DIR = (
    DESKTOP_ROOT / "docs" / "development" / "evidence-published" / "rt3-rt8-latest"
)
WORK_ROOT = DESKTOP_ROOT / "target" / "rt3-rt8"

SCENARIO_ORDER = (
    "discovery_connect",
    "restart_recovery_without_relaunch",
    "descriptor_lifecycle",
    "task_survival_after_desktop_exit",
    "clean_close",
)
STATUS_LABELS = base.STATUS_LABELS


class DiscoveredDesktopApp:
    """Desktop launched without an endpoint override, relying on discovery."""

    def __init__(self, *, label: str, work: Path, secret: str, localappdata: Path):
        self.label = label
        self.work = work
        self.secret = secret
        self.localappdata = localappdata
        self.process: subprocess.Popen | None = None
        self.log_path = work / f"desktop-{label}.log"
        self.log_handle = None
        self.environment: dict[str, str] = {}

    def launch(self) -> subprocess.Popen:
        environment = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith("COS_DESKTOP_")
            and not key.startswith("COS_RUNTIME_")
            and key not in {"COS_FIXTURE_TRACE", "COS_PREVIEW_TRACE", "COS_EXPORT_TRACE"}
        }
        environment["LOCALAPPDATA"] = str(self.localappdata)
        # Deliberately no COS_DESKTOP_RUNTIME_ENDPOINT / TOKEN: discovery and the
        # issuance secret are the only connection inputs.
        environment["COS_DESKTOP_RUNTIME_ISSUANCE_SECRET"] = self.secret
        environment["COS_DESKTOP_RUNTIME_DISCOVERY_PATH"] = str(
            self.work / "runtime-discovery.json"
        )
        self.environment = environment
        self.localappdata.mkdir(parents=True, exist_ok=True)
        self.log_handle = self.log_path.open("wb")
        self.process = subprocess.Popen(  # noqa: S603 - fixed binary, no fixture flag
            [str(DESKTOP_EXE)],
            cwd=str(DESKTOP_ROOT),
            env=environment,
            stdout=self.log_handle,
            stderr=subprocess.STDOUT,
        )
        return self.process

    def alive(self) -> bool:
        return self.process is not None and self.process.poll() is None

    def close(self) -> dict:
        if self.process is None:
            return {"closed": False, "reason": "not_started"}
        pid = self.process.pid
        sent = False
        if base.pid_alive(pid):
            with contextlib.suppress(RuntimeError):
                result = base.probe("close", pid, self.work / f"close-{self.label}.json")
                sent = bool(result.get("sent"))
        try:
            code = self.process.wait(timeout=25)
            exited = True
        except subprocess.TimeoutExpired:
            code = None
            exited = False
        forced = None
        if not exited:
            forced = base.kill_tree(pid)
            exited = not base.pid_alive(pid)
        if self.log_handle is not None:
            self.log_handle.close()
            self.log_handle = None
        return {
            "pid": pid,
            "wm_close_sent": sent,
            "exited": exited,
            "exit_code": code,
            "forced": forced,
        }

    def force_stop(self) -> None:
        if self.process is not None and base.pid_alive(self.process.pid):
            base.kill_tree(self.process.pid)
        if self.log_handle is not None:
            self.log_handle.close()
            self.log_handle = None


def descriptor_snapshot(path: Path) -> dict:
    if not path.is_file():
        return {"present": False}
    raw = path.read_bytes()
    try:
        document = json.loads(raw.decode("utf-8"))
    except ValueError:
        document = None
    return {
        "present": True,
        "sha256": base.sha256_file(path),
        "bytes": len(raw),
        "document": document,
    }


def wait_descriptor(path: Path, predicate, timeout: float, label: str) -> dict:
    deadline = time.monotonic() + timeout
    last: dict = {"present": False}
    while time.monotonic() < deadline:
        last = descriptor_snapshot(path)
        if last["present"] and predicate(last["document"] or {}):
            return last
        time.sleep(0.2)
    raise RuntimeError(f"descriptor {label} did not match within {timeout}s: {last}")


def wait_descriptor_gone(path: Path, timeout: float) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if not path.exists():
            return True
        time.sleep(0.2)
    return False


def descriptor_valid(document: dict, *, epoch: str) -> bool:
    protocols = {"major": 2, "min_minor": 0, "max_minor": 0}
    return (
        document.get("schema_version") == 1
        and document.get("origin") == ENDPOINT
        and document.get("server_identity") == "cos-runtime-orchestrator"
        and document.get("epoch") == epoch
        and document.get("protocol") == protocols
        and document.get("schema") == protocols
        and isinstance(document.get("pid"), int)
        and document.get("pid") > 0
        and document.get("ready") is True
        and isinstance(document.get("started_at"), str)
    )


def poll_title(pid: int, evidence: Path, title: str, timeout: float) -> tuple[dict, str, bool]:
    deadline = time.monotonic() + timeout
    last_dump: dict = {}
    last_text = ""
    while time.monotonic() < deadline:
        if not base.pid_alive(pid):
            return last_dump, last_text, False
        try:
            dump, text = base.wait_uia(
                pid, evidence / "uia-recovering.json", lambda joined: title in joined, 5.0
            )
        except RuntimeError:
            time.sleep(0.5)
            continue
        last_dump, last_text = dump, text
        if title in text:
            return dump, text, True
        time.sleep(0.5)
    return last_dump, last_text, False


def poll_withheld(pid: int, evidence: Path, title: str, timeout: float) -> tuple[dict, str, bool]:
    """Wait until the old projection is actually withheld after the stop."""
    deadline = time.monotonic() + timeout
    last_dump: dict = {}
    last_text = ""
    while time.monotonic() < deadline:
        if not base.pid_alive(pid):
            return last_dump, last_text, False
        try:
            dump, text = base.wait_uia(
                pid, evidence / "uia-withheld.json", lambda _joined: True, 5.0
            )
        except RuntimeError:
            time.sleep(0.5)
            continue
        last_dump, last_text = dump, text
        if title not in text and any(marker in text for marker in base.WITHDRAWN_MARKERS):
            return dump, text, True
        time.sleep(0.5)
    return last_dump, last_text, False


def run_once(label: str, collected: list[str]) -> dict:
    started = base.now_iso()
    run_work = WORK_ROOT / label
    if run_work.exists():
        shutil.rmtree(run_work)
    run_work.mkdir(parents=True, exist_ok=True)
    evidence = EVIDENCE_DIR / label
    if evidence.exists():
        shutil.rmtree(evidence)
    evidence.mkdir(parents=True, exist_ok=True)
    descriptor_path = run_work / "runtime-discovery.json"

    facts: dict = {
        "label": label,
        "started_at": started,
        "scenarios": {},
        "files": {},
        "findings": [],
        "endpoint_env_recorded": False,
        "token_recorded": False,
        "run_secret_recorded": False,
    }
    run_secret = secrets.token_hex(24)
    collected.append(run_secret)
    secrets_list = [run_secret]

    db_copy = run_work / "generation-copy.sqlite"
    base.copy_database(SOURCE_DB, db_copy)
    facts["db_copy_sha256"] = base.sha256_file(db_copy)
    facts["db_copy_bytes"] = db_copy.stat().st_size

    localappdata = run_work / "localappdata"
    (localappdata / "cos-desktop").mkdir(parents=True, exist_ok=True)

    server = base.RuntimeServer(
        label=label,
        work=run_work,
        db_copy=db_copy,
        secret=run_secret,
        discovery_path=descriptor_path,
        stop_file=run_work / "runtime.stop",
    )
    app: DiscoveredDesktopApp | None = None
    try:
        base.free_port_fixed()
        base.cleanup_credential_store(ENDPOINT)
        if descriptor_path.exists():
            descriptor_path.unlink()
        with base.native_lock("impl-flow2/rt3-rt8"):
            first = server.start()
            facts["server_first"] = first
            descriptor_first = wait_descriptor(
                descriptor_path,
                lambda document: descriptor_valid(
                    document, epoch=first["epoch"]
                ),
                15.0,
                "first",
            )
            facts["descriptor_first"] = descriptor_first

            harness_credential = server.issue_desktop_credential()
            secrets_list.append(harness_credential["token"])
            collected.append(harness_credential["token"])
            observation = server.observation(harness_credential["token"])
            tasks = base.expected_tasks(observation)
            expected = base.choose_expected(tasks)
            status_label = STATUS_LABELS.get(str(expected["task_status"]), "")
            facts["projection"] = {
                "task_count": len(tasks),
                "expected_task_ref": expected["task_ref"],
                "expected_title": expected["title"],
                "expected_task_status": expected["task_status"],
                "expected_status_label": status_label,
                "expected_inner_phase": expected["inner_phase"],
                "expected_iteration_index": expected["iteration_index"],
            }

            # Scenario A: discovery-only connect and the real projection.
            app = DiscoveredDesktopApp(
                label="discovered",
                work=run_work,
                secret=run_secret,
                localappdata=localappdata,
            )
            app.launch()
            app_pid = app.process.pid
            facts["desktop"] = {
                "pid": app_pid,
                "endpoint_env_absent": "COS_DESKTOP_RUNTIME_ENDPOINT" not in app.environment,
                "token_env_absent": "COS_DESKTOP_RUNTIME_TOKEN" not in app.environment,
                "issuance_secret_present": "COS_DESKTOP_RUNTIME_ISSUANCE_SECRET"
                in app.environment,
                "discovery_override_present": "COS_DESKTOP_RUNTIME_DISCOVERY_PATH"
                in app.environment,
            }
            window = base.wait_window(app_pid, evidence / "uia-window.json")
            dump, text = base.wait_uia(
                app_pid,
                evidence / "uia-connected.json",
                lambda joined: expected["title"] in joined,
                60.0,
            )
            connected = bool(dump.get("found") and expected["title"] in text)
            shot = base.probe(
                "shot", app_pid, evidence / "shot-connected.json", evidence / "connected.png"
            )
            facts["scenarios"]["discovery_connect"] = {
                "verdict": "PASS" if connected else "FAIL",
                "window": {"found": window.get("found"), "title": window.get("title")},
                "title_visible": connected,
                "status_label_visible": bool(status_label and status_label in text),
                "descriptor_origin": descriptor_first["document"]["origin"],
                "descriptor_epoch": descriptor_first["document"]["epoch"],
                "matched_excerpt": base.bounded(
                    "\n".join(
                        line
                        for line in text.splitlines()
                        if expected["title"] in line or status_label in line
                    ),
                    1200,
                ),
                "screenshot": {
                    "file": "connected.png",
                    "printed": shot.get("ok"),
                    "width": shot.get("width"),
                    "height": shot.get("height"),
                },
                "uia_element_count": dump.get("elements"),
            }
            app_connect_stderr = base.sanitized_log(app.log_path, secrets_list)
            (evidence / "app-connect.stderr.sanitized.txt").write_text(
                app_connect_stderr, encoding="utf-8"
            )
            facts["scenarios"]["discovery_connect"]["discovered_line"] = (
                "Runtime discovered:" in app_connect_stderr
            )
            if not connected:
                raise RuntimeError("discovery connect did not render the expected task title")

            # Scenario B: Runtime restart, same Desktop process.
            first_epoch = first["epoch"]
            stop_first = server.stop(clean=True)
            facts["server_first"]["stop"] = stop_first
            descriptor_removed = wait_descriptor_gone(descriptor_path, 20.0)
            facts["descriptor_removed_on_shutdown"] = descriptor_removed
            # The old projection must actually be withheld (not merely still
            # visible from before the break) before recovery is measured.
            withheld_dump, withheld_text, withheld_observed = poll_withheld(
                app_pid, evidence, expected["title"], 45.0
            )
            withheld_shot = base.probe(
                "shot", app_pid, evidence / "shot-withheld.json", evidence / "withheld.png"
            )
            facts["withheld_after_stop"] = {
                "observed": withheld_observed,
                "uia_element_count": withheld_dump.get("elements"),
                "matched_excerpt": base.bounded(
                    "\n".join(
                        line
                        for line in withheld_text.splitlines()
                        if any(marker in line for marker in base.WITHDRAWN_MARKERS)
                    ),
                    800,
                ),
                "screenshot": {
                    "file": "withheld.png",
                    "printed": withheld_shot.get("ok"),
                },
            }
            second = server.start()
            facts["server_second"] = second
            descriptor_second = wait_descriptor(
                descriptor_path,
                lambda document: descriptor_valid(
                    document, epoch=second["epoch"]
                ),
                15.0,
                "second",
            )
            facts["descriptor_second"] = descriptor_second
            facts["epoch_changed"] = first_epoch != second["epoch"]
            facts["same_desktop_process"] = app.alive() and app.process.pid == app_pid
            recovered_dump, recovered_text, recovered = poll_title(
                app_pid, evidence, expected["title"], 120.0
            )
            recovered_shot = base.probe(
                "shot", app_pid, evidence / "shot-recovered.json", evidence / "recovered.png"
            )
            app_restart_stderr = base.sanitized_log(app.log_path, secrets_list)
            (evidence / "app-recovered.stderr.sanitized.txt").write_text(
                app_restart_stderr, encoding="utf-8"
            )
            facts["scenarios"]["restart_recovery_without_relaunch"] = {
                "verdict": "PASS"
                if (withheld_observed and recovered and facts["same_desktop_process"])
                else "FAIL",
                "withheld_after_stop": withheld_observed,
                "same_desktop_process": facts["same_desktop_process"],
                "desktop_pid": app_pid,
                "process_alive": app.alive(),
                "epoch_changed": facts["epoch_changed"],
                "first_epoch": first_epoch,
                "second_epoch": second["epoch"],
                "title_visible_after_restart": recovered,
                "reauthorized_line": "Runtime credential reauthorized after rejection"
                in app_restart_stderr,
                "matched_excerpt": base.bounded(
                    "\n".join(
                        line
                        for line in recovered_text.splitlines()
                        if expected["title"] in line or status_label in line
                    ),
                    1200,
                ),
                "screenshot": {
                    "file": "recovered.png",
                    "printed": recovered_shot.get("ok"),
                    "width": recovered_shot.get("width"),
                    "height": recovered_shot.get("height"),
                },
            }
            if not recovered:
                raise RuntimeError(
                    "the same Desktop process did not recover the projection after the Runtime restart"
                )

            # Scenario C: Desktop exit must not stop Runtime task execution.
            close = app.close()
            app_stderr = base.sanitized_log(app.log_path, secrets_list)
            (evidence / "app-close.stderr.sanitized.txt").write_text(
                app_stderr, encoding="utf-8"
            )
            close_markers = [marker for marker in base.CLOSE_MARKERS if marker in app_stderr]
            still_ready = True
            try:
                base.wait_http(f"{ENDPOINT}/api/ready", 10.0)
            except RuntimeError:
                still_ready = False
            facts["scenarios"]["task_survival_after_desktop_exit"] = {
                "verdict": "PASS" if (still_ready and descriptor_path.exists()) else "FAIL",
                "runtime_ready_after_desktop_exit": still_ready,
                "descriptor_still_published": descriptor_path.exists(),
                "desktop_forced": close.get("forced"),
            }
            facts["scenarios"]["clean_close"] = {
                "verdict": "PASS"
                if close.get("exited") and not close.get("forced") and len(close_markers) == 2
                else "FAIL",
                "exit_code": close.get("exit_code"),
                "wm_close_sent": close.get("wm_close_sent"),
                "close_markers": close_markers,
                "process_gone": not base.pid_alive(app_pid),
                "forced": close.get("forced"),
            }

            # Scenario D: Runtime shutdown removes its own descriptor.
            stop_second = server.stop(clean=True)
            facts["server_second"]["stop"] = stop_second
            final_removed = wait_descriptor_gone(descriptor_path, 20.0)
            facts["scenarios"]["descriptor_lifecycle"] = {
                "verdict": "PASS"
                if descriptor_removed and final_removed and facts["epoch_changed"]
                else "FAIL",
                "first_published": descriptor_first["present"],
                "first_sha256": descriptor_first.get("sha256"),
                "removed_on_first_shutdown": descriptor_removed,
                "second_published": descriptor_second["present"],
                "second_sha256": descriptor_second.get("sha256"),
                "removed_on_second_shutdown": final_removed,
                "epoch_changed": facts["epoch_changed"],
            }
            runtime_log = base.sanitized_log(server.log_path, secrets_list)
            (evidence / "runtime.log.sanitized.txt").write_text(runtime_log, encoding="utf-8")
            facts["runtime_log_sha256"] = base.sha256_text(runtime_log)
            facts["descriptor_first_document"] = descriptor_first.get("document")
            facts["descriptor_second_document"] = descriptor_second.get("document")
    finally:
        if app is not None:
            with contextlib.suppress(Exception):
                app.force_stop()
        with contextlib.suppress(Exception):
            server.stop(clean=False)
        with contextlib.suppress(Exception):
            base.cleanup_credential_store(ENDPOINT)
        if (run_work / "localappdata").exists():
            shutil.rmtree(run_work / "localappdata", ignore_errors=True)

    facts["finished_at"] = base.now_iso()
    facts["files"] = {
        "screenshots": [
            name
            for name in ("connected.png", "withheld.png", "recovered.png")
            if (evidence / name).is_file()
        ],
        "evidence_dir": f"docs/development/evidence-published/rt3-rt8-latest/{label}",
    }
    for name in facts["files"]["screenshots"]:
        facts.setdefault("file_hashes", {})[name] = base.sha256_file(evidence / name)
    for name in (
        "uia-connected.json",
        "uia-window.json",
        "uia-withheld.json",
        "shot-connected.json",
        "shot-withheld.json",
        "shot-recovered.json",
    ):
        if (evidence / name).is_file():
            facts.setdefault("file_hashes", {})[name] = base.sha256_file(evidence / name)
    base.write_json(evidence / "facts.json", facts)
    return facts


def summarize(runs: list[dict]) -> dict:
    scenarios = {}
    for scenario in SCENARIO_ORDER:
        verdicts = [run["scenarios"].get(scenario, {}).get("verdict") for run in runs]
        scenarios[scenario] = {
            "verdicts": verdicts,
            "passed": bool(verdicts) and all(verdict == "PASS" for verdict in verdicts),
        }
    return {
        "scenarios": scenarios,
        "all_required_passed": bool(runs) and all(entry["passed"] for entry in scenarios.values()),
        "epoch_changed_all_runs": all(run.get("epoch_changed") for run in runs),
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runs", type=int, default=1, choices=(1, 2, 3))
    parser.add_argument("--label-prefix", default="run")
    parser.add_argument("--no-build", action="store_true", help="reuse the existing release binary")
    args = parser.parse_args(argv)

    for required in (RUNTIME_PYTHON, SOURCE_DB, DESKTOP_EXE, PROBE):
        if not required.is_file():
            print(f"missing required path: {required}", file=sys.stderr)
            return 2

    EVIDENCE_DIR.mkdir(parents=True, exist_ok=True)
    WORK_ROOT.mkdir(parents=True, exist_ok=True)
    # A previous failed attempt's diagnostic must not survive a successful run.
    for stale in ("failure.json", "build-failure.json"):
        (EVIDENCE_DIR / stale).unlink(missing_ok=True)
    build_facts: dict = {"performed": False, "attempts": []}
    if not args.no_build:
        attempts: list[dict] = []
        stable = False
        for attempt in (1, 2):
            before = base.source_state()
            build = base.build_desktop()
            after = base.source_state()
            stable_now = before["fingerprint"] == after["fingerprint"]
            attempts.append(
                {
                    "attempt": attempt,
                    "exit": build["exit"],
                    "ok": build["ok"],
                    "source_stable": stable_now,
                    "tail": build["tail"],
                }
            )
            if build["ok"] and stable_now:
                stable = True
                break
        build_facts = {
            "performed": True,
            "attempts": attempts,
            "attempt_count": len(attempts),
            "retried": len(attempts) > 1,
        }
        if not stable:
            base.write_json(EVIDENCE_DIR / "build-failure.json", {"attempts": attempts})
            print("[harness] build failed or source changed", file=sys.stderr, flush=True)
            return 2

    secrets_list: list[str] = []
    runs: list[dict] = []
    error = None
    try:
        for index in range(args.runs):
            label = f"{args.label_prefix}{index + 1}"
            print(f"[harness] {label}: starting", flush=True)
            facts = run_once(label, secrets_list)
            runs.append(facts)
            print(
                f"[harness] {label}: "
                + ", ".join(
                    f"{name}={facts['scenarios'].get(name, {}).get('verdict')}"
                    for name in SCENARIO_ORDER
                ),
                flush=True,
            )
        summary = summarize(runs)
        session = {
            "measurement": "RT3 automatic reauthorization + RT8 service discovery live campaign",
            "date": base.now_iso(),
            "harness": {
                "command": "python apps/desktop/devtools/runtime_integration/run_rt3_rt8.py --runs "
                + str(args.runs),
                "port": PORT,
                "runs_requested": args.runs,
                "endpoint_passed_to_desktop": False,
                "token_passed_to_desktop": False,
                "discovery_redirect_env": "COS_RUNTIME_DISCOVERY_PATH",
                "run_secret_recorded": False,
            },
            "desktop": {
                "head": base.git_value(DESKTOP_ROOT, "rev-parse", "HEAD"),
                "binary": str(DESKTOP_EXE),
                "binary_sha256": base.sha256_file(DESKTOP_EXE),
                "build": build_facts,
                "source_at_end": base.source_state(),
            },
            "runtime": {
                "root": str(RUNTIME_ROOT),
                "head": base.git_value(RUNTIME_ROOT, "rev-parse", "HEAD"),
                "python": str(RUNTIME_PYTHON),
            },
            "runs": runs,
            "summary": summary,
        }
        base.write_json(EVIDENCE_DIR / "session.json", session)
        # Non-negotiable: no token or operator secret may appear in evidence.
        leaked = []
        for path in EVIDENCE_DIR.rglob("*"):
            if not path.is_file():
                continue
            data = path.read_bytes()
            for secret in secrets_list:
                if secret and secret.encode() in data:
                    leaked.append(str(path.relative_to(EVIDENCE_DIR)))
                    break
        if leaked:
            raise RuntimeError(f"token/secret material leaked into evidence: {sorted(set(leaked))}")
        print(
            f"[harness] summary: all_required_passed={summary.get('all_required_passed')} "
            f"epoch_changed_all_runs={summary.get('epoch_changed_all_runs')}",
            flush=True,
        )
    except Exception as error_object:  # noqa: BLE001 - harness records its own failure
        error = error_object
        print(f"[harness] error: {error_object}", file=sys.stderr, flush=True)
        base.write_json(
            EVIDENCE_DIR / "failure.json",
            {"error": type(error_object).__name__, "message": str(error_object)[:2000]},
        )
    return 0 if error is None and runs and summarize(runs).get("all_required_passed") else 1


if __name__ == "__main__":
    raise SystemExit(main())
