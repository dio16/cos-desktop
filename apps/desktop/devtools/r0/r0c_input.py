"""MP-R0C-S1 Windows synthetic input probe (development-only).

Finds the single approved fixture window, replays the deterministic planned trace
with PostMessage, records actual timestamps and classifications, then posts one
WM_CLOSE. Synthetic input is never described as a human session. The probe never
changes focus, IME state or held-key repeat policy; it only posts the declared
messages. No new dependency and no unsafe Rust.
"""

from __future__ import annotations

import argparse
import ctypes
import json
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

PROBE_VERSION = "r0c-input-1"


class ProbeError(Exception):
    def __init__(self, code: str):
        super().__init__(code)
        self.code = code


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


def find_window(user32, kernel32, title: str, excluded: set[int], wait_ms: int) -> tuple[int, int, str]:
    deadline = time.monotonic() + wait_ms / 1000.0
    while True:
        candidates: list[tuple[int, int, str]] = []
        for pid in _pids():
            if pid in excluded or _image_name(kernel32, pid) != "cos-desktop.exe":
                continue
            for hwnd, text in _windows_for(user32, pid):
                if text == title:
                    candidates.append((pid, hwnd, text))
        if len(candidates) > 1:
            raise ProbeError("WINDOW_AMBIGUOUS")
        if candidates:
            return candidates[0]
        if time.monotonic() >= deadline:
            raise ProbeError("WINDOW_NOT_FOUND")
        time.sleep(0.05)


def _pids() -> list[int]:
    import csv
    import io
    import subprocess

    completed = subprocess.run(
        ["tasklist", "/FI", "IMAGENAME eq cos-desktop.exe", "/FO", "CSV", "/NH"],
        capture_output=True, text=True, encoding="utf-8-sig", errors="replace", timeout=15,
    )
    pids = []
    for row in csv.reader(io.StringIO(completed.stdout)):
        if len(row) >= 2 and row[0].lower() == "cos-desktop.exe":
            try:
                pids.append(int(row[1]))
            except ValueError:
                pass
    return pids


def _post(user32, hwnd: int, message: int, wparam: int, lparam: int) -> bool:
    return bool(user32.PostMessageW(hwnd, message, wparam, lparam))


def _lparam(x: int, y: int) -> int:
    return ((y & 0xFFFF) << 16) | (x & 0xFFFF)


def _send(user32, hwnd: int, action: dict) -> list[tuple[int, int, int]]:
    kind = action["kind"]
    payload = action["payload"]
    if kind == "keyboard":
        key = VK[payload["key"]]
        return [(WM_KEYDOWN, key, 1), (WM_KEYUP, key, 0xC0000001)]
    if kind == "held-key":
        return [(WM_KEYDOWN, VK[payload["key"]], 1)]
    if kind == "pointer":
        x, y = payload["x"], payload["y"]
        messages = [(WM_MOUSEMOVE, 0, _lparam(x, y))]
        if action["sequence"] % 20 == 0:
            messages.append((WM_LBUTTONDOWN, 0x0001, _lparam(x, y)))
            messages.append((WM_LBUTTONUP, 0x0000, _lparam(x, y)))
        return messages
    if kind == "scroll":
        delta = int(payload.get("delta", 120))
        return [(WM_MOUSEWHEEL, (delta << 16) & 0xFFFFFFFF, 0)]
    if kind == "posted-saturation":
        return [(WM_NULL, 0, 0)]
    return []


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
        excluded: set[int], max_lag_ms: int, close_timeout_ms: int, window_wait_ms: int) -> int:
    plan = json.loads(plan_path.read_text(encoding="utf-8"))
    actions = plan["actions"]
    if len(actions) > r0c.MAX_ACTIONS:
        raise ProbeError("PLAN_TOO_LARGE")
    user32, kernel32 = _load_user32()
    pid, hwnd, window_title = find_window(user32, kernel32, title, excluded, window_wait_ms)
    # The planned clock starts after the window is bound, so warmup is measured
    # from a real visible window rather than from process spawn.
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
        if lag > max_lag_ns:
            records.append({
                "sequence": action["sequence"], "kind": action["kind"],
                "planned_ns": action["planned_ns"], "actual_ns": actual,
                "classification": r0c.classify(
                    action["planned_ns"], actual, False, interval_ns=interval_ns,
                    max_lag_ns=max_lag_ns),
                "excluded_reason": "SCHEDULE_LAG_EXCEEDED",
            })
            continue
        messages = _send(user32, hwnd, action)
        if action["kind"] == "held-key":
            held.append(VK[action["payload"]["key"]])
        delivered = all(_post(user32, hwnd, message, wparam, lparam)
                        for message, wparam, lparam in messages)
        records.append({
            "sequence": action["sequence"], "kind": action["kind"],
            "planned_ns": action["planned_ns"], "actual_ns": actual,
            "classification": r0c.classify(
                action["planned_ns"], actual, delivered, interval_ns=interval_ns,
                max_lag_ns=max_lag_ns),
            "excluded_reason": "" if delivered else "POST_MESSAGE_REFUSED",
        })
    close = {"attempts": 0, "posted": False, "window_gone": False}
    for key in dict.fromkeys(held):
        _post(user32, hwnd, WM_KEYUP, key, 0xC0000001)
    if not close["posted"]:
        close["attempts"] += 1
        close["posted"] = _post(user32, hwnd, WM_CLOSE, 0, 0)
    deadline = time.monotonic() + close_timeout_ms / 1000.0
    while time.monotonic() < deadline:
        if not user32.IsWindow(hwnd):
            close["window_gone"] = True
            break
        time.sleep(0.02)
    document = {
        "probe_version": PROBE_VERSION,
        "generation": generation,
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
    parser.add_argument("--excluded-pids", default="")
    parser.add_argument("--max-lag-ms", type=int, default=r0c.MAX_LAG_MS)
    parser.add_argument("--close-timeout-ms", type=int, default=r0c.CLOSE_TIMEOUT_MS)
    parser.add_argument("--window-wait-ms", type=int, default=20000)
    args = parser.parse_args(argv)
    excluded = {int(item) for item in args.excluded_pids.split(",") if item.strip()}
    try:
        return run(Path(args.plan), Path(args.out), args.title, args.generation,
                   excluded, args.max_lag_ms, args.close_timeout_ms, args.window_wait_ms)
    except ProbeError as error:
        print(error.code, file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
