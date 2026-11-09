#!/usr/bin/env python3
"""Test-only native accessibility / IME / DPI audit harness.

This driver reuses the MP-ACC1 native lane (``run_acc1_native.py``): the same
isolated Runtime (fresh read-only DB copy, per-run secrets, redirected discovery
descriptor, port 8155) and the same ``probe_window.ps1`` UIA probe. It launches
the release Desktop with no Runtime endpoint/token (discovery only), starts no
generation and touches no ComfyUI. Every surface dump, keyboard step, synthetic
IME attempt and DPI fact is written to the evidence directory verbatim.

It is a development/integration helper, not a product entry point. It never
claims a screen-reader pronouncement: NVDA and the Windows SDK accessibility
tools are not installed on this host and that measurement stays NOT_MEASURED.
"""

from __future__ import annotations

import argparse
import json
import platform
import subprocess
import sys
import time
from pathlib import Path

HARNESS = Path(__file__).resolve().parent
sys.path.insert(0, str(HARNESS))

import run_acc1_native as acc1  # noqa: E402 - local test helper, stdlib only
import run_integration as base  # noqa: E402 - local test helper, stdlib only

EVIDENCE_DEFAULT = (
    acc1.ROOT / "docs" / "development" / "evidence-published" / "native-ax-ime-dpi-latest"
)
PORT = 8155


def now() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%S%z")


class Audit:
    """One bounded audit session over the native window."""

    def __init__(self, evidence: Path) -> None:
        self.evidence = evidence
        self.uia_dir = evidence / "uia"
        self.shot_dir = evidence / "shots"
        self.uia_dir.mkdir(parents=True, exist_ok=True)
        self.shot_dir.mkdir(parents=True, exist_ok=True)
        self.calls = 0
        self.steps: dict[str, object] = {}
        self.cleanup: dict[str, object] = {}

    # -- probe wrapper ----------------------------------------------------------
    def probe(self, label: str, action: str, *, match: str = "", text: str = "",
              shot: str = "") -> dict:
        self.calls += 1
        out = self.uia_dir / f"{self.calls:03d}-{label}-{action}.json"
        command = [
            "powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass",
            "-File", str(acc1.PROBE), "-ProcessId", str(self.pid), "-Action", action,
            "-Out", str(out),
        ]
        image = self.shot_dir / f"{self.calls:03d}-{label}.png" if shot else Path()
        if shot:
            command += ["-ImageOut", str(image)]
        if match:
            command += ["-NameMatch", match]
        if text:
            command += ["-Text", text]
        completed = subprocess.run(
            command, capture_output=True, text=True, timeout=180,
            encoding="utf-8", errors="replace",
        )
        if not out.is_file():
            return {"action": action, "found": False,
                    "error": f"probe produced no output (exit {completed.returncode})",
                    "stderr": completed.stderr[-400:]}
        result = json.loads(out.read_text(encoding="utf-8"))
        result["_out"] = str(out.relative_to(self.evidence))
        if shot and image.is_file():
            result["_image"] = str(image.relative_to(self.evidence))
            result["_image_sha256"] = acc1.sha256_file(image)
            result["_image_bytes"] = image.stat().st_size
        return result

    def tree(self, label: str, *, shot: str = "") -> dict:
        return self.probe(label, "tree", shot=shot)

    @staticmethod
    def focused(tree: dict) -> dict:
        for row in tree.get("rows") or []:
            if row.get("focused"):
                return {
                    "name": row.get("name"),
                    "control_type": row.get("control_type"),
                    "id": row.get("id"),
                    "value": row.get("value"),
                }
        return {}

    def focused_now(self, label: str) -> dict:
        tree = self.probe(label, "tree")
        return {"focused": self.focused(tree), "row_count": tree.get("row_count"),
            "error": tree.get("error")}

    def key(self, label: str, keys: str) -> dict:
        result = self.probe(label, "key", text=keys)
        time.sleep(0.25)
        result["focused_after"] = self.focused_now(f"{label}-after")
        return result

    # -- pass 2: real keyboard input -------------------------------------------
    def pass2(self, ctx) -> dict:
        """Foreground the window and measure the steps posted keys cannot carry.

        Posted WM_KEYDOWN cannot update the async key state, so a modifier+key
        pair never reaches GPUI as a chord. ``SendInput`` does update it, which is
        the only faithful way to measure Shift+Tab, Ctrl+K and the IME context.
        The window is raised for this pass and that is recorded.
        """
        record: dict = {"steps": {}}
        record["surfaces_main"] = self.snapshot("p2-main", shot="p2-main")
        record["foreground"] = self.probe("p2", "foreground")
        record["dpi"] = self.probe("p2", "dpi")

        # Brief: real Escape dismissal and a real Shift+Tab/Tab pair.
        record["brief_open"] = self.probe("p2-brief", "invoke", match="新しいタスクを作成")
        time.sleep(0.8)
        record["surfaces_brief"] = self.snapshot("p2-brief", shot="p2-brief")
        record["focus_goal"] = self.probe("p2-brief", "focus", match="目的")
        record["value_before"] = self.probe("p2-brief", "valueread", match="目的")
        record["synthetic_composition"] = self.probe("p2-brief", "ime", text="あい")
        record["value_after_composition"] = self.probe("p2-brief", "valueread", match="目的")
        record["focus_direction"] = self.probe("p2-brief", "focus", match="方向性")
        record["shift_tab"] = self.probe("p2-brief", "sendinput", text="shift+tab")
        record["after_shift_tab"] = self.focused_now("p2-shifttab")
        record["tab_back"] = self.probe("p2-brief", "sendinput", text="tab")
        record["after_tab"] = self.focused_now("p2-tabback")
        record["escape"] = self.probe("p2-brief", "sendinput", text="escape")
        record["after_escape"] = self.snapshot("p2-after-escape", shot="p2-after-escape")
        record["escape_focus"] = self.focused_now("p2-after-escape")

        # Enter/Space activation of the focused History button.
        record["focus_history_button"] = self.probe(
            "p2-enter", "focus", match="このタスクの実行履歴を開く")
        record["enter"] = self.probe("p2-enter", "sendinput", text="enter")
        record["after_enter"] = self.snapshot("p2-after-enter", shot="p2-after-enter")
        if not any(
            (row.get("name") or "") == "履歴を閉じる"
            for row in record["after_enter"].get("rows") or []
        ):
            record["space"] = self.probe("p2-enter", "sendinput", text="space")
            record["after_space"] = self.snapshot("p2-after-space", shot="p2-after-space")
        record["enter_eval"] = {
            "history_opened_by_enter": any(
                (row.get("name") or "") == "履歴を閉じる"
                for row in record["after_enter"].get("rows") or []
            ),
            "history_opened_by_space": any(
                (row.get("name") or "") == "履歴を閉じる"
                for row in (record.get("after_space") or {}).get("rows") or []
            ),
            "focused_after_enter": self.focused(record["after_enter"]),
        }

        # History: real Escape while its list owns focus.
        record["history_open"] = self.probe(
            "p2-history", "invoke", match="このタスクの実行履歴を開く")
        time.sleep(1.0)
        record["surfaces_history"] = self.snapshot("p2-history", shot="p2-history")
        record["history_focus"] = self.focused_now("p2-history")
        record["history_escape"] = self.probe("p2-history", "sendinput", text="escape")
        record["after_history_escape"] = self.snapshot(
            "p2-after-history-escape", shot="p2-after-history-escape")

        # Palette: real Ctrl+K, traversal, refusal and focus restoration.
        record["focus_before_palette"] = self.focused_now("p2-before-palette")
        record["ctrl_k"] = self.probe("p2-palette", "sendinput", text="ctrl+k")
        time.sleep(0.6)
        record["surfaces_palette"] = self.snapshot("p2-palette", shot="p2-palette")
        record["palette_opened"] = any(
            (row.get("name") or "") == "コマンドパレット"
            for row in record["surfaces_palette"].get("rows") or []
        )
        record["palette_focus"] = self.focused_now("p2-palette")
        for index in range(1, 4):
            record[f"palette_tab_{index}"] = self.probe(
                "p2-palette", "sendinput", text="tab")
            record[f"palette_focus_{index}"] = self.focused_now(f"p2-palette-tab{index}")
        record["palette_escape"] = self.probe("p2-palette", "sendinput", text="escape")
        record["after_palette_escape"] = self.snapshot(
            "p2-after-palette", shot="p2-after-palette")
        record["after_palette_escape_focus"] = self.focused_now("p2-after-palette")
        return record

    # -- pass 3: one-process foreground sequences ------------------------------
    def pass3(self, ctx) -> dict:
        """Measure focus transitions inside one foregrounded probe process.

        A separate probe process per key drops the window activation and hides
        the real focus transition, so each measured sequence (focus -> keys ->
        names) runs in a single process.
        """
        record: dict = {}
        record["surfaces_main"] = self.snapshot("p3-main", shot="p3-main")
        record["dpi"] = self.probe("p3", "dpi")

        # Real Tab / Shift+Tab order from the rail search field.
        record["baseline_no_foreground"] = self.probe(
            "p3-baseline", "sequence", text="focus:タスクを検索|wait:200|names")
        record["tab_shift_tab"] = self.probe(
            "p3-tab",
            "sequence",
            text=(
                "fg|focus:タスクを検索|wait:200|tab|tab|tab|shift+tab|shift+tab|"
                "shift+tab|names"
            ),
        )

        # Enter activation of a normal focused button, then Enter on the close
        # control of the surface it opens.
        record["enter_history"] = self.probe(
            "p3-enter",
            "sequence",
            text=(
                "fg|focus:このタスクの実行履歴を開く|wait:200|enter|wait:800|names|"
                "focus:履歴を閉じる|wait:200|enter|wait:800|names"
            ),
        )
        record["after_enter_history"] = self.snapshot("p3-after-enter", shot="p3-after-enter")

        # Palette: real Ctrl+K, Tab order inside the dialog, Escape and the
        # focus restoration to the element that owned focus before it opened.
        record["palette"] = self.probe(
            "p3-palette",
            "sequence",
            text=(
                "fg|focus:タスクを検索|wait:300|ctrl+k|wait:700|names|tab|tab|shift+tab|"
                "names|escape|wait:400|names"
            ),
        )
        record["after_palette"] = self.snapshot("p3-after-palette", shot="p3-after-palette")

        # Brief: open, focus the Goal field, attempt the synthetic composition
        # with the window foreground and that editable control focused, then read
        # the value back and close with Escape.
        record["brief_open"] = self.probe("p3-brief", "invoke", match="新しいタスクを作成")
        time.sleep(0.8)
        record["brief_ime"] = self.probe(
            "p3-brief",
            "sequence",
            text=(
                "fg|focus:目的|wait:300|names|ime:あい|wait:400|escape|wait:600|names"
            ),
        )
        record["surfaces_brief"] = self.snapshot("p3-brief", shot="p3-brief")
        record["after_brief_escape"] = self.snapshot("p3-after-brief", shot="p3-after-brief")
        return record

    # -- keyboard shortcut pass -------------------------------------------------
    def shortcuts(self) -> dict:
        """Measure the shortcuts that open surfaces with no focusable button."""
        record: dict = {"steps": {}}
        record["steps"]["main"] = self.probe("sc-main", "sequence", text="names")
        record["steps"]["ctrl_h"] = self.probe(
            "sc-history", "sequence", text="fg|ctrl+h|wait:900|names")
        record["steps"]["escape"] = self.probe(
            "sc-history", "sequence", text="fg|escape|wait:500|names")
        record["steps"]["ctrl_n"] = self.probe(
            "sc-brief", "sequence", text="fg|ctrl+n|wait:900|names")
        record["steps"]["escape_brief"] = self.probe(
            "sc-brief", "sequence", text="fg|escape|wait:500|names")
        return record

    # -- screenshot pass --------------------------------------------------------
    def shots(self) -> dict:
        """Capture one PrintWindow screenshot per audited surface."""
        record: dict = {}
        record["main"] = self.probe("shot-main", "shot", shot="main")
        record["brief_open"] = self.probe("shot-brief", "invoke", match="新しいタスクを作成")
        time.sleep(0.8)
        record["brief"] = self.probe("shot-brief", "shot", shot="brief")
        record["brief_close"] = self.probe("shot-brief", "sequence", text="fg|escape")
        time.sleep(0.5)
        record["palette_open"] = self.probe("shot-palette", "sequence", text="fg|ctrl+k")
        time.sleep(0.7)
        record["palette"] = self.probe("shot-palette", "shot", shot="palette")
        record["palette_close"] = self.probe("shot-palette", "sequence", text="fg|escape")
        time.sleep(0.5)
        record["history_open"] = self.probe(
            "shot-history", "invoke", match="このタスクの実行履歴を開く")
        time.sleep(1.2)
        record["history"] = self.probe("shot-history", "shot", shot="history")
        record["history_close"] = self.probe("shot-history", "sequence", text="fg|escape")
        time.sleep(0.5)
        record["main_after"] = self.probe("shot-main-after", "shot", shot="main-after")
        return record

    # -- surfaces ---------------------------------------------------------------
    def snapshot(self, label: str, *, shot: str = "") -> dict:
        tree = self.tree(label, shot=shot)
        rows = tree.get("rows") or []
        tree["role_counts"] = role_counts(rows)
        tree["named_controls"] = [
            {"name": row.get("name"), "control_type": row.get("control_type"),
             "id": row.get("id"), "value": row.get("value"),
             "enabled": row.get("enabled"), "focusable": row.get("focusable"),
             "offscreen": row.get("offscreen"), "patterns": row.get("patterns")}
            for row in rows
            if row.get("name") or row.get("control_type")
        ]
        return tree


def role_counts(rows: list[dict]) -> dict[str, int]:
    counts: dict[str, int] = {}
    for row in rows:
        key = str(row.get("control_type") or "unknown")
        counts[key] = counts.get(key, 0) + 1
    return counts


def write(path: Path, document: object) -> None:
    path.write_text(json.dumps(document, ensure_ascii=False, indent=2), encoding="utf-8")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence-dir", default=str(EVIDENCE_DEFAULT))
    parser.add_argument("--timeout", type=float, default=600.0)
    parser.add_argument("--pass2", action="store_true",
                        help="run only the foregrounded real-input pass")
    parser.add_argument("--pass3", action="store_true",
                        help="run only the one-process foreground sequences")
    parser.add_argument("--shots", action="store_true",
                        help="run only the per-surface screenshot pass")
    parser.add_argument("--shortcuts", action="store_true",
                        help="run only the keyboard-shortcut pass")
    args = parser.parse_args(argv)
    evidence = Path(args.evidence_dir).resolve()
    evidence.mkdir(parents=True, exist_ok=True)

    session: dict = {
        "measurement": "Native accessibility / IME / DPI audit (measured vs NOT_MEASURED)",
        "task": "MP-AX1/MP-AX2 audit follow-up",
        "started_at": now(),
        "harness": {
            "command": "python apps/desktop/devtools/runtime_integration/run_ax_ime_dpi.py",
            "port": PORT, "probe": str(acc1.PROBE),
            "observation": "native window UIA tree + PrintWindow screenshots",
            "not_measured_environment": (
                "NVDA, Inspect.exe and the Windows SDK accessibility tools are not "
                "installed on this host, so screen-reader pronouncement is NOT_MEASURED"
            ),
        },
        "environment": {
            "os": platform.version(),
            "desktop_root": str(acc1.ROOT),
            "head": base.git_value(acc1.ROOT, "rev-parse", "HEAD"),
            "binary": str(acc1.DESKTOP_EXE),
            "binary_sha256": acc1.sha256_file(acc1.DESKTOP_EXE)
            if acc1.DESKTOP_EXE.is_file() else "missing",
        },
        "surfaces": {}, "keyboard": [], "ime": {}, "dpi": {}, "not_measured": [],
        "residuals": [], "leak_check": {},
    }
    audit = Audit(evidence)
    ctx = None
    secrets: list[str] = []
    try:
        ctx = acc1.Ctx("ax-audit", PORT)
        secrets = list(ctx.collect)
        session["runtime"] = {
            "work_dir": str(ctx.work),
            "start": ctx.start_runtime(wait_generation=False),
        }
        window = ctx.launch_desktop("ax")
        session["window"] = window
        audit.pid = ctx.app.ui.pid if ctx.app and ctx.app.ui else 0

        if args.shortcuts:
            session["shortcuts"] = audit.shortcuts()
            session["finished_at"] = now()
        elif args.shots:
            session["shots"] = audit.shots()
            session["finished_at"] = now()
        elif args.pass3:
            session["pass3"] = audit.pass3(ctx)
            session["finished_at"] = now()
        elif args.pass2:
            session["pass2"] = audit.pass2(ctx)
            session["finished_at"] = now()
        else:
            # 1. Main work surface: roles/names/values/states, then the tab order.
            session["surfaces"]["main"] = audit.snapshot("s0-main", shot="s0-main")
            session["dpi"] = audit.probe("s0-main", "dpi")

            for index in range(1, 13):
                session["keyboard"].append({
                    "step": f"tab-{index}", **audit.key(f"s1-tab{index}", "tab"),
                })
            for index in range(1, 5):
                session["keyboard"].append({
                    "step": f"shift-tab-{index}", **audit.key(f"s2-shifttab{index}", "shift+tab"),
                })

            # 2. Brief surface (dialog-like): entry, focus trap, IME and dismissal.
            open_brief = audit.probe("s3-brief", "invoke", match="新しいタスクを作成")
            time.sleep(0.8)
            session["surfaces"]["brief"] = audit.snapshot("s3-brief", shot="s3-brief")
            session["brief_open"] = open_brief
            for index in range(1, 9):
                session["keyboard"].append({
                    "step": f"brief-tab-{index}", **audit.key(f"s3-brieftab{index}", "tab"),
                })
            for index in range(1, 4):
                session["keyboard"].append({
                    "step": f"brief-shift-tab-{index}",
                    **audit.key(f"s3-briefshifttab{index}", "shift+tab"),
                })
            # Real composition is attempted on the Goal field: focus it, send ASCII
            # through the existing WM_CHAR path, then try a synthetic IMM32
            # composition and read the value back.
            focus_goal = audit.probe("s4-ime", "focus", match="目的")
            ascii_type = audit.probe("s4-ime", "type", match="目的", text="ax")
            value_ascii = audit.probe("s4-ime", "valueread", match="目的")
            ime = audit.probe("s4-ime", "ime", text="あい")
            value_japanese = audit.probe("s4-ime", "valueread", match="目的")
            session["ime"] = {
                "focus": focus_goal, "ascii_type": ascii_type, "value_after_ascii": value_ascii,
                "synthetic_composition": ime, "value_after_composition": value_japanese,
            }
            session["surfaces"]["brief_after_input"] = audit.snapshot(
                "s4-brief-after", shot="s4-brief-after")

            # Escape first (no local dismissal is implemented for the Brief), then
            # the explicit close control if one exists.
            session["keyboard"].append({"step": "brief-escape",
                                        **audit.key("s5-escape", "escape")})
            escape_tree = audit.tree("s5-after-escape")
            session["surfaces"]["brief_after_escape"] = {
                "brief_still_open": any(
                    (row.get("name") or "").startswith("目的") for row in escape_tree.get("rows") or []
                ),
                "role_counts": role_counts(escape_tree.get("rows") or []),
                "row_count": escape_tree.get("row_count"),
            }
            close_brief = audit.probe("s5-brief", "invoke", match="ブリーフを閉じる")
            time.sleep(0.5)
            session["brief_close"] = close_brief
            session["surfaces"]["main_after_close"] = audit.snapshot("s5-main-after")

            # 3. Command palette: Ctrl+K, traversal, refusal of a disabled command,
            # Escape and focus restoration.
            palette_open = audit.key("s6-palette", "ctrl+k")
            session["surfaces"]["palette"] = audit.snapshot("s6-palette", shot="s6-palette")
            session["palette_open"] = palette_open
            for index in range(1, 4):
                session["keyboard"].append({
                    "step": f"palette-tab-{index}", **audit.key(f"s6-palettetab{index}", "tab"),
                })
            session["keyboard"].append({
                "step": "palette-escape", **audit.key("s6-paletteescape", "escape"),
            })
            session["surfaces"]["after_palette_escape"] = audit.snapshot(
                "s6-after-escape", shot="s6-after-escape")

            # 4. Inspector / evidence and the task-scoped History surface.
            session["surfaces"]["inspector"] = audit.snapshot("s7-inspector", shot="s7-inspector")
            history_open = audit.probe("s7-history", "invoke", match="このタスクの実行履歴を開く")
            time.sleep(1.2)
            session["surfaces"]["history"] = audit.snapshot("s7-history", shot="s7-history")
            session["history_open"] = history_open
            for index in range(1, 4):
                session["keyboard"].append({
                    "step": f"history-tab-{index}", **audit.key(f"s7-historytab{index}", "tab"),
                })
            escape_history = audit.key("s7-historyescape", "escape")
            session["surfaces"]["after_history_escape"] = audit.snapshot("s7-after-history")
            session["history_escape"] = escape_history

            # 5. Artifact viewer: record exactly what the surface exposes. It is not
            # opened if the projection carries no current candidate/artifact.
            session["surfaces"]["artifact_viewer"] = audit.snapshot(
                "s8-artifact", shot="s8-artifact")

            session["finished_at"] = now()
    except Exception as error:  # noqa: BLE001 - record the failure, never hide it
        import traceback
        session["error"] = f"{type(error).__name__}: {error}"
        session["traceback"] = traceback.format_exc()[-4000:]
    finally:
        if ctx is not None:
            session["cleanup"] = {
                "desktop": ctx.app.close() if ctx.app else None,
                "runtime": ctx.runtime.stop(),
                "ports": {str(PORT): acc1.port_open(PORT), "8188": acc1.port_open(8188)},
            }
            base.cleanup_credential_store(ctx.runtime.endpoint)
        document = acc1.redact_document(session)
        document["leak_check"] = acc1.leak_scan(evidence, list(secrets))
        name = "session.json"
        if args.pass2:
            name = "session-pass2.json"
        elif args.pass3:
            name = "session-pass3.json"
        elif args.shots:
            name = "session-shots.json"
        elif args.shortcuts:
            name = "session-shortcuts.json"
        write(evidence / name, document)
        print(json.dumps({"verdict": "ERROR" if "error" in session else "DONE",
                          "error": session.get("error", ""),
                          "evidence": str(evidence)}, ensure_ascii=False))
    return 0 if "error" not in session else 1


if __name__ == "__main__":
    raise SystemExit(main())
