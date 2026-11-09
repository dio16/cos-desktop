# UIA focus-event stream from a same-integrity client — AX F1 evidence gap (2026-11-10)

Bounded follow-up to [the native AX / IME / DPI audit](../native-ax-ime-dpi-latest/README.md):
that audit could not observe the focused control because `accevent.exe` requires elevation
(`os error 740`) and its own probe's `AutomationElement.FocusedElement` reads came back empty
once the window was foregrounded (finding F1). This directory answers the same question from an
ordinary same-integrity UIA **client** — no SDK tool, no elevation, no screen reader — and
records every focus-changed event the product raises while real `SendInput` Tab / Shift+Tab /
Ctrl+N / Ctrl+K sequences run against the release window.

- **Verdict:** `PASS` — the provider **does** raise UIA focus-changed events
  (`UIA_AutomationFocusChangedEventId`, `20005`), names every focused control, and the measured
  Tab order matches the visible order. **F1 is refuted**; the earlier negative is explained by
  the measuring path, with one narrower residual recorded below.
- **Run:** 2026-11-10T05:11:43 → 05:13:49 (+09:00), one bounded session (listener
  05:12:15 → 05:13:49). The instrument is validated first by a WinForms focus target in the same
  session, then the Desktop runs in the MP-ACC1 native lane.
- **Desktop:** `C:/dev/cos-desktop`, HEAD `3e381ca` (`fix(compare): acquire each Compare side
  through the media boundary`; the diff touches `compare_media.rs`, `compare_view.rs` and
  `lib.rs` media code, **not** focus/keyboard/accessibility code). Release binary
  `target/release/cos-desktop.exe`, **21,156,864 B**, sha256
  `sha256:d2009ebe27ec4c8d7f2736f8cc596f03c29f516c569ba8e3ce6ab3a6e242b4eb`, mtime
  05:04:02 (+09:00) — built after `3e381ca`; no Rust source was modified in the worktree at
  measurement time.
- **Runtime:** MP-ACC1 native lane (`run_acc1_native.Ctx`): fresh read-only DB copy of
  `generation.sqlite`, copied presets, own config/logs/CAS, port **8166**, per-run
  `ORCHESTRATOR_AUTH_SECRET` and `ORCHESTRATOR_COS_MEDIA_HANDLE_SECRET`, redirected RT8
  discovery descriptor. Desktop launched with **no** endpoint/token (discovery only). No
  generation was requested and no ComfyUI was started.
- **Instrument:** [`uia_focus_listener.cs`](../../../../apps/desktop/devtools/runtime_integration/uia_focus_listener.cs)
  compiled with the .NET Framework `csc`
  (`C:/Windows/Microsoft.NET/Framework64/v4.0.30319/csc.exe`, FileVersion
  `14.8.10001.0 built by: bd501acdc000004`) against the GAC assemblies
  `UIAutomationClient`, `UIAutomationTypes` and `WindowsBase` (hashes in
  [`listener/build.json`](listener/build.json)). It calls
  `Automation.AddAutomationFocusChangedEventHandler` and, independently,
  `AutomationElement.FocusedElement` every 100 ms. The listener ran as the logged-on user in
  session 1 with `IsUserAnAdmin() == False`: **UIA client event subscription needs no elevation
  for a same-integrity target**.
- **Harness:** [`run_ax_focus_events.py`](../../../../apps/desktop/devtools/runtime_integration/run_ax_focus_events.py)
  driving [`probe_window.ps1`](../../../../apps/desktop/devtools/runtime_integration/probe_window.ps1)
  (two probe additions for this audit: the `raise` action and the `fg2` sequence token —
  activation **without** `SetFocus`, i.e. the path a person's click produces; sequence steps now
  also carry `start_utc`/`end_utc` and `focus_after_rid`).
- **Not measured:** a screen-reader pronouncement (NVDA is still absent) and anything the
  product's UIA tree cannot express. This record is a UIA client event stream, never an AT
  utterance.

## Instrument validation (same session, before the Desktop)

`uia_focus_control.cs` is a WinForms window that moves focus between three named controls on a
timer. The listener subscribed while it ran and recorded **10 focus events / 12 polls** for
`control-a`, `control-b` and `control-c` (`control-focus-events.jsonl`,
`control-focus-events-summary.json`, `session.json` `control`). This proves the subscription and
the polling observe real focus transitions on this host, so the Desktop result cannot be
attributed to the measurement path.

## Activation requirement — why the earlier pass saw nothing

GPUI updates its accessibility host-focus state from `WM_ACTIVATE`
(`gpui_windows/src/events.rs`, `handle_activate_msg` → `adapter.update_window_focus_state`), and
the accesskit adapter is created lazily on the first UIA request. The earlier audit's `foreground`
probe calls `SetFocus(hWnd)` on the top-level window and never produced a deactivate/reactivate
cycle, so the provider stayed "unfocused": it raised no focus events and `FocusedElement` fell
back to the top-level window. That is exactly F1's observation.

This harness therefore performs a real activation cycle before measuring: it parks keyboard focus
on the WinForms helper window (`foreground` on its pid), then raises the Desktop with the `raise`
action (no `SetFocus`). The run records the transition:

| # | Record | Result |
|---|---|---|
| 1 | before the cycle (polls only) | `AutomationElement.FocusedElement` = `COS Desktop` (`ControlType.Window`), `HasKeyboardFocus=false`; **no events** |
| 2 | after park → raise (`focus_cycle`) | 2 `20005` events for `COS Desktop 作業画面` (`ControlType.Pane`, `HasKeyboardFocus=true`) |

The same pattern is reproduced by the separate F1-path pass in
[`runs/setfocus-path/`](runs/setfocus-path/): 112.4 s with the probe's `SetFocus` foregrounding,
**0 focus events**, `FocusedElement` = top-level window throughout, while the app's UIA tree and
keyboard shortcuts still worked. That pass is the negative control for this one.

## Measured focus sequence (real `SendInput`, listener events)

Records: `desktop-focus-events.jsonl` (60 lines: 29 events + 30 polls + 1 start; **27 events and
26 polls belong to the Desktop process**; every event id is `20005`), `session.json`
`analysis.steps` (per-step attribution by UTC window), `uia/*.json` (probe dumps), `shots/*.png`.

**Tab order from `COS Desktop 作業画面` (12 Tab presses, one process, no UIA `SetFocus`):**

| # | Focused control (event name) | Control type |
|---|---|---|
| 1 | `タスクを停止（現在は操作できません）` | Button |
| 2 | `停止要求の受付状況を確認（現在は操作できません）` | Button |
| 3 | `タスクを検索` | Edit |
| 4 | `task-00eeff… — 北欧の女子高生。…` (rail row) | ListItem |
| 5 | `画像表示。0: 全体、1: 物理等倍、+/-: 拡大縮小、矢印: 移動、F11: 全画面、Escape: 復帰` | Group |
| 6 | `診断情報を開く` | Button |
| 7 | `成果物  Ctrl+1` | Button |
| 8 | `候補を比較  Ctrl+2` | Button |
| 9 | `処理の流れ  Ctrl+3` | Button |
| 10 | `目的 — 利用可能: task-…` | Button |
| 11 | `評価 — 利用不可: 候補は選択されていません。` | Button |
| 12 | `判断の根拠 — 利用不可: 候補は選択されていません。` | Button |

This **matches the visible/tree order** recorded in the earlier audit (its row 8 listed the first
eight stops in exactly this order) and extends it with stops 9–12. Every event carries the
runtime id, class, automation id, bounding rectangle and `HasKeyboardFocus=true`.

**Shift+Tab (4 presses):** `評価` → `目的` → `処理の流れ  Ctrl+3` → `候補を比較  Ctrl+2` — the
exact reverse of the Tab sequence, measured as `20005` events.

**Brief (`Ctrl+N`):** `COS Desktop` (Window) → `目的` (Edit) → Tab → `方向性` (Edit) → Tab →
`ブリーフを提出（無効）` (Button) → `Escape` → `COS Desktop 作業画面` (Pane). Focus restoration
after dismissal is therefore measured, not inferred.

**Command palette (`Ctrl+K`):** `コマンド検索` (Edit) takes focus; Tab and Shift+Tab stay in the
search field (the list uses the active-descendant pattern); `Escape` restores
`COS Desktop 作業画面` (Pane).

The probe's own `AutomationElement.FocusedElement` read (`focus_after` in `uia/*-sequence.json`)
agrees with the listener for these steps, which is the second independent signal F1 asked for.

## Findings

- **F1 — refuted (was `ACTIONABLE NON-BLOCKING`, DC-MP-AX1 focus quality).** The provider raises
  UIA focus-changed events for every focused control, and both the event stream and
  `FocusedElement` identify the control by name, role, class, runtime id and focus state. A UIA
  client (the interface a screen reader uses) **does** learn the focused control and the Tab /
  Shift+Tab order; dialog focus restoration is observable. The earlier "no focused element /
  top-level window in every state" was a property of the measuring path (forced `SetFocus` on the
  top-level HWND with no post-adapter activation), not of the provider.
- **Residual (measured, narrow).** When the window was already active and no
  deactivate/reactivate cycle followed the accessibility adapter's creation, the provider
  reported the top-level window and raised **no** focus events for the whole session
  (`runs/setfocus-path/`: 112.4 s, 0 events) even though its tree and shortcuts worked; the
  activation cycle in the main pass resolves it. Source reading explains the mechanism
  (`gpui_windows` handles `WM_ACTIVATE` only, and the adapter is created lazily), but whether a
  real AT client attaching to an already-focused app hits the same condition was **not**
  measured — NVDA is still absent. This is recorded as a follow-up candidate, not a proven
  product defect: the test-only scope of this task does not authorize product changes.
- **Observation (no rank).** On a freshly launched window the app has no focused node; Tab
  traversal produced no focus change until the window had been activated with a real
  `WM_ACTIVATE` (the activation cycle in this harness). This matches the earlier posted-Tab
  negative and is worth a separate AX2 check.

## Artifacts

| Path | Content |
|---|---|
| `session.json` | Full measurement record: environment, build hashes, control pass, focus cycle, every sequence attempt with attribution, per-step listener events, cleanup, leak scan |
| `desktop-focus-events.jsonl` | Raw listener log, one JSON object per event/poll/start (60 lines) |
| `desktop-focus-events-summary.json`, `-status.json` | Listener summary (counts, stop reason, target elements) and early readiness status |
| `control-focus-events.jsonl`, `-summary.json`, `-status.json` | WinForms instrument-validation pass |
| `uia/*.json` | Probe dumps: tree before/after each phase, sequence step logs (with per-step UTC boundaries), foreground/raise results |
| `shots/*.png` | `PrintWindow` captures of the main work surface after each phase (after the prologue, after the Tab order, after the Brief pass, after the palette pass, after dismissal). The Brief and palette dialog screenshots are the earlier audit's `native-ax-ime-dpi-latest/shots/003-shot-brief.png` and `006-shot-palette.png`; these captures document the surface the focus returned to |
| `listener/uia_focus_listener.cs`, `listener/uia_focus_control.cs` | Exact sources used (copied byte-for-byte) |
| `listener/build.json` | `csc` path/version/hash, UIA assembly hashes, compiled exe hashes, exact build commands |
| `runs/setfocus-path/` | The F1-reproducing negative control: same listener, probe `foreground` (`SetFocus`) path, 0 events over 112.4 s |

## Reproduction

```text
cargo build --locked --release -p cos-desktop
PY=<python>  # any Python 3; the host `python.exe` is a Windows Store stub
"$PY" apps/desktop/devtools/runtime_integration/run_ax_focus_events.py
```

The harness compiles the listener and the control target into
`target/ax-focus-build/<stamp>/` (gitignored), copies the sources and the build manifest into
`listener/`, then creates its own isolated Runtime copy under
`target/acc1-native/<stamp>-ax-focus`, launches the release window, runs the control pass, the
activation cycle, the four input sequences, writes the record, closes the window (`WM_CLOSE`,
exit 0) and stops the Runtime through its stop file (clean exit 0).

## Leak scan and cleanup

- Leak scan (`run_acc1_native.leak_scan`) over this directory: **36 files checked, 2 per-run
  secrets checked, 0 findings** at record time (`session.json` `leak_check.clean = true`), and a
  final scan over the assembled directory including `runs/setfocus-path/` checked **43 files,
  0 findings** (`leak-scan-final.json`). No token, bearer header or media handle appears in the
  records; the Runtime endpoint and epoch are recorded, the credential values are not.
- Cleanup: `WM_CLOSE` sent, Desktop exited 0 (`forced: null`), Runtime clean stop (exit 0),
  port **8166 closed** afterwards. Port 8188 (ComfyUI) was open at cleanup time; the harness
  never starts or stops it and requested no generation, so that port is not part of this
  measurement.
- **Environment caveat:** earlier attempts of this same harness observed concurrent
  `cos-desktop.exe` instances (another build from a sibling task) taking keyboard focus mid-run;
  those attempts were rejected and are not part of this record. The final run had no other
  instance during the measurement — every sequence was accepted on its first attempt with
  `foreign_app_events = 0` — and a sibling build appeared only after the last input
  (`session.json` `steps.other_instances_after`), so it cannot have influenced the result.
