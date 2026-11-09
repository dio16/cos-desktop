#!/usr/bin/env python3
"""MP-ACC1 round-2 / reconnect repair: native same-process reconnect measurement.

Test-only, evidence-only. One isolated Runtime (port 8134, fresh read-only DB
copy, per-run secrets, redirected discovery descriptor) and **one** Desktop
process for both phases; no generation is performed.

1. ``automatic`` — the Desktop connects through discovery, the Runtime is stopped
   cleanly and stays down past the client's bounded recovery budget (the round-2
   loss condition), then restarted on the same discovery path under a new epoch.
   The record measures whether the *same* process reconnected with no user action.
2. ``explicit`` — after a second, longer outage (so the supervisor's pending
   backoff is at its cap), the Runtime is restarted and the native
   ``再接続する`` action is invoked immediately. The record measures the
   attempts before and after the action and the click-to-observed interval, so a
   recovery caused by the action is distinguishable from a scheduled one.

No task state is claimed beyond what the publication rendered and nothing is
inferred from UI text alone.

Run from the repository root with the Runtime's interpreter::

    C:/AI/orchestrator/.venv/Scripts/python.exe -u \\
      apps/desktop/devtools/runtime_integration/run_reconnect_native.py \\
      --evidence-dir docs/development/evidence-published/reconnect-native-latest
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import json
import shutil
import sys
import time
from pathlib import Path

HARNESS = Path(__file__).resolve().parent
sys.path.insert(0, str(HARNESS))

import run_acc1_native as native  # noqa: E402 - local test helper, stdlib only
import run_integration as base  # noqa: E402 - local test helper, stdlib only

ROOT = native.ROOT
DESKTOP_EXE = native.DESKTOP_EXE
PORT = 8134
# Five bounded client recovery attempts (0.5 s .. 8 s) finish inside ~16 s; the
# round-2 loss used a 23 s outage. 30 s guarantees the terminal client state.
AUTOMATIC_OUTAGE_SECONDS = 30.0
# A second, longer outage grows the supervisor's pending delay to its 15 s cap.
EXPLICIT_OUTAGE_SECONDS = 45.0
POST_RESTART_SECONDS = 75.0
ATTEMPT_LINE = "Runtime rediscover"


def now_iso() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%S%z")


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def write_json(path: Path, document: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(document, ensure_ascii=False, indent=2, sort_keys=True),
                    encoding="utf-8")


def file_size(path: Path) -> int:
    return path.stat().st_size if path.is_file() else 0


def read_since(path: Path, offset: int, limit: int = 400_000) -> str:
    if not path.is_file():
        return ""
    with path.open("rb") as handle:
        handle.seek(offset)
        return handle.read(limit).decode("utf-8", errors="replace")


def http_lines(text: str, limit: int = 120) -> list[str]:
    lines = [line.strip() for line in text.splitlines() if "HTTP/1.1" in line]
    return lines[-limit:]


def attempt_count(text: str) -> int:
    return sum(1 for line in text.splitlines() if ATTEMPT_LINE in line)


def redact(text: str, secrets_list: list[str]) -> str:
    for secret in secrets_list:
        if secret:
            text = text.replace(secret, "<redacted>")
    return text


def connected_predicate(value: str) -> bool:
    """An observing session: the freshness label says data is flowing and no
    unavailable/reauth surface is present."""
    return ("情報を受信中" in value
            and "情報源なし" not in value
            and "再接続する" not in value
            and "データは非公開" not in value)


def withheld_predicate(value: str) -> bool:
    """A withheld publication: the round-2 unavailable surface."""
    return ("情報源なし" in value
            or "再接続する" in value
            or "データは非公開" in value)


def phase_automatic(ctx: native.Ctx, record: dict, secrets_list: list[str]) -> None:
    ui = ctx.app.ui
    assert ui is not None
    runtime_offset = file_size(ctx.runtime.log_path)
    app_offset = file_size(ctx.app.log_path)
    stop_started = time.monotonic()
    record["phases"]["automatic"] = {
        "outage_seconds": AUTOMATIC_OUTAGE_SECONDS,
        "runtime_stop": {
            "at": now_iso(), "result": ctx.runtime.stop(),
            "discovery_present": ctx.runtime.discovery_path.exists(),
            "port_free": native.wait_port_free(ctx.port, 60.0),
        },
    }
    lost = ui.wait_names(withheld_predicate, 60.0, "withheld")
    phase = record["phases"]["automatic"]
    # The withheld state appears within a couple of seconds; the outage itself must
    # still last longer than the client's bounded recovery budget (~16 s), which is
    # the exact round-2 loss condition.
    remaining = AUTOMATIC_OUTAGE_SECONDS - (time.monotonic() - stop_started)
    if remaining > 0:
        time.sleep(remaining)
    phase["actual_outage_seconds"] = round(time.monotonic() - stop_started, 2)
    phase["withheld"] = {
        "found": lost[2], "at": now_iso(),
        "excerpt": native.bounded("\n".join(
            line for line in lost[1].splitlines()
            if "再接続" in line or "情報源" in line or "非公開" in line), 1500),
        "uia": native.uia_snapshot(ui, "automatic-withheld"),
    }
    stop_app_offset = file_size(ctx.app.log_path)
    phase["desktop_attempts_while_down"] = attempt_count(
        read_since(ctx.app.log_path, app_offset))
    restarted = ctx.runtime.start()
    record["epochs"]["automatic"] = restarted.get("epoch")
    phase["runtime_restart"] = {
        "at": now_iso(), "pid": restarted.get("pid"), "epoch": restarted.get("epoch"),
        "endpoint": restarted.get("endpoint"),
        "discovery_present": ctx.runtime.discovery_path.exists(),
        "same_discovery_path": str(ctx.runtime.discovery_path),
        "epoch_changed": restarted.get("epoch") != record["runtime_start"].get("epoch"),
    }
    restart_offset = file_size(ctx.runtime.log_path)
    started = time.monotonic()
    _dump, auto_text, connected = ui.wait_names(
        connected_predicate, POST_RESTART_SECONDS, "automatic")
    phase["result"] = {
        "observed": connected, "seconds": round(time.monotonic() - started, 2),
        "at": now_iso(),
        "excerpt": native.bounded("\n".join(
            line for line in auto_text.splitlines()
            if "タスク" in line or "接続" in line or "再接続" in line), 2000),
        "uia": native.uia_snapshot(ui, "automatic-connected"),
    }
    phase["serving_http"] = http_lines(read_since(ctx.runtime.log_path, restart_offset))
    phase["desktop_log"] = native.bounded(read_since(ctx.app.log_path, stop_app_offset), 8000)
    phase["attempts_after_restart"] = attempt_count(
        read_since(ctx.app.log_path, stop_app_offset))


def phase_explicit(ctx: native.Ctx, record: dict) -> None:
    ui = ctx.app.ui
    assert ui is not None
    phase = record["phases"].setdefault("explicit", {})
    phase["outage_seconds"] = EXPLICIT_OUTAGE_SECONDS
    app_offset = file_size(ctx.app.log_path)
    runtime_offset = file_size(ctx.runtime.log_path)
    phase["runtime_stop"] = {
        "at": now_iso(), "result": ctx.runtime.stop(),
        "discovery_present": ctx.runtime.discovery_path.exists(),
        "port_free": native.wait_port_free(ctx.port, 60.0),
    }
    lost = ui.wait_names(withheld_predicate, 60.0, "withheld-explicit")
    phase["withheld"] = {
        "found": lost[2], "at": now_iso(),
        "uia": native.uia_snapshot(ui, "explicit-withheld"),
    }
    down_offset = file_size(ctx.app.log_path)
    # Wait out the outage so the client is terminal and the supervisor's pending
    # delay grows to its cap.
    deadline = time.monotonic() + EXPLICIT_OUTAGE_SECONDS
    while time.monotonic() < deadline:
        time.sleep(1.0)
    phase["attempts_while_down"] = attempt_count(read_since(ctx.app.log_path, app_offset))
    # Restart immediately after a *recorded failed attempt*: the next scheduled one
    # is then a full backoff away, so a recovery right after the click can only be
    # the forced attempt and not an overlapping scheduled one.
    before = attempt_count(read_since(ctx.app.log_path, down_offset))
    wait_until = time.monotonic() + 60.0
    while time.monotonic() < wait_until:
        if attempt_count(read_since(ctx.app.log_path, down_offset)) > before:
            break
        time.sleep(0.2)
    restarted = ctx.runtime.start()
    record["epochs"]["explicit"] = restarted.get("epoch")
    phase["runtime_restart"] = {
        "at": now_iso(), "pid": restarted.get("pid"), "epoch": restarted.get("epoch"),
        "endpoint": restarted.get("endpoint"),
        "epoch_changed": restarted.get("epoch") != record["epochs"].get("automatic"),
    }
    restart_offset = file_size(ctx.runtime.log_path)
    # The restart-to-click window: a scheduled attempt here would already have
    # recovered the session, so the action's own attempt stays distinguishable.
    pre_click_dump = ui.dump()
    pre_click_names = "\n".join(pre_click_dump.get("names") or [])
    click_app_offset = file_size(ctx.app.log_path)
    click_runtime_offset = file_size(ctx.runtime.log_path)
    started = time.monotonic()
    action = ui.invoke("再接続する")
    phase["manual_action"] = {
        "at": now_iso(), "invoked": action.get("invoked"), "found": action.get("found"),
        "matched_name": action.get("matched_name"),
        "connected_before_click": connected_predicate(pre_click_names),
        "attempts_between_restart_and_click": attempt_count(
            read_since(ctx.app.log_path, click_app_offset)),
        "restart_to_click_seconds": round(time.monotonic() - started, 3),
    }
    _dump, text, connected = ui.wait_names(
        connected_predicate, POST_RESTART_SECONDS, "explicit-connected")
    phase["result"] = {
        "observed": connected, "seconds": round(time.monotonic() - started, 2),
        "at": now_iso(),
        "attempts_after_click": attempt_count(
            read_since(ctx.app.log_path, click_app_offset)),
        "excerpt": native.bounded("\n".join(
            line for line in text.splitlines()
            if "タスク" in line or "接続" in line or "再接続" in line), 2000),
        "uia": native.uia_snapshot(ui, "explicit-connected"),
    }
    phase["serving_http_after_click"] = http_lines(
        read_since(ctx.runtime.log_path, click_runtime_offset))
    phase["desktop_log_after_click"] = native.bounded(
        read_since(ctx.app.log_path, click_app_offset), 8000)
    phase["desktop_log_after_restart"] = native.bounded(
        read_since(ctx.app.log_path, restart_offset), 8000)
    phase["serving_http_before_click"] = http_lines(
        read_since(ctx.runtime.log_path, restart_offset))
    phase["desktop_attempts_while_down"] = attempt_count(
        read_since(ctx.app.log_path, down_offset))


def main(argv: list[str] | None = None) -> int:
    global AUTOMATIC_OUTAGE_SECONDS, EXPLICIT_OUTAGE_SECONDS
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence-dir", required=True)
    parser.add_argument("--desktop-exe", default=str(DESKTOP_EXE))
    parser.add_argument("--port", type=int, default=PORT)
    parser.add_argument("--automatic-outage", type=float, default=AUTOMATIC_OUTAGE_SECONDS)
    parser.add_argument("--explicit-outage", type=float, default=EXPLICIT_OUTAGE_SECONDS)
    args = parser.parse_args(argv)
    AUTOMATIC_OUTAGE_SECONDS = args.automatic_outage
    EXPLICIT_OUTAGE_SECONDS = args.explicit_outage
    native.DESKTOP_EXE = Path(args.desktop_exe)

    evidence = (ROOT / args.evidence_dir).resolve()
    evidence.mkdir(parents=True, exist_ok=True)
    for name in ("logs", "uia", "shots"):
        (evidence / name).mkdir(exist_ok=True)

    ctx = native.Ctx("runtime-reconnect", args.port)
    record: dict = {
        "label": "runtime-reconnect", "started_at": now_iso(), "port": args.port,
        "desktop_exe": str(native.DESKTOP_EXE),
        "desktop_exe_sha256": sha256_file(Path(native.DESKTOP_EXE)),
        "desktop_exe_bytes": Path(native.DESKTOP_EXE).stat().st_size,
        "desktop_commit": native.commit_of_desktop_sources() if hasattr(
            native, "commit_of_desktop_sources") else "",
        "steps": {}, "epochs": {}, "phases": {}, "cleanup": {},
        "verdicts": {}, "residuals": [],
    }
    secrets_list = list(ctx.collect)
    try:
        record["runtime_start"] = ctx.start_runtime(wait_generation=False)
        record["epochs"]["startup"] = record["runtime_start"].get("epoch")
        record["steps"]["connected"] = ctx.launch_desktop("reconnect")
        ui = ctx.app.ui
        assert ui is not None
        record["steps"]["connected_uia"] = native.uia_snapshot(ui, "connected")

        phase_automatic(ctx, record, secrets_list)
        explicit_ready = bool(record["phases"]["automatic"]["result"]["observed"])
        if explicit_ready:
            phase_explicit(ctx, record)
        else:
            record["residuals"].append(
                "the explicit action was not exercised because the automatic path did not"
                " recover the session; a failed automatic path cannot be used to measure"
                " the forced attempt")

        descriptor = {}
        with contextlib.suppress(OSError, json.JSONDecodeError):
            descriptor = json.loads(ctx.runtime.discovery_path.read_text(encoding="utf-8"))
        record["discovery_descriptor"] = {
            "keys": sorted(descriptor.keys()),
            "server_identity": descriptor.get("server_identity"),
            "epoch": descriptor.get("epoch"),
            "origin": descriptor.get("origin"),
            "path": str(ctx.runtime.discovery_path),
        }

        record["cleanup"]["desktop"] = ctx.app.close()
        record["cleanup"]["runtime"] = ctx.runtime.stop()
        record["cleanup"]["port_free"] = native.wait_port_free(args.port, 60.0)
        record["cleanup"]["discovery_present_after_stop"] = ctx.runtime.discovery_path.exists()
        record["cleanup"]["credential_cleanup"] = base.cleanup_credential_store(
            ctx.runtime.endpoint)
    finally:
        with contextlib.suppress(Exception):
            if ctx.app is not None and ctx.app.alive():
                ctx.app.force_stop()
        with contextlib.suppress(Exception):
            if ctx.runtime.process is not None and ctx.runtime.process.poll() is None:
                ctx.runtime.stop()
        record["finished_at"] = now_iso()

    automatic = bool(record["phases"].get("automatic", {}).get("result", {}).get("observed"))
    explicit = record["phases"].get("explicit", {}).get("result", {}).get("observed")
    action = record["phases"].get("explicit", {}).get("manual_action") or {}
    # A recovery right after the click only counts as the action's if no scheduled
    # attempt could have preceded it and the UI was still withheld at click time.
    attributed = bool(
        explicit
        and not action.get("connected_before_click", True)
        and action.get("attempts_between_restart_and_click") == 0)
    record["verdicts"] = {
        "automatic_reconnect": {"observed": automatic},
        "explicit_reconnect": {
            "observed": explicit,
            "attributed_to_the_action": attributed,
            "connected_before_click": action.get("connected_before_click"),
            "attempts_between_restart_and_click":
                action.get("attempts_between_restart_and_click"),
        },
    }
    record["verdict"] = (
        "PASS" if automatic and attributed else
        "PARTIAL" if automatic or explicit else "FAIL")

    logs = evidence / "logs"
    for name, source in (("runtime.log", ctx.runtime.log_path),
                         ("desktop.log", ctx.app.log_path if ctx.app else None)):
        if source is None or not source.is_file():
            continue
        text = redact(source.read_text(encoding="utf-8", errors="replace"), secrets_list)
        (logs / name).write_text(text, encoding="utf-8")
    for directory, target in (("uia", evidence / "uia"), ("shots", evidence / "shots")):
        source_dir = ctx.work / directory
        if source_dir.is_dir():
            for path in sorted(source_dir.glob("*")):
                if path.is_file():
                    shutil.copyfile(path, target / path.name)
    write_json(evidence / "record-runtime-reconnect.json", record)
    (evidence / "README.md").write_text(render_readme(record), encoding="utf-8")
    files = {
        str(path.relative_to(evidence)): {
            "bytes": path.stat().st_size, "sha256": sha256_file(path)}
        for path in sorted(evidence.rglob("*"))
        if path.is_file() and path.name != "manifest.json"
    }
    write_json(evidence / "manifest.json", {"files": files})
    findings = []
    for path in sorted(evidence.rglob("*")):
        if not path.is_file():
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        for index, secret in enumerate(secrets_list):
            if secret and secret in text:
                findings.append({"file": str(path.relative_to(evidence)), "secret": index})
    write_json(evidence / "leak-scan.json",
               {"checked_files": len(files), "findings": findings})
    print(json.dumps({"verdict": record["verdict"], "automatic": automatic,
                      "explicit": explicit, "evidence": str(evidence),
                      "leak_findings": len(findings)}, ensure_ascii=False))
    return 0


def render_readme(record: dict) -> str:
    automatic = record["phases"].get("automatic", {})
    explicit = record["phases"].get("explicit", {})
    auto_result = automatic.get("result") or {}
    explicit_result = explicit.get("result") or {}
    epochs = record["epochs"]
    return f"""# Native same-process Runtime reconnect (2026-11-10, MP-ACC1 round-2 repair)

Evidence-only run of the Desktop Runtime-reconnect repair. No generation was
performed and no task state is claimed beyond what the publication rendered.
One Desktop process served both phases; only the Runtime was restarted.

- Desktop binary: `{record["desktop_exe"]}`, {record["desktop_exe_bytes"]} B,
  sha256 `{record["desktop_exe_sha256"]}`.
- Isolated Runtime on port {record["port"]} (`C:/AI/orchestrator`), fresh
  read-only DB copy, per-run `ORCHESTRATOR_AUTH_SECRET` / media-handle secret and
  a redirected discovery descriptor. The Desktop ran with **no** endpoint or
  token environment: only the per-run issuance secret and the discovery path.
- Epochs: startup `{epochs.get("startup")}`, after the phase-1 restart
  `{epochs.get("automatic")}`, after the phase-2 restart `{epochs.get("explicit")}`.
- Every restart reused the **same** discovery path and port; each new Runtime
  process advertised a new epoch in the descriptor.

## 1 — Automatic reconnect (no user action)

- The Runtime was stopped cleanly and stayed down for
  {automatic.get("outage_seconds")} s — longer than the client's five bounded
  recovery attempts (~16 s), which is the exact round-2 loss condition.
- Withheld state observed in the UI: `{bool((automatic.get("withheld") or {}).get("found"))}`
  (`情報源なし / データ非公開` + `再接続する`).
- Desktop attempts while the Runtime was down: {automatic.get("desktop_attempts_while_down")}
  (the Desktop keeps retrying instead of staying terminal).
- After the restart the **same** process recovered with no user action:
  `{auto_result.get("observed")}` in {auto_result.get("seconds")} s.
- Serving requests for that recovery: `{" | ".join(automatic.get("serving_http") or [])}`
  — the expected `observation 401` → `credentials/refresh 401` →
  `credentials 200` → fresh `observation 200` → `deltas 200` sequence.

## 2 — Explicit `再接続する` action

- Second outage: {explicit.get("outage_seconds")} s (the supervisor's pending
  delay grows to its 15 s cap).
- Attempts while down: {explicit.get("desktop_attempts_while_down")}; invocations
  of `再接続する`:
  `{explicit.get("manual_action")}`.
- Attempts between the Runtime restart and the click:
  `{(explicit.get("manual_action") or {}).get("attempts_between_restart_and_click")}`
  (a scheduled attempt in that window could not have been the recovery).
- Result: `{explicit_result.get("observed")}` in
  {explicit_result.get("seconds")} s after the click, with
  {explicit_result.get("attempts_after_click")} fresh attempt(s) recorded.

## Verdict

**{record["verdict"]}** — automatic `{record["verdicts"]["automatic_reconnect"]["observed"]}`,
explicit `{record["verdicts"]["explicit_reconnect"]["observed"]}`
(attributed to the action:
`{record["verdicts"]["explicit_reconnect"]["attributed_to_the_action"]}`, i.e. the UI was still
withheld at click time and no scheduled attempt intervened).

Raw (redacted) Desktop/Runtime logs, UIA dumps and screenshots are in `logs/`,
`uia/` and `shots/`; `manifest.json` pins the size/sha256 of every file and
`leak-scan.json` records the per-run secret scan. Cleanup: Desktop
`{record["cleanup"].get("desktop")}`, Runtime `{record["cleanup"].get("runtime")}`.

Residuals: {"; ".join(record["residuals"]) if record["residuals"] else "none recorded"}.
"""


if __name__ == "__main__":
    raise SystemExit(main())
