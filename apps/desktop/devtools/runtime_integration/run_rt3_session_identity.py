#!/usr/bin/env python3
"""RT3 media session-identity native measurement.

Measures the repair of the same-process Runtime-restart media defect recorded in
``docs/development/evidence-published/media-expiry-native-latest/defect.json``:

1. an isolated Runtime (fresh copy of a prior isolated closure run's DB/CAS; no
   generation) serves one already-admitted artifact on port 8167;
2. the native Desktop displays that artifact through the real media boundary;
3. the Runtime is cleanly stopped and restarted on the same DB copy, minting a
   new epoch, a new observation context and a new Desktop principal;
4. the same Desktop process re-enters the Artifact surface and re-resolves the
   artifact: the media descriptor must answer 200 (never 401) and the native
   image must be displayed again without a persistent ``Forbidden`` state.

The Desktop binary is passed with ``--desktop-exe`` so the repaired build is
never confused with the pre-fix release binary. Runtime/Desktop logs are
sanitized; tokens, principals and access handles are never written to evidence.
The pre-fix negative is not re-run here: it is the already published
``media-expiry-native-latest`` measurement, whose product source is identical
for the defect path; its ``defect.json`` hash is recorded.

Usage (from the repository root)::

    python apps/desktop/devtools/runtime_integration/run_rt3_session_identity.py \\
        --desktop-exe C:/tmp/rt3-session-identity/target/release/cos-desktop.exe
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import time
from pathlib import Path

HARNESS = Path(__file__).resolve().parent
sys.path.insert(0, str(HARNESS))
import run_acc1_native as acc1  # noqa: E402 - local test helper, stdlib only
import run_integration as base  # noqa: E402 - local test helper, stdlib only
import run_rt3_residual as residual  # noqa: E402 - local test helper, stdlib only

ROOT = base.DESKTOP_ROOT
RUNTIME_ROOT = base.RUNTIME_ROOT
EVIDENCE = (
    ROOT / "docs" / "development" / "evidence-published" / "media-session-identity-latest"
)
WORK_ROOT = ROOT / "target" / "media-session-identity"
SOURCE_RUN_CANDIDATES = (
    ROOT / "target" / "acc1-native" / "20261109T233420-golden",
    Path("C:/dev/cos-desktop/target/acc1-native/20261109T233420-golden"),
)
SOURCE_RUN = next(
    (candidate for candidate in SOURCE_RUN_CANDIDATES if candidate.is_dir()),
    SOURCE_RUN_CANDIDATES[0],
)
PREFIX_EVIDENCE = (
    ROOT / "docs" / "development" / "evidence-published" / "media-expiry-native-latest"
)
TASK_REF = "task-ab47119a8bec467a8e128d7b82f1220a"
PORT = 8167

MEDIA_DISPLAYED_MARKER = "メディア画像（"
MEDIA_FORBIDDEN_TEXT = (
    "画像を利用できません: このセッションにはメディアの閲覧権限がありません。"
)
MEDIA_ACCESS = re.compile(
    r'"(?:GET|HEAD) (?P<path>/api/cos/media/[^ ]*) HTTP/1\.1" (?P<status>\d{3})'
)
RECONNECT_CREDENTIAL = 'POST /api/cos/desktop/credentials HTTP/1.1" 200'
RECONNECT_OBSERVATION = 'POST /api/cos/desktop/observation HTTP/1.1" 200'


# --------------------------------------------------------------------------- basics


def now_iso() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%S%z")


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return "sha256:" + digest.hexdigest()


def sha256_bytes(raw: bytes) -> str:
    return "sha256:" + hashlib.sha256(raw).hexdigest()


def write_json(path: Path, document: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(document, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )


def run_git(*args: str, cwd: Path = ROOT) -> str:
    completed = subprocess.run(
        ["git", *args], capture_output=True, text=True, timeout=30, cwd=str(cwd)
    )
    return (completed.stdout or "").lstrip("\ufeff").rstrip("\r\n")


def port_free(port: int) -> bool:
    probe = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    try:
        probe.settimeout(0.5)
        return probe.connect_ex(("127.0.0.1", port)) != 0
    finally:
        probe.close()


def wait_port_free(port: int, timeout: float = 30.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if port_free(port):
            return True
        time.sleep(0.2)
    return port_free(port)


def media_lines(names: list[str], limit: int = 40) -> list[str]:
    return [
        line
        for line in names
        if any(
            needle in line
            for needle in ("メディア", "画像", "成果物", "記述子", "再取得", "閲覧権限")
        )
    ][:limit]


def media_state(names: list[str]) -> str:
    text = "\n".join(names)
    if MEDIA_DISPLAYED_MARKER in text:
        return "DISPLAYED"
    if MEDIA_FORBIDDEN_TEXT in text:
        return "FORBIDDEN"
    if "メディア画像を準備中" in text:
        return "PREPARING"
    if "画像を利用できません" in text:
        return "UNAVAILABLE"
    return "UNKNOWN"


def capture(ui: acc1.Ui, label: str) -> dict:
    """Persist one UIA dump and screenshot under the evidence directory."""
    uia_dir = EVIDENCE / "uia"
    shot_dir = EVIDENCE / "shots"
    uia_dir.mkdir(parents=True, exist_ok=True)
    shot_dir.mkdir(parents=True, exist_ok=True)
    dump = ui.probe("dump")
    raw = {key: value for key, value in dump.items() if not key.startswith("_")}
    uia_path = uia_dir / f"{label}.json"
    uia_path.write_text(
        json.dumps(raw, ensure_ascii=False, indent=1) + "\n", encoding="utf-8"
    )
    image = shot_dir / f"{label}.png"
    shot = ui.probe("shot", shot=image)
    names = list(dump.get("names") or [])
    return {
        "label": label,
        "at": now_iso(),
        "state": media_state(names),
        "elements": dump.get("elements"),
        "window": {
            "title": dump.get("title"),
            "class": dump.get("class"),
            "width": dump.get("width"),
            "height": dump.get("height"),
        },
        "uia_file": str(uia_path.relative_to(EVIDENCE)),
        "uia_sha256": sha256_file(uia_path),
        "names_sha256": sha256_bytes("\n".join(names).encode("utf-8")),
        "names_count": len(names),
        "media_lines": media_lines(names),
        "has_displayed_marker": MEDIA_DISPLAYED_MARKER in "\n".join(names),
        "forbidden_text_present": MEDIA_FORBIDDEN_TEXT in "\n".join(names),
        "screenshot": {
            "file": str(image.relative_to(EVIDENCE)),
            "sha256": shot.get("_image_sha256"),
            "bytes": shot.get("_image_bytes"),
            "printed": shot.get("ok"),
            "width": shot.get("width"),
            "height": shot.get("height"),
        },
    }


def wait_for_media_state(
    ui: acc1.Ui, targets: tuple[str, ...], timeout: float
) -> tuple[list[dict], dict, list[str]]:
    """Poll UIA until one target state appears; retain the observed transitions."""
    deadline = time.monotonic() + timeout
    transitions: list[dict] = []
    dump: dict = {}
    names: list[str] = []
    seen = ""
    while time.monotonic() < deadline:
        dump = ui.probe("dump")
        names = list(dump.get("names") or [])
        state = media_state(names)
        if state != seen:
            transitions.append({"at": now_iso(), "state": state})
            seen = state
            if state in targets:
                break
        if not base.pid_alive(ui.pid):
            break
        time.sleep(1.5)
    return transitions, dump, names


def log_since(path: Path, offset: int) -> str:
    with contextlib.suppress(OSError):
        with path.open("r", encoding="utf-8", errors="replace") as handle:
            handle.seek(offset)
            return handle.read()
    return ""


def media_access(lines: str) -> dict:
    entries: list[dict] = []
    for line in lines.splitlines():
        match = MEDIA_ACCESS.search(line)
        if not match:
            continue
        path = match.group("path")
        entries.append(
            {
                "route": "blob"
                if "/blob" in path or "/thumbnail" in path
                else "descriptor",
                "status": int(match.group("status")),
                "line": line.strip()[-240:],
            }
        )
    return {
        "entries": entries,
        "descriptor_statuses": [
            entry["status"] for entry in entries if entry["route"] == "descriptor"
        ],
        "blob_statuses": [
            entry["status"] for entry in entries if entry["route"] == "blob"
        ],
    }


def invoke_or_click(ui: acc1.Ui, label: str) -> dict:
    invoked = ui.invoke(label)
    if invoked.get("invoked"):
        return {"method": "uia_invoke", "result": invoked}
    clicked = ui.click(label)
    return {"method": "wm_lbutton_at_uia_bounds", "result": clicked}


def sanitize_and_copy(source: Path, destination: Path, secrets_list: list[str]) -> dict:
    text = source.read_text(encoding="utf-8", errors="replace")
    sanitized, redactions = residual.sanitized_log(text, secrets_list)
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(sanitized, encoding="utf-8")
    return {
        "source": str(source),
        "file": destination.name,
        "sha256": sha256_file(destination),
        "bytes": destination.stat().st_size,
        "redactions": redactions,
    }


def leak_scan(evidence: Path, secrets_list: list[str]) -> dict:
    findings: list[str] = []
    checked = 0
    for path in sorted(evidence.rglob("*")):
        if not path.is_file() or path.suffix not in {".json", ".md", ".txt", ".sha256"}:
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        checked += 1
        for secret in secrets_list:
            if secret and secret in text:
                findings.append(f"{path.name}: exact secret material")
        if residual.HANDLE_PATTERN.search(text):
            findings.append(f"{path.name}: access-handle value")
        if re.search(r"Bearer\s+[A-Za-z0-9._\-]{16,}", text):
            findings.append(f"{path.name}: bearer token")
        if re.search(r"desktop:[0-9a-f]{32}", text):
            findings.append(f"{path.name}: unredacted Desktop principal")
    return {"files_checked": checked, "findings": findings, "clean": not findings}


def safe_descriptor(descriptor: dict) -> dict:
    """Descriptor evidence with the principal claim reduced to a short hash."""
    summary = residual.descriptor_summary(descriptor)
    audience = summary.pop("audience", None)
    if audience:
        summary["audience_sha256_16"] = (
            "sha256:" + hashlib.sha256(str(audience).encode("utf-8")).hexdigest()[:16]
        )
    return summary


def pre_fix_reference() -> dict:
    defect = PREFIX_EVIDENCE / "defect.json"
    record = PREFIX_EVIDENCE / "record-native-expiry.json"
    reference: dict = {
        "evidence": str(PREFIX_EVIDENCE.relative_to(ROOT)),
        "note": (
            "published pre-fix negative from the same product source for the defect"
            " path; not re-run in this session"
        ),
    }
    if defect.is_file():
        reference["defect_sha256"] = sha256_file(defect)
        document = json.loads(defect.read_text(encoding="utf-8"))
        observed = document.get("observed") or {}
        reference["observed"] = {
            "status": document.get("status"),
            "media_descriptor_http_status": observed.get(
                "media_descriptor_http_status"
            ),
            "native_state": observed.get("native_state"),
            "native_sentence": observed.get("native_sentence"),
            "image_displayed": observed.get("image_displayed"),
            "retry_observed": observed.get("retry_observed"),
        }
    if record.is_file():
        reference["record_sha256"] = sha256_file(record)
    return reference


# --------------------------------------------------------------------------- native run


def scenario(*, desktop_exe: Path, label: str) -> dict:
    started_at = now_iso()
    work = WORK_ROOT / f"{time.strftime('%Y%m%dT%H%M%S')}-{label}"
    work.mkdir(parents=True, exist_ok=True)
    acc1.WORK_ROOT = work / "ctx"
    acc1.DESKTOP_EXE = desktop_exe

    record: dict = {
        "schema_version": 1,
        "kind": "native-measurement",
        "label": label,
        "started_at": started_at,
        "verdict": "NOT_RUN",
        "precondition": {},
        "before_restart": {},
        "restart": {},
        "reconnect": {},
        "after_restart": {},
        "protocol_after_restart": {},
        "prefix_reference": pre_fix_reference(),
        "cleanup": {},
        "residuals": [],
    }
    secrets_list: list[str] = []
    ctx = acc1.Ctx(label, PORT, control=False, source_run=SOURCE_RUN)
    ui: acc1.Ui | None = None
    try:
        start1 = ctx.start_runtime(wait_generation=False)
        secrets_list.extend([ctx.secret, ctx.handle_secret, ctx.token])
        deadline = time.monotonic() + 30.0
        while time.monotonic() < deadline and not ctx.runtime.discovery_path.is_file():
            time.sleep(0.2)
        record["precondition"] = {
            "task_ref": TASK_REF,
            "source_run": str(SOURCE_RUN),
            "desktop_binary": {
                "path": str(desktop_exe),
                "sha256": sha256_file(desktop_exe),
                "bytes": desktop_exe.stat().st_size,
                "built_at": time.strftime(
                    "%Y-%m-%dT%H:%M:%S",
                    time.localtime(desktop_exe.stat().st_mtime),
                ),
            },
            "desktop_head": run_git("rev-parse", "HEAD"),
            "desktop_status_porcelain": run_git("status", "--porcelain"),
            "runtime_head": run_git("rev-parse", "HEAD", cwd=RUNTIME_ROOT),
            "runtime_epoch_1": start1["epoch"],
            "runtime_server_identity": start1["server_identity"],
            "db_copy_sha256": ctx.record.get("db_copy_sha256"),
            "db_copy_bytes": ctx.record.get("db_copy_bytes"),
            "discovery_only": True,
            "port": PORT,
            "generation_executed": False,
        }
        task = ctx.projection_task(TASK_REF)
        goal = str(task.get("title") or "")
        record["precondition"]["task_projection"] = {
            key: task.get(key)
            for key in ("title", "task_status", "inner_phase", "capabilities_ref")
        }
        if not goal:
            record["residuals"].append("the reused DB carries no TASK projection")
            return record
        capability = ctx.wait_capability_entry(
            TASK_REF, "media", "ORIGINAL", lambda support: support == "SUPPORTED", 45.0
        )
        record["precondition"]["capability_media"] = capability

        connected = ctx.launch_desktop(f"{label}-before")
        ui = ctx.app.ui if ctx.app is not None else None
        record["before_restart"]["connected"] = connected
        select = ctx.select_task(goal, f"{label}-task", TASK_REF)
        record["before_restart"]["task_selected"] = {
            key: select.get(key) for key in ("method", "viewed")
        }
        before_offset = ctx.runtime.log_path.stat().st_size
        view_before = ctx.view_artifact(TASK_REF, f"{label}-before")
        record["before_restart"]["artifact_surface"] = {
            key: view_before.get(key)
            for key in ("state", "viewport_present", "inspector_task_match")
        }
        record["before_restart"]["uia"] = capture(ui, "identity-before")
        resolved = ctx.resolve_artifact(TASK_REF)
        digest = str((resolved.get("artifact") or {}).get("artifact_digest") or "")
        record["before_restart"]["resolved_artifact"] = {
            "artifact": resolved.get("artifact"),
            "descriptor_http_status": resolved.get("descriptor_http_status"),
            "descriptor": safe_descriptor(resolved.get("descriptor") or {}),
        }
        if not digest:
            record["residuals"].append("the artifact digest was not projected")
            return record
        before_log = log_since(ctx.runtime.log_path, before_offset)
        record["before_restart"]["media_access"] = media_access(before_log)
        if view_before.get("state") not in {"DISPLAYED", "AVAILABLE"}:
            record["residuals"].append(
                f"the native artifact view did not display before the restart "
                f"({view_before.get('state')})"
            )
            return record

        # Stop and restart the Runtime on the same DB copy: a new epoch and a
        # new credential registry force the Desktop reconnect path.
        restart_offset = ctx.runtime.log_path.stat().st_size
        stop1 = ctx.runtime.stop()
        restart_at_unix = time.time()
        record["restart"]["stop_1"] = stop1
        record["restart"]["port_free_after_stop"] = wait_port_free(PORT, 30.0)
        start2 = ctx.runtime.start()
        record["restart"]["start_2"] = {
            "pid": start2.get("pid"),
            "epoch": start2.get("epoch"),
            "server_identity": start2.get("server_identity"),
            "epoch_changed": start2.get("epoch") != start1.get("epoch"),
            "at": now_iso(),
        }
        deadline = time.monotonic() + 30.0
        while time.monotonic() < deadline and not ctx.runtime.discovery_path.is_file():
            time.sleep(0.2)
        record["restart"]["discovery_reappeared"] = ctx.runtime.discovery_path.is_file()
        if not record["restart"]["start_2"]["epoch_changed"]:
            record["residuals"].append("the restarted Runtime reported the same epoch")

        # The Desktop reconnect supervisor re-runs discovery, authorization and
        # the client build. Wait for the new credential and observation before
        # touching the Artifact surface.
        reconnect: dict = {
            "at": now_iso(),
            "seen": False,
            "credential_200": False,
            "observation_200": False,
            "log_lines": [],
        }
        deadline = time.monotonic() + 150.0
        while time.monotonic() < deadline:
            new_log = log_since(ctx.runtime.log_path, restart_offset)
            reconnect["log_lines"] = [
                line
                for line in new_log.splitlines()
                if "desktop/credentials" in line or "desktop/observation" in line
            ][-16:]
            reconnect["credential_200"] = RECONNECT_CREDENTIAL in new_log
            reconnect["observation_200"] = RECONNECT_OBSERVATION in new_log
            if reconnect["credential_200"] and reconnect["observation_200"]:
                reconnect["seen"] = True
                break
            if not ctx.app.alive():
                break
            time.sleep(2.0)
        reconnect["after"] = now_iso()
        record["reconnect"] = reconnect

        # Let the reconnected publication settle so the viewed candidate and its
        # artifact projection are current before the Artifact surface is
        # re-entered. A missing candidate must be recorded, not papered over.
        settle: dict = {"at": now_iso(), "candidate_projected": False, "polls": 0}
        deadline = time.monotonic() + 90.0
        while time.monotonic() < deadline:
            settle["polls"] += 1
            names_now = list(ui.probe("dump").get("names") or []) if ui is not None else []
            if any("第0回" in line for line in names_now):
                settle["candidate_projected"] = True
                break
            if not ctx.app.alive():
                break
            time.sleep(1.5)
        settle["after"] = now_iso()
        record["reconnect"]["candidate_settle"] = settle

        # The reconnect reconciliation may move the local dock cursor to the
        # first row. Re-select the measured task so the displayed candidate and
        # the admitted image describe the same artifact before the final capture.
        reselect = ctx.select_task(goal, f"{label}-after-task", TASK_REF)
        record["reconnect"]["task_reselect"] = {
            key: reselect.get(key) for key in ("method", "selected", "scope_task_matched")
        }

        # Re-enter the Artifact surface in the same process. The shell re-keys
        # the media resolution on the new epoch/read-context/principal.
        reentry: dict = {"at": now_iso(), "actions": []}
        if ui is not None:
            reentry["actions"].append(
                {"action": "flow", "result": invoke_or_click(ui, "処理の流れ  Ctrl+3")}
            )
            time.sleep(0.7)
            reentry["log_offset"] = ctx.runtime.log_path.stat().st_size
            reentry["actions"].append(
                {"action": "artifact", "result": invoke_or_click(ui, "成果物  Ctrl+1")}
            )
        else:
            reentry["log_offset"] = ctx.runtime.log_path.stat().st_size

        # A native image marker alone could be the previous process image still
        # mounted. Require a descriptor+blob pair minted after the Runtime
        # restart; the shell may already have re-keyed and re-fetched while the
        # View transition was in flight, so the check starts at the restart
        # boundary rather than at the re-entry action.
        media_observed: dict = {"at": now_iso(), "found": False, "poll": 0}
        deadline = time.monotonic() + 150.0
        while time.monotonic() < deadline:
            media_observed["poll"] += 1
            new_log = log_since(ctx.runtime.log_path, restart_offset)
            access_now = media_access(new_log)
            if 200 in access_now["descriptor_statuses"] and 200 in access_now["blob_statuses"]:
                media_observed.update(
                    {
                        "found": True,
                        "access": access_now,
                        "elapsed_seconds": round(time.time() - restart_at_unix, 3),
                    }
                )
                break
            if not ctx.app.alive():
                break
            time.sleep(1.5)
        media_observed["after"] = now_iso()
        media_observed["after_reentry"] = media_access(
            log_since(ctx.runtime.log_path, reentry["log_offset"])
        )

        transitions, dump, names = wait_for_media_state(
            ui, ("DISPLAYED", "FORBIDDEN", "UNAVAILABLE"), 60.0
        )
        after_state = media_state(names)
        viewed_coherent = any(
            line.startswith("閲覧中の候補（この画面での選択）:")
            and "含まれていません" not in line
            for line in names
        )
        after_capture = capture(ui, "identity-after-restart")
        after_log = log_since(ctx.runtime.log_path, restart_offset)
        access = media_access(after_log)
        record["after_restart"] = {
            "at": now_iso(),
            "reentry": reentry,
            "media_refetch_after_restart": media_observed,
            "transitions": transitions,
            "final_state": after_state,
            "viewed_candidate_coherent": viewed_coherent,
            "displayed_marker": after_capture.get("has_displayed_marker"),
            "forbidden_text_present": after_capture.get("forbidden_text_present"),
            "uia": after_capture,
            "media_access": access,
            "descriptor_200_after_restart": 200 in access["descriptor_statuses"],
            "descriptor_401_after_restart": 401 in access["descriptor_statuses"],
            "blob_200_after_restart": 200 in access["blob_statuses"],
            "reconnect_log_lines": reconnect["log_lines"],
            "elapsed_after_restart_seconds": round(time.time() - restart_at_unix, 3),
        }

        # Independent protocol check: a freshly issued credential (post-restart
        # Runtime) fetches the same descriptor and blob, proving the server side
        # still serves canonical media and the 200 is not native-only.
        protocol: dict = {"at": now_iso()}
        credential = ctx.runtime.issue_credential()
        token = str((credential.get("document") or {}).get("token") or "")
        secrets_list.append(token)
        observation = ctx.runtime.open_observation(token)
        document = observation.get("document") or {}
        context = str(
            ((document.get("grant") or {}).get("stamp") or {}).get("context_id") or ""
        )
        descriptor = ctx.runtime.media_descriptor(token, context, digest)
        descriptor_document = descriptor.get("document") or {}
        handle = str(descriptor_document.get("access_handle") or "")
        blob = ctx.runtime.media_blob(token, handle) if handle else {}
        raw = bytes(blob.get("raw") or b"")
        protocol.update(
            {
                "credential_http_status": credential.get("status"),
                "observation_http_status": observation.get("status"),
                "descriptor_http_status": descriptor.get("status"),
                "descriptor": safe_descriptor(descriptor_document),
                "blob_http_status": blob.get("status"),
                "blob_bytes": len(raw),
                "blob_sha256": sha256_bytes(raw) if raw else None,
                "digest_matches_descriptor": (
                    sha256_bytes(raw) == descriptor_document.get("artifact_digest")
                    if raw
                    else False
                ),
            }
        )
        record["protocol_after_restart"] = protocol

        after = record["after_restart"]
        passed = (
            reconnect["seen"]
            and after["media_refetch_after_restart"]["found"]
            and after["viewed_candidate_coherent"]
            and after["final_state"] == "DISPLAYED"
            and after["displayed_marker"]
            and not after["forbidden_text_present"]
            and after["descriptor_200_after_restart"]
            and protocol.get("descriptor_http_status") == 200
            and protocol.get("blob_http_status") == 200
            and protocol.get("digest_matches_descriptor")
        )
        record["verdict"] = "PASS" if passed else "FAIL"
        if after["forbidden_text_present"]:
            record["residuals"].append(
                "the same-process artifact view still shows the Forbidden sentence"
            )
        if after["descriptor_401_after_restart"]:
            record["residuals"].append(
                "the media descriptor still answered 401 after the Runtime restart"
            )
        if not reconnect["seen"]:
            record["residuals"].append(
                "the Desktop reconnect credential/observation sequence was not observed"
            )
        return record
    finally:
        if ctx.app is not None:
            with contextlib.suppress(Exception):
                record["desktop_log_tail"] = acc1.bounded(
                    acc1.redact_text(ctx.app.log_text()), 6000
                )
        ctx.cleanup()
        record["cleanup"] = ctx.record.get("cleanup", {})
        record["finished_at"] = now_iso()
        record["_secrets"] = [value for value in secrets_list if value]
        record["runtime_log_path"] = str(ctx.runtime.log_path)
        record["desktop_log_path"] = (
            str(ctx.app.log_path) if ctx.app is not None else None
        )


# --------------------------------------------------------------------------- evidence


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--desktop-exe",
        required=True,
        help="repaired cos-desktop binary to measure (not the pre-fix release binary)",
    )
    parser.add_argument("--label", default="session-identity")
    parser.add_argument("--keep-work", action="store_true")
    args = parser.parse_args(argv)
    desktop_exe = Path(args.desktop_exe).resolve()
    if not desktop_exe.is_file():
        print(f"desktop binary not found: {desktop_exe}", file=sys.stderr)
        return 2

    if EVIDENCE.exists():
        shutil.rmtree(EVIDENCE)
    EVIDENCE.mkdir(parents=True, exist_ok=True)
    (EVIDENCE / "uia").mkdir(exist_ok=True)
    (EVIDENCE / "shots").mkdir(exist_ok=True)
    (EVIDENCE / "logs").mkdir(exist_ok=True)
    WORK_ROOT.mkdir(parents=True, exist_ok=True)

    session: dict = {
        "schema_version": 1,
        "task": "RT3 media session-identity repair (same-process Runtime restart)",
        "started_at": now_iso(),
        "desktop_root": str(ROOT),
        "runtime_root": str(RUNTIME_ROOT),
        "desktop_head": run_git("rev-parse", "HEAD"),
        "desktop_status_porcelain": run_git("status", "--porcelain"),
        "runtime_head": run_git("rev-parse", "HEAD", cwd=RUNTIME_ROOT),
        "runtime_status_porcelain": run_git("status", "--porcelain", cwd=RUNTIME_ROOT),
        "desktop_binary": {
            "path": str(desktop_exe),
            "sha256": sha256_file(desktop_exe),
            "bytes": desktop_exe.stat().st_size,
        },
        "source_db": {
            "path": str(residual.SOURCE_DB),
            "sha256": sha256_file(residual.SOURCE_DB),
            "bytes": residual.SOURCE_DB.stat().st_size,
        },
        "native_source_run": {
            "path": str(SOURCE_RUN),
            "db_sha256": sha256_file(SOURCE_RUN / "generation.sqlite"),
            "db_bytes": (SOURCE_RUN / "generation.sqlite").stat().st_size,
            "task_ref": TASK_REF,
            "note": (
                "fresh copy of the prior isolated closure run's DB/CAS; the task's"
                " capability snapshot set is test-only material, no generation was"
                " executed by this session"
            ),
        },
        "ports": {"native": PORT},
        "measurement": {},
        "cleanup": {},
        "residuals": [],
    }
    record = scenario(desktop_exe=desktop_exe, label=args.label)
    secrets_list = record.pop("_secrets", [])
    runtime_log = Path(record.pop("runtime_log_path", ""))
    desktop_log = record.get("desktop_log_path")

    log_manifests: list[dict] = []
    if runtime_log.is_file():
        log_manifests.append(
            sanitize_and_copy(
                runtime_log, EVIDENCE / "logs" / "runtime-native.log.sanitized.txt", secrets_list
            )
        )
    if desktop_log:
        desktop_path = Path(desktop_log)
        if desktop_path.is_file():
            log_manifests.append(
                sanitize_and_copy(
                    desktop_path,
                    EVIDENCE / "logs" / "desktop-native.log.sanitized.txt",
                    secrets_list,
                )
            )

    write_json(EVIDENCE / "record-native.json", record)
    session["measurement"] = record
    session["cleanup"] = record.get("cleanup", {})
    session["residuals"] = record.get("residuals", [])
    session["log_manifests"] = log_manifests
    session["finished_at"] = now_iso()
    write_json(EVIDENCE / "session.json", session)

    leak = leak_scan(EVIDENCE, secrets_list)
    write_json(EVIDENCE / "leak-scan.json", leak)

    hashes = {
        path.relative_to(EVIDENCE).as_posix(): sha256_file(path)
        for path in sorted(EVIDENCE.rglob("*"))
        if path.is_file() and path.name != "session.sha256"
    }
    write_json(EVIDENCE / "hashes.json", hashes)
    (EVIDENCE / "session.sha256").write_text(
        "\n".join(f"{digest}  {name}" for name, digest in hashes.items()) + "\n",
        encoding="utf-8",
    )

    print(
        json.dumps(
            {
                "verdict": record.get("verdict"),
                "after_state": (record.get("after_restart") or {}).get("final_state"),
                "descriptor_statuses": (
                    (record.get("after_restart") or {}).get("media_access") or {}
                ).get("descriptor_statuses"),
                "blob_statuses": (
                    (record.get("after_restart") or {}).get("media_access") or {}
                ).get("blob_statuses"),
                "leak_clean": leak.get("clean"),
                "evidence": str(EVIDENCE),
            },
            ensure_ascii=False,
        ),
        flush=True,
    )
    return 0 if record.get("verdict") == "PASS" and leak.get("clean") else 1


if __name__ == "__main__":
    raise SystemExit(main())
