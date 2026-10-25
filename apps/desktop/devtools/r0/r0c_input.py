"""MP-R0C-S1 Windows synthetic input probe (development-only).

Finds the single window owned by the approved launcher's helper child, replays
the deterministic planned trace with PostMessage, records actual timestamps and
classifications, then posts one WM_CLOSE. Synthetic input is never described as
a human session. The probe never changes focus, IME state or held-key repeat
policy; it only posts the declared messages. No new dependency and no unsafe Rust.
"""

from __future__ import annotations

import argparse
import ctypes
import json
import os
import sys
import time
from ctypes import wintypes
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import r0c_contracts as r0c  # noqa: E402

WM_KEYDOWN = 0x0100
WM_KEYUP = 0x0101
WM_MOUSEMOVE = 0x0200
WM_LBUTTONDOWN = 0x0201
WM_LBUTTONUP = 0x0202
WM_MOUSEWHEEL = 0x020A
WM_CLOSE = 0x0010
WM_NULL = 0x0000

VK = {"TAB": 0x09, "ENTER": 0x0D, "SPACE": 0x20, "LEFT": 0x25, "RIGHT": 0x27,
      "UP": 0x26, "DOWN": 0x28, "ESCAPE": 0x1B}

PROBE_VERSION = "r0c-input-2"
TH32CS_SNAPPROCESS = 0x00000002
MAX_PATH = 260


class ProbeError(Exception):
    def __init__(self, code: str):
        super().__init__(code)
        self.code = code


class PROCESSENTRY32W(ctypes.Structure):
    _fields_ = [
        ("dwSize", wintypes.DWORD),
        ("cntUsage", wintypes.DWORD),
        ("th32ProcessID", wintypes.DWORD),
        ("th32DefaultHeapID", ctypes.POINTER(ctypes.c_ulong)),
        ("th32ModuleID", wintypes.DWORD),
        ("cntThreads", wintypes.DWORD),
        ("th32ParentProcessID", wintypes.DWORD),
        ("pcPriClassBase", ctypes.c_long),
        ("dwFlags", wintypes.DWORD),
        ("szExeFile", ctypes.c_wchar * MAX_PATH),
    ]


def _load_user32():
    if sys.platform != "win32":
        raise ProbeError("WINDOWS_ONLY")
    user32 = ctypes.windll.user32
    user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
    user32.PostMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
    user32.IsWindowVisible.argtypes = [wintypes.HWND]
    user32.IsWindow.argtypes = [wintypes.HWND]
    user32.GetWindowTextLengthW.argtypes = [wintypes.HWND]
    user32.GetWindowTextW.argtypes = [wintypes.HWND, ctypes.c_wchar_p, ctypes.c_int]
    user32.EnumWindows.argtypes = [
        ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM), wintypes.LPARAM
    ]
    kernel32 = ctypes.windll.kernel32
    kernel32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel32.QueryFullProcessImageNameW.argtypes = [
        wintypes.HANDLE, wintypes.DWORD, wintypes.LPWSTR, ctypes.POINTER(wintypes.DWORD)
    ]
    kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
    kernel32.CreateToolhelp32Snapshot.argtypes = [wintypes.DWORD, wintypes.DWORD]
    kernel32.CreateToolhelp32Snapshot.restype = wintypes.HANDLE
    kernel32.Process32FirstW.argtypes = [wintypes.HANDLE, ctypes.POINTER(PROCESSENTRY32W)]
    kernel32.Process32NextW.argtypes = [wintypes.HANDLE, ctypes.POINTER(PROCESSENTRY32W)]
    return user32, kernel32


def _image_name(kernel32, pid: int) -> str:
    handle = kernel32.OpenProcess(0x1000, False, pid)
    if not handle:
        return ""
    try:
        size = wintypes.DWORD(32768)
        buffer = ctypes.create_unicode_buffer(size.value)
        if not kernel32.QueryFullProcessImageNameW(handle, 0, buffer, ctypes.byref(size)):
            return ""
        return buffer.value.rsplit("\\", 1)[-1].lower()
    finally:
        kernel32.CloseHandle(handle)


def _parent_map(kernel32) -> dict[int, tuple[int, str]]:
    snapshot = kernel32.CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
    if not snapshot or snapshot == ctypes.c_void_p(-1).value:
        raise ProbeError("PROCESS_SNAPSHOT_FAILED")
    processes: dict[int, tuple[int, str]] = {}
    try:
        entry = PROCESSENTRY32W()
        entry.dwSize = ctypes.sizeof(PROCESSENTRY32W)
        if not kernel32.Process32FirstW(snapshot, ctypes.byref(entry)):
            raise ProbeError("PROCESS_SNAPSHOT_FAILED")
        while True:
            processes[int(entry.th32ProcessID)] = (
                int(entry.th32ParentProcessID), entry.szExeFile.lower()
            )
            if not kernel32.Process32NextW(snapshot, ctypes.byref(entry)):
                break
    finally:
        kernel32.CloseHandle(snapshot)
    return processes


def _bind_child(kernel32, launcher_pid: int) -> tuple[int, int]:
    """Bind the approved launcher to its helper and the helper to the app child."""
    processes = _parent_map(kernel32)
    helpers = [pid for pid, (parent, image) in processes.items()
               if parent == launcher_pid and image == "r0_owner.exe"]
    if not helpers:
        raise ProbeError("LAUNCHER_CHILD_NOT_FOUND")
    if len(helpers) > 1:
        raise ProbeError("LAUNCHER_CHILD_AMBIGUOUS")
    apps = [pid for pid, (parent, image) in processes.items()
            if parent == helpers[0] and image == "cos-desktop.exe"]
    if not apps:
        raise ProbeError("APP_CHILD_NOT_FOUND")
    if len(apps) > 1:
        raise ProbeError("APP_CHILD_AMBIGUOUS")
    return helpers[0], apps[0]


def _windows_for(user32, pid: int) -> list[tuple[int, str]]:
    found: list[tuple[int, str]] = []

    def callback(hwnd, _lparam):
        owner = wintypes.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value == pid and user32.IsWindowVisible(hwnd):
            length = user32.GetWindowTextLengthW(hwnd)
            buffer = ctypes.create_unicode_buffer(length + 1)
            user32.GetWindowTextW(hwnd, buffer, length + 1)
            if buffer.value:
                found.append((int(hwnd), buffer.value))
        return True

    user32.EnumWindows(ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)(callback), 0)
    return found


def _tasklist() -> str:
    root = os.environ.get("SystemRoot")
    if not root:
        raise ProbeError("SYSTEM_ROOT_UNAVAILABLE")
    path = Path(root) / "System32" / "tasklist.exe"
    if not path.is_file():
        raise ProbeError("TASKLIST_UNAVAILABLE")
    return str(path)


def _pids() -> list[int]:
    import csv
    import io
    import subprocess

    completed = subprocess.run(
        [_tasklist(), "/FI", "IMAGENAME eq cos-desktop.exe", "/FO", "CSV", "/NH"],
        capture_output=True, text=True, encoding="utf-8-sig", errors="replace", timeout=15,
    )
    if completed.returncode != 0:
        raise ProbeError("TASKLIST_FAILED")
    pids = []
    for row in csv.reader(io.StringIO(completed.stdout)):
        if len(row) >= 2 and row[0].lower() == "cos-desktop.exe":
            try:
                pids.append(int(row[1]))
            except ValueError:
                pass
    return pids


def find_window(user32, kernel32, launcher_pid: int, title: str,
                excluded: set[int], wait_ms: int) -> tuple[int, int, str, int, int]:
    deadline = time.monotonic() + wait_ms / 1000.0
    last = "WINDOW_NOT_FOUND"
    while True:
        try:
            helper_pid, app_pid = _bind_child(kernel32, launcher_pid)
        except ProbeError as error:
            last = error.code
        else:
            if app_pid in excluded:
                last = "APP_CHILD_EXCLUDED"
            else:
                windows = [(hwnd, text) for hwnd, text in _windows_for(user32, app_pid)
                           if text == title]
                if len(windows) > 1:
                    raise ProbeError("WINDOW_AMBIGUOUS")
                if windows:
                    return app_pid, windows[0][0], windows[0][1], helper_pid, launcher_pid
                last = "WINDOW_NOT_FOUND"
        if time.monotonic() >= deadline:
            raise ProbeError(last)
        time.sleep(0.05)


def _lparam(x: int, y: int) -> int:
    return ((y & 0xFFFF) << 16) | (x & 0xFFFF)


def message_plan(kind: str, payload: dict, sequence: int) -> list[tuple[int, int, int]]:
    """The exact synthetic messages one action posts. Pure and testable."""
    if kind == "keyboard":
        key = VK[payload["key"]]
        return [(WM_KEYDOWN, key, 1), (WM_KEYUP, key, 0xC0000001)]
    if kind == "held-key":
        return [(WM_KEYDOWN, VK[payload["key"]], 1)]
    if kind == "pointer":
        x, y = payload["x"], payload["y"]
        messages = [(WM_MOUSEMOVE, 0, _lparam(x, y))]
        if sequence % 20 == 0:
            messages.append((WM_LBUTTONDOWN, 0x0001, _lparam(x, y)))
            messages.append((WM_LBUTTONUP, 0x0000, _lparam(x, y)))
        return messages
    if kind == "scroll":
        delta = int(payload.get("delta", 120))
        return [(WM_MOUSEWHEEL, (delta << 16) & 0xFFFFFFFF, 0)]
    if kind == "posted-saturation":
        return [(WM_NULL, 0, 0)]
    return []


def action_record(planned_ns: int, actual_ns: int, delivered: bool, suppressed: bool,
                  interval_ns: int, max_lag_ns: int) -> dict:
    """One probe record, with its classification derived in one place."""
    return {
        "planned_ns": planned_ns,
        "actual_ns": actual_ns,
        "delivered": delivered,
        "suppressed": suppressed,
        "classification": r0c.classify(
            planned_ns, actual_ns, delivered, interval_ns=interval_ns,
            suppressed=suppressed, max_lag_ns=max_lag_ns,
        ),
    }


def close_once(close: dict, post) -> bool:
    """Post WM_CLOSE at most once; a second call is a no-op."""
    if close["posted"]:
        return True
    close["attempts"] += 1
    close["posted"] = bool(post())
    return close["posted"]


def _post(user32, hwnd: int, message: int, wparam: int, lparam: int) -> bool:
    return bool(user32.PostMessageW(hwnd, message, wparam, lparam))


def _sleep_until(target_ns: int) -> None:
    while True:
        remaining = target_ns - time.perf_counter_ns()
        if remaining <= 0:
            return
        if remaining > 2_000_000:
            time.sleep(min(remaining / 1_000_000_000, 0.002))
        else:
            time.sleep(0)


def run(plan_path: Path, out_path: Path, title: str, generation: int,
        excluded: set[int], max_lag_ms: int, close_timeout_ms: int, window_wait_ms: int,
        launcher_pid: int) -> int:
    plan = json.loads(plan_path.read_text(encoding="utf-8"))
    actions = plan["actions"]
    if len(actions) > r0c.MAX_ACTIONS:
        raise ProbeError("PLAN_TOO_LARGE")
    user32, kernel32 = _load_user32()
    pid, hwnd, window_title, helper_pid, bound_launcher = find_window(
        user32, kernel32, launcher_pid, title, excluded, window_wait_ms
    )
    owner = wintypes.DWORD()
    user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
    if int(owner.value) != pid:
        raise ProbeError("WINDOW_PID_MISMATCH")
    max_lag_ns = max_lag_ms * 1_000_000
    start = time.perf_counter_ns()
    records: list[dict] = []
    held: list[int] = []
    cancelled = False
    for action in actions:
        interval_ns = max(1, int(round(1_000_000_000 / action["requested_hz"])))
        _sleep_until(start + action["planned_ns"])
        actual = time.perf_counter_ns() - start
        lag = actual - action["planned_ns"]
        record = {"sequence": action["sequence"], "kind": action["kind"]}
        if lag > max_lag_ns:
            record.update(action_record(
                action["planned_ns"], actual, False, True, interval_ns, max_lag_ns
            ))
            record["excluded_reason"] = "SCHEDULE_LAG_EXCEEDED"
            records.append(record)
            continue
        messages = message_plan(action["kind"], action["payload"], action["sequence"])
        if action["kind"] == "held-key":
            held.append(VK[action["payload"]["key"]])
        delivered = all(_post(user32, hwnd, message, wparam, lparam)
                        for message, wparam, lparam in messages)
        record.update(action_record(
            action["planned_ns"], actual, delivered, False, interval_ns, max_lag_ns
        ))
        record["excluded_reason"] = "" if delivered else "POST_MESSAGE_REFUSED"
        records.append(record)
    close = {"attempts": 0, "posted": False, "window_gone": False}
    for key in dict.fromkeys(held):
        _post(user32, hwnd, WM_KEYUP, key, 0xC0000001)
    close_once(close, lambda: _post(user32, hwnd, WM_CLOSE, 0, 0))
    deadline = time.monotonic() + close_timeout_ms / 1000.0
    while time.monotonic() < deadline:
        if not user32.IsWindow(hwnd):
            close["window_gone"] = True
            break
        time.sleep(0.02)
    document = {
        "probe_version": PROBE_VERSION,
        "generation": generation,
        "binding": {
            "bound": True,
            "launcher_pid": bound_launcher,
            "helper_pid": helper_pid,
            "window_pid": pid,
        },
        "window": {"pid": pid, "title": window_title},
        "synthetic": True,
        "actions": records,
        "close": close,
        "cancelled": cancelled,
    }
    text = json.dumps(document, sort_keys=True, indent=2) + "\n"
    if len(text.encode("utf-8")) > r0c.MAX_TRACE_BYTES:
        raise ProbeError("OUTPUT_TOO_LARGE")
    with out_path.open("x", encoding="utf-8", newline="\n") as handle:
        handle.write(text)
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="MP-R0C-S1 synthetic input probe")
    parser.add_argument("--plan", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--title", required=True)
    parser.add_argument("--generation", type=int, required=True)
    parser.add_argument("--launcher-pid", type=int, required=True)
    parser.add_argument("--excluded-pids", default="")
    parser.add_argument("--max-lag-ms", type=int, default=r0c.MAX_LAG_MS)
    parser.add_argument("--close-timeout-ms", type=int, default=r0c.CLOSE_TIMEOUT_MS)
    parser.add_argument("--window-wait-ms", type=int, default=20000)
    args = parser.parse_args(argv)
    excluded = {int(item) for item in args.excluded_pids.split(",") if item.strip()}
    try:
        return run(Path(args.plan), Path(args.out), args.title, args.generation,
                   excluded, args.max_lag_ms, args.close_timeout_ms, args.window_wait_ms,
                   args.launcher_pid)
    except ProbeError as error:
        print(error.code, file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
