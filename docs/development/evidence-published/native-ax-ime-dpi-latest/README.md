# Native accessibility / IME / DPI audit — measured vs `NOT_MEASURED` (2026-11-10)

Bounded native audit of the release Desktop against the [DC-MP-AX1](../../../architecture/DESKTOP_DESIGN_CONTRACTS.md#dc-mp-ax1)
scope (screen-reader names/roles, keyboard-only content, IME composition, dialog focus
restoration, DPI) and the [DC-MP-AX2](../../../architecture/DESKTOP_DESIGN_CONTRACTS.md#dc-mp-ax2)
keyboard-completability expectation. Every measurement below names the record that holds
the raw probe output; nothing in this directory is derived from a fixture or a model test.

- **Verdict:** `PARTIAL` — the UIA tree of the main, Brief, inspector/evidence, History,
  artifact-viewer and command-palette surfaces, the reachable keyboard order and the dialog
  dismissal paths are measured on the release window; the synthetic IME composition and a
  second display scale are `NOT_MEASURED` with the exact API/environment reason, and no
  screen-reader pronouncement is claimed.
- **Run:** 2026-11-10T01:47 → 02:24 (+09:00), five bounded sessions (main pass, foregrounded
  pass, one-process sequences, screenshots, shortcut pass).
- **Desktop:** `C:/dev/cos-desktop`, HEAD `c1d97a4f028fed099c2e86f4150a7cc043029603`
  (docs/evidence only over `8bad92d`, the commit the binary was built from; no Rust source
  changed between them). Release binary `target/release/cos-desktop.exe`,
  **21,120,512 B**, sha256
  `sha256:c889fdf0d114b96e6da0b9a4303be1c7cd93c0d29bf5e993dea34b262ca5f205`, built with
  `cargo build --locked --release -p cos-desktop` (exit 0, no relink afterwards).
- **Runtime:** `C:/AI/orchestrator` via the MP-ACC1 native lane
  ([`run_acc1_native.py`](../../../../apps/desktop/devtools/runtime_integration/run_acc1_native.py)
  `Ctx`): fresh read-only DB copy of `generation.sqlite`, copied presets, own config/logs/CAS,
  port **8155**, per-run `ORCHESTRATOR_AUTH_SECRET` and
  `ORCHESTRATOR_COS_MEDIA_HANDLE_SECRET`, redirected RT8 discovery descriptor. The Desktop was
  launched with **no** endpoint/token (discovery only). No generation was requested and no
  ComfyUI was started (`127.0.0.1:8188` closed before and after).
- **Harness:** [`run_ax_ime_dpi.py`](../../../../apps/desktop/devtools/runtime_integration/run_ax_ime_dpi.py)
  driving [`probe_window.ps1`](../../../../apps/desktop/devtools/runtime_integration/probe_window.ps1)
  (four probe actions added for this audit: `tree`, `ime`, `dpi`, `foreground`/`sendinput`,
  `sequence`, plus modifier chords on `key`). One isolated window per session; no product/fixture mode.
- **Environment limitation (measured, not assumed):** NVDA is absent (no
  `C:/Program Files/NVDA/nvda.exe`, no `nvda` process, no NVDA appx). The Windows SDK
  accessibility tools **are** present
  (`C:/Program Files (x86)/Windows Kits/10/bin/10.0.19041.0/x64/{inspect,accevent}.exe`) but
  `accevent.exe` refuses to run without elevation (`os error 740`) and this session cannot
  elevate. Therefore **no UIA event stream and no screen-reader pronouncement exist**; every
  statement below is an out-of-process UIA API answer from the probe, never a spoken result.

## What was measured

Records: `session.json` (main pass), `session-pass2.json` (foregrounded pass),
`session-pass3.json` (one-process sequences), `session-shots.json` (screenshots),
`session-shortcuts.json` (Ctrl+H/Ctrl+N). `uia/*.json` are the probe outputs (171 files),
`shots/*.png` the `PrintWindow` captures (5 files).

| # | Surface / property | Result | Evidence |
|---|---|---|---|
| 1 | Main work surface exposure | 65 nodes: Pane 1, Group 10, Text 24, Button 26, Edit 1, List 1, ListItem 2 | `session.json` `surfaces.main` |
| 2 | Static shell text in the accessibility tree | **present** — 24 `Text` nodes carry the six state-axis cells (e.g. `タスクの状態: 完了（COMPLETED）`, `成果物の利用状態: 取得可能（AVAILABLE）（1件）`), the rail header and inspector lines. The DC-MP-AX1 "Observed (2026-10-18)" note (static text absent) no longer describes this build | `uia/001-s0-main-tree.json` |
| 3 | Brief dialog | 72 nodes, adds `ControlType.Window` `クリエイティブ・ブリーフ`, 3 `Edit` (目的/方向性/検索), 提出 button exposed as `ブリーフを提出（無効）` (disabled reason in name + `IsEnabled=false`) | `uia/036-s3-brief-tree.json`, `shots/003-shot-brief.png` |
| 4 | Inspector / evidence | section headers are focusable `Button`s with `ValuePattern`: `目的 — 利用可能: task-…`, `評価 — 利用不可: 候補は選択されていません。`, `判断の根拠 — 利用不可: …`, `改善 — 利用不可: …`, `結果 — 利用可能: 正式状態: 完了（COMPLETED）`. No evidence group is fabricated | `session.json` `surfaces.inspector` |
| 5 | Artifact viewer | canvas `Group` named `画像表示。0: 全体、1: 物理等倍、+/-: 拡大縮小、矢印: 移動、F11: 全画面、Escape: 復帰`, focusable; zoom `Button`s `全体表示 (0)`, `物理ピクセル1:1 (1)`, `拡大 (+)`, `縮小 (-)`, `全画面 / 復帰 (F11)` are not focusable. No candidate/artifact is projected in this DB state (`候補は表示されていません`), so the painted image is `NOT_MEASURED` | `session.json` `surfaces.artifact_viewer` |
| 6 | History surface | opens from the inspector button (UIA `invoke`), adds `ControlType.Window` `実行履歴`, `履歴を閉じる`, `List` named `実行履歴。1件。上下キーで移動、Enterで開きます。`, one `ListItem` whose name carries run/plan/record identities, `これより古い履歴はありません` | `uia/084-s7-history-tree.json`, `shots/009-shot-history.png` |
| 7 | Command palette | real `Ctrl+K` opens `ControlType.Window` `コマンドパレット` with `Edit` `コマンド検索` and `List` `コマンド` (11 rows). Disabled rows carry `利用不可: <reason>` in the accessible name (`評価`/`判断の根拠`: no candidate; `タスクを停止`: `機能の利用可否情報が投影されていません（capability not projected）`; `停止結果を確認`: no stop request). `Escape` closes it (79 → 65 nodes) | `session-pass3.json` `palette`, `session-shots.json`, `shots/006-shot-palette.png` |
| 8 | Tab order (posted keys, window not foregrounded) | 1 `タスクを停止（現在は操作できません）` → 2 `停止要求の受付状況を確認（現在は操作できません）` → 3 `タスクを検索` → 4 rail `ListItem` → 5 `画像表示…` Group → 6 `診断情報を開く` → 7 `成果物  Ctrl+1` → 8 `候補を比較  Ctrl+2`; steps 9–12 report **no** focused element | `session.json` `keyboard[tab-1..12]` |
| 9 | Shift+Tab order | `NOT_MEASURED` — posted `WM_KEYDOWN` does not update the async key state, so `SendInput` was required and the window foregrounded; in that state the provider reported no focused element (see finding F1) and no UIA event monitor is available (`accevent.exe` needs elevation). The binding itself is source-verified at `crates/desktop-gpui/src/lib.rs:177` | `session-pass2.json`, `session-pass3.json` |
| 10 | Dialog dismissal and focus restoration | `Escape` closes the Brief (72 → 65 nodes; posted keys in the main pass, real `SendInput` in the foregrounded pass) and returns UIA focus to the shell `Pane` `COS Desktop 作業画面`; real `Escape` closes History (71 → 65) and the palette (79 → 65). `Ctrl+H` opens History (65 → 71) and `Ctrl+N` opens the Brief (65 → 72); both dismiss with `Escape` | `session.json` `keyboard[brief-escape]`, `session-pass2.json`, `session-shortcuts.json` |
| 11 | Enter/Space activation | `NOT_MEASURED` for the focused-control path: the inspector button `このタスクの実行履歴を開く` refuses UIA focus (`Target element cannot receive focus.`, `IsKeyboardFocusable=false`), so `Enter` could not be delivered to it and no other control exposes both focus and an `Activate` handler in this state. The binding exists (`lib.rs:178-179`, `Activate` with exclusion contexts) | `session-pass3.json` `enter_history` |
| 12 | DPI facts | `GetAwarenessFromDpiAwarenessContext` = **2** (per-monitor aware); window DPI **96**; nearest monitor effective DPI **96×96**; 2 monitors; physical window rect **1296×839** equals the DIP rect, i.e. one 100 % scale | `session.json` `dpi` |
| 13 | Second / mixed display scale | `NOT_MEASURED` — no scaled monitor exists on this host and changing the system scale needs a sign-out/session change. A synthetic `WM_DPICHANGED` is deliberately **not** posted: its suggested rectangle is an `lParam` pointer that cannot be marshalled into another process, so an invalid pointer would fault the app | `session.json` `dpi` (`NOT_MEASURED` reason) |
| 14 | IME ASCII path | measured — `WM_CHAR` typed `ax` into 目的 and `ValuePattern` read back `ax` | `session.json` `ime.ascii_type` |
| 15 | IME composition (Japanese) | `NOT_MEASURED` — `ImmGetContext` returned **NULL** (`himc = 0`) in both the non-foregrounded and the foregrounded pass, so `ImmSetCompositionStringW` was never reachable and no `WM_IME_COMPOSITION` (`GCS_COMPSTR`/`GCS_RESULTSTR`) could carry text the app reads. The value stayed `何を達成するかを入力…` after the attempt. A real IME session (and an AT observation) is required | `session.json` `ime.synthetic_composition`, `session-pass3.json` `brief_ime` |

## Findings

Rank: `BLOCKING` fails an acceptance line; `ACTIONABLE NON-BLOCKING` needs an owner decision
but no repair in this test-only task; `NIT` is recorded and does not gate.

- **F1 (ACTIONABLE NON-BLOCKING, DC-MP-AX1 focus quality).** Two independent focus signals
  disagree: per-element `HasKeyboardFocus` is reported in tree dumps but goes absent from the
  9th Tab stop onward, and with the window foregrounded `AutomationElement.FocusedElement`
  returns the top-level window in every state (Brief, palette, rail). No UIA event stream can
  be captured here (`accevent.exe` requires elevation). A screen reader following UIA focus
  events therefore has **no measured** way to learn the focused control; either the provider
  does not publish focus for non-button nodes or the probe path is not equivalent to an AT
  client. Repair needs an AT observation (`accevent.exe` with elevation or NVDA), not a
  fixture test. Evidence: `session.json` `keyboard[tab-9..12]`, `session-pass3.json`.
- **F2 (NIT, keyboard reachability of visible affordances).** `新しいタスクを作成` and
  `このタスクの実行履歴を開く` are `ControlType.Button` with `IsKeyboardFocusable=false`, and
  the artifact zoom buttons likewise. Their actions are keyboard-reachable through `Ctrl+N`,
  `Ctrl+H` and the viewer's documented group keys (both shortcuts measured here), so no
  workflow is mouse-only; the accessible *affordance* is not Tab-reachable, which a screen
  reader user may read as a disabled path. Evidence: `session.json` `surfaces.main` rows.
- **F3 (NIT, role consistency).** Equivalent selectable rows expose different roles: rail and
  palette rows are `ListItem` under a `List` (active descendant, the implemented keyboard
  pattern), while dock rows are `Button` under a focusable `Group` `候補一覧`. The rail `List`
  has an empty accessible name (bounds `[1096,570,208,322]`) while the palette list is named
  `コマンド` and the History list is named with its own key instructions. Evidence:
  `session.json` `surfaces.main`, `session-pass2.json` `surfaces_palette` (`uia/031-p2-palette-tree.json`).
- **F4 (NIT, palette coverage).** The palette inventory
  (`crates/ui-model/src/palette.rs:69-131`) has 11 commands and **no** History/open-history
  command, although `Ctrl+H` and the inspector button open that surface; DC-MP-AX2's
  "palette action missing for a canon product action" is therefore only partially covered
  for the reopen-from-History step. Evidence: `shots/006-shot-palette.png`,
  `session-pass2.json` `surfaces_palette.rows`, `uia/031-p2-palette-tree.json`.

## `NOT_MEASURED` list (with reasons)

1. Screen-reader pronouncement and NVDA output — NVDA is not installed; no AT session exists.
2. UIA event stream (`UIA_AutomationFocusChangedEventId` and friends) — `accevent.exe` is
   present but requires elevation (`os error 740`), which this session cannot request.
3. Shift+Tab order — needs the event stream or a real AT client (see F1); posted keys cannot
   carry a modifier chord faithfully.
4. Enter/Space activation of a focused control — the only candidate control refuses UIA focus.
5. Japanese IME composition/commit/cancel — `ImmGetContext` returns NULL; no IME context, no
   real IME session.
6. Second and mixed display scale, DPI migration preserving focus/selection/anchor — one
   96-DPI display set; a session change would be needed, and a synthetic `WM_DPICHANGED`
   cannot be delivered safely.
7. High Contrast theme integration — changing the OS theme is out of scope for this audit.
8. Painted artifact image and media states — the isolated DB copy projects no current
   candidate, so the viewer surface is measured but no image is displayed.
9. Compare and Flow surface trees — not part of this audit's surface list.

## Reproduction

```text
cargo build --locked --release -p cos-desktop
PY=<python>  # any Python 3; the host `python.exe` is a Windows Store stub
"$PY" apps/desktop/devtools/runtime_integration/run_ax_ime_dpi.py              # main pass
"$PY" apps/desktop/devtools/runtime_integration/run_ax_ime_dpi.py --pass2      # foregrounded
"$PY" apps/desktop/devtools/runtime_integration/run_ax_ime_dpi.py --pass3      # sequences
"$PY" apps/desktop/devtools/runtime_integration/run_ax_ime_dpi.py --shots      # screenshots
"$PY" apps/desktop/devtools/runtime_integration/run_ax_ime_dpi.py --shortcuts  # Ctrl+H/N
```

Each session creates its own isolated Runtime copy under `target/acc1-native/<stamp>-ax-audit`,
writes the record, closes the window (`WM_CLOSE`, exit 0) and stops the Runtime through its
stop file (clean exit 0). Ports 8155 and 8188 were closed afterwards in every session.

## Leak scan and cleanup

- Leak scan (`run_acc1_native.leak_scan`) over this directory: **175 files checked, 2 per-run
  secrets checked, 0 findings** in the final session (`session-shortcuts.json`
  `leak_check`); each earlier session record carries the same scan for its own file set.
  No token, bearer header or media handle appears in the records; the Runtime endpoint, epoch
  and config path are recorded, the credential values are not.
- Cleanup per session: `WM_CLOSE` sent, Desktop exited 0 (`forced: null`), Runtime clean stop
  (exit 0), `8155`/`8188` closed, credential store entry for the run removed. No generation
  process, ComfyUI or extra listener was started at any point.

## What this evidence does not claim

- It is not product acceptance, not MP-AX1/MP-AX2 certification and not a screen-reader
  quality verdict.
- The `PrintWindow` captures of the Brief, History and palette show the dialog painted over an
  unpainted shell background (a documented `PrintWindow` partial-frame artefact of this host);
  the UIA tree captured the full shell at the same moment, so no visual defect is claimed from
  those images.
