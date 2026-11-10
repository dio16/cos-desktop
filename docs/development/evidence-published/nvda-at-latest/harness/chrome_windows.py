"""Minimize/restore visible Chrome top-level windows for the NVDA session.

Test-only helper. It records only window handles/classes, never titles, so no
browsing content reaches the evidence. Chrome is restored after the run.
"""
from __future__ import annotations

import ctypes
import json
import sys
from ctypes import wintypes
from pathlib import Path

user32 = ctypes.WinDLL("user32", use_last_error=True)
SW_MINIMIZE = 6
SW_RESTORE = 9
SW_SHOW = 5
GW_OWNER = 4

EnumWindowsProc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)


def window_class(hwnd: int) -> str:
    buf = ctypes.create_unicode_buffer(256)
    user32.GetClassNameW(hwnd, buf, 256)
    return buf.value


def is_visible(hwnd: int) -> bool:
    return bool(user32.IsWindowVisible(hwnd))


def is_iconic(hwnd: int) -> bool:
    return bool(user32.IsIconic(hwnd))


def main(argv: list[str]) -> int:
    action = argv[1] if len(argv) > 1 else "minimize"
    state_path = argv[2] if len(argv) > 2 else ""
    handles: list[int] = []

    def callback(hwnd, _lparam):
        if not is_visible(hwnd):
            return True
        if user32.GetWindow(hwnd, GW_OWNER):
            return True
        if window_class(hwnd) != "Chrome_WidgetWin_1":
            return True
        handles.append(int(hwnd))
        return True

    user32.EnumWindows(EnumWindowsProc(callback), 0)

    if action == "minimize":
        state = []
        for hwnd in handles:
            was_iconic = is_iconic(hwnd)
            shown = bool(user32.ShowWindow(hwnd, SW_MINIMIZE))
            state.append({"hwnd": hwnd, "was_iconic": was_iconic, "minimized": shown})
        document = {"action": "minimize", "windows": state}
        if state_path:
            Path(state_path).write_text(json.dumps(document, ensure_ascii=False), encoding="utf-8")
    elif action == "restore":
        state = json.loads(open(state_path, encoding="utf-8").read()) if state_path else {"windows": []}
        restored = []
        for item in state.get("windows", []):
            if item.get("was_iconic"):
                continue
            hwnd = int(item["hwnd"])
            shown = bool(user32.ShowWindow(hwnd, SW_RESTORE))
            restored.append({"hwnd": hwnd, "restored": shown})
        document = {"action": "restore", "windows": restored}
    else:
        document = {"action": action, "error": "unknown_action", "windows": handles}
    print(json.dumps(document, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
