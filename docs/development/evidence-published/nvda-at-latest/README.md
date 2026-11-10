# Real NVDA screen-reader session — COS Desktop v0.1.0 release

Bounded, falsifiable measurement of a **real NVDA session** against the v0.1.0 portable
release. This closes the "real screen-reader pronouncement" line that the release
`README-RELEASE.md` listed as limited, for the surfaces below. It is **not product
acceptance**, not MP-AX1/MP-AX2 certification and not a screen-reader quality verdict.

- **Verdict:** `MEASURED — PASS` for the bounded pronouncement measurement. 77 committed
  utterances (8 startup + 69 step) from the main surface, Brief, inspector/evidence, History and command palette,
  including real Tab traversal and dialog open/close. Three focus-pattern findings remain
  (F1–F3).
- **Run:** 2026-11-11T00:02:46 → 00:05:33 (+09:00). One isolated Runtime, one Desktop
  window, one NVDA process, no generation.
- **Release:** `target/cos-desktop-v0.1.0-portable-unsigned.zip` sha256
  `73bf6ca10e6ba4d457b6d770db827052e88dd8bb282ad19475e385bc88505a55`; the contained
  `cos-desktop.exe` is **21,258,240 B**, sha256
  `263134b5e8af53451366c81bb2ffe603c5b4c24d9e0c03248c4858ed4cdf6fce`, identical to
  `target/release-staging/cos-desktop.exe`. Repo HEAD at run:
  `92b705f0564e2884d37378d10ae7261370b68a59` (docs-only release record over `7fec9c7`).
  The worktree also carried unrelated uncommitted media-delay changes from another lane;
  they are **not** in this binary.
- **NVDA:** official NV Access **2026.3.1** installer, sha256
  `6f80a5b0ca720a7d0d314b5be212f37388bbe81a1107ac3298c1821a1cd3f052`, valid Authenticode
  signature by `CN=NV Access Limited`, file version `2026.3.1.58288`. Per-user deployment
  is the official portable copy (`--create-portable-silent`); the machine-wide
  `--install-silent` path requires elevation and was not run (see Install result).
- **Environment:** Windows 11 Pro `10.0.26300` x64; user `DIO\diobrando`, standard user
  (`IsAdminRole=false`), UAC enabled (`EnableLUA=1`, `ConsentPromptBehaviorUser=3`,
  `PromptOnSecureDesktop=1`).

## Authority and boundaries

- Test-only observation. No product source, fixture, Runtime or ComfyUI change was made.
- The Desktop was launched with **no Runtime endpoint/token** (discovery only) against the
  isolated MP-ACC1 Runtime copy on port **8155**. `health.llm`/`health.comfy` were `null`,
  `generation_plane_required=false`, `can_submit=true`; **no generation was requested**.
  Port 8188 was closed before and after.
- Chrome was minimized during the measurement (window handles only were recorded, never
  titles) and restored afterwards; this avoided unrelated browser speech. No browser
  content is committed.
- The raw NVDA log contained unrelated browser speech before the app session. It was
  filtered to speech naming a COS Desktop surface/control, hashed
  (`sha256:6d911c6027b8bb54186269cb73d087f90e4da4916d37592879df9cd210ae9bf5`,
  26,376 B) and deleted with the portable copy. No raw browser text is in this directory.

## Install result (no admin)

### Machine-wide silent install — `NOT_RUN_ELEVATION_REQUIRED`

NVDA 2026.3.1 has **no per-user install mode**. `source/gui/installerGui.py`
(`doInstall`) calls `systemUtils.execElevated(config.SLAVE_FILENAME, ["install", ...],
wait=True, handleAlreadyElevated=True)`; `source/systemUtils.py` (`execElevated`) sets
`lpVerb = "runas"` when the process is not already elevated; `source/installer.py`
(`install`) writes `WritePaths.defaultInstallDir` (`%ProgramFiles%\nvda`) and registers
under HKLM. With this environment's UAC settings that raises an **unattended secure-desktop
credential prompt**, which cannot be answered in this session. It was deliberately not
triggered: an unanswered UAC prompt would block the machine. This is the exact blocker,
not a simulated success.

### Portable per-user deployment — `PASS`

```text
nvda_2026.3.1.exe --create-portable-silent \
  --portable-path=%LOCALAPPDATA%\nvda-at\portable \
  -c %LOCALAPPDATA%\nvda-at\config \
  -f %LOCALAPPDATA%\nvda-at\logs\portable-create.log -l 12 -n ja --no-sr-flag
```

Exit code **0**. Portable `nvda.exe` sha256
`b3442f222cb677141138dab33a08f9baeae1b3b24b06e2cd9c1bf56083bd15a1`. The portable copy,
config, logs and installer were removed after the session (see `cleanup.json`).

## Method

1. Launch the release binary against the isolated Runtime; wait for the connected task
   rail (`タスク一覧`/`タスクの状態`), then foreground it.
2. Minimize Chrome (handles only), start portable NVDA with an isolated config
   (`-c`), IO-level logging (`-l 12`), Japanese UI/synth language (`-n ja`), and
   **Windows OneCore** (Japanese; `MSTTS_V110_jaJP_AyumiM` was selected in the isolated config during setup) as the synth.
3. Force an app deactivate/reactivate cycle while NVDA is running (minimize + restore the
   app window, the user-visible equivalent of alt-tabbing away and back). The AX focus-event
   follow-up showed this is required for GPUI to publish provider focus events in this
   environment.
4. Drive the app with real `SendInput` Tab/Escape/shortcuts and UIA `SetFocus`/`Invoke`
   through `probe_window.ps1`; after each step read the new NVDA
   `IO - speech.speech.speak` lines.
5. Speech is parsed from NVDA's own log, i.e. the exact text handed to the synthesizer.
   Nothing was generated or paraphrased. The committed log is restricted to speech that
   names a COS Desktop surface/control; the raw log is hashed and deleted.

NVDA `speechViewer` window was not opened by the final config; the probe returned
`window_not_found` (`nvda/speech-viewer-tree.json`). The log is the primary output.

## Captured output

`nvda-app.log` (155 lines), `speech.jsonl` (69 step utterances), `uia/*.json` (focus
transitions and trees), `shots/*.png`. Counts:

| Surface | Utterances | Evidence |
|---|---:|---|
| startup (app) | 8 | `session.json` `speech.startup` |
| main work surface + 12 Tab steps | 34 | `uia/main-tab-order.json`, `shots/main-tab-order.png` |
| Brief open / tabs / close | 3 / 3 / 3 | `uia/brief-open.json`, `uia/brief-tabs.json`, `uia/brief-close.json`, `shots/brief.png` |
| inspector / evidence | 7 | `uia/inspector-evidence.json` |
| History open / tabs / close | 4 / 4 / 6 | `uia/history-open.json`, `uia/history-tabs.json`, `uia/history-close.json`, `shots/history.png` |
| command palette open / tabs / close | 2 / 0 / 3 | `uia/palette-open.json`, `uia/palette-tabs.json`, `uia/palette-close.json`, `shots/palette.png` |

### Main work surface

Representative NVDA utterances (exact text, commands stripped):

```text
COS Desktop 作業画面
現在のタスクと状態 グループ
停止の操作と現在の受付状況 グループ
タスクを停止（現在は操作できません） ボタン 使用不能 機能の利用可否情報が投影されていません（capability not projected）
停止要求の受付状況を確認（現在は操作できません） ボタン 使用不能 受付状況を確認できる停止要求はありません。
タスク一覧 グループ
タスクを検索 エディット
task-00eeff7805964e1f835534b81e57065b — 北欧の女子高生。あざとい表情をしてこちらを見ている自撮り。 選択なし
作業領域：成果物 グループ
画像表示。0: 全体、1: 物理等倍、+/-: 拡大縮小、矢印: 移動、F11: 全画面、Escape: 復帰 グループ
候補の詳細 グループ
診断情報を開く ボタン
作業画面の切り替え グループ
成果物  Ctrl+1 ボタン 閲覧中の候補を扱う画面に切り替えます。
候補を比較 — 候補を2つ選ぶ必要があります ボタン 2つの候補を比較する画面です。現在は候補が2つに満たないため開けません。
処理の流れ  Ctrl+3 ボタン 処理のつながりを必要なときだけ確認する画面に切り替えます。
Microsoft IME
```

### Tab order (real SendInput, from the stop button)

`uia/main-tab-order.json` records each step's `focus_before`/`focus_after` and
`focus_after_type`:

```text
COS Desktop 作業画面 (Pane)
→ タスクを停止（現在は操作できません） (Button)
→ 停止要求の受付状況を確認（現在は操作できません） (Button)
→ タスクを検索 (Edit)
→ task-… — 北欧の女子高生… (ListItem)
→ 画像表示。… (Group)
→ 診断情報を開く (Button)
→ 成果物  Ctrl+1 (Button)
→ 候補を比較 — 候補を2つ選ぶ必要があります (Button)
→ 処理の流れ  Ctrl+3 (Button)
→ 目的 — 利用可能: task-… (Button)
```

The 12-step sequence produced these 11 distinct controls; one intermediate transition
(`成果物` → `候補比較` → `処理の流れ` → `目的`) is recorded in the probe's per-step
`focus_before`/`focus_after` fields and is a timing artefact of the probe, not an
additional accessible control.

### Brief dialog

```text
COS Desktop ウィンドウ
クリエイティブ・ブリーフ ダイアログ 編集中
目的 エディット 何を達成するかを入力…
方向性 エディット
ブリーフを提出（無効） ボタン 入力内容を確認してください
COS Desktop 作業画面          (after focus:目的 + Escape)
```

Open via `Ctrl+N`, close via refocusing `目的` then `Escape`; focus restoration to
`COS Desktop 作業画面` (Pane) was observed. After the submit button, Tab moved to
`COS Desktop` (Window) — see F1.

### Inspector / evidence

```text
候補の詳細 グループ
詳細と説明 グループ
目的 — 利用可能: task-… ボタン task-… 折りたたみ
評価 — 利用不可: 候補は選択されていません。 ボタン 候補は選択されていません。 折りたたみ
判断の根拠 — 利用不可: 候補は選択されていません。 ボタン 候補は選択されていません。 折りたたみ
改善 — 利用不可: 候補は選択されていません。 ボタン 候補は選択されていません。 折りたたみ
結果 — 利用可能: 正式状態: 完了（COMPLETED） ボタン 正式状態: 完了（COMPLETED） 折りたたみ
```

### History

```text
実行履歴 ダイアログ 履歴を読み込み中…
これより古い履歴はありません
実行履歴。0件。上下キーで移動、Enterで開きます。 リスト
実行履歴。1件。上下キーで移動、Enterで開きます。
反復 0。フェーズ RUNNING。実行 run-…。計画 plan-…。作成 2026-09-14T13:17:35.295123+00:00。記録 run-…:0。 この時点を開
```

Open via `Ctrl+H`; close via refocusing the list item then `Escape`, which restored
`結果 — 利用可能: 正式状態: 完了（COMPLETED）` (the inspector result button that owned
focus before the History opened). Tab from the list moved to the main stop button — F2.

### Command palette

```text
コマンドパレット ダイアログ
コマンド検索 エディット コマンドを入力…
```

Open via `Ctrl+K`; `Escape` from the search field restored
`結果 — 利用可能: 正式状態: 完了（COMPLETED）`. Two Tab presses left focus on
`コマンド検索` (0 new utterances) — F3.

## Findings

- **F1 (ACTIONABLE NON-BLOCKING — Brief focus containment).** After the disabled
  `ブリーフを提出（無効）` button, the next Tab moves focus to the main
  `COS Desktop` window (`ControlType.Window`), not back into the Brief. `Escape` from
  that state does not close the Brief; closing required refocusing `目的` first. Evidence:
  `uia/brief-tabs.json`, `uia/brief-close.json`.
- **F2 (ACTIONABLE NON-BLOCKING — History focus containment).** Tab from the History list
  item moves to the main `タスクを停止（現在は操作できません）` button; the History dialog
  is not a focus trap. Closing required refocusing the list item, then `Escape` restored
  the inspector result. Evidence: `uia/history-tabs.json`, `uia/history-close.json`.
- **F3 (ACTIONABLE NON-BLOCKING — palette Tab).** Two real Tab presses from
  `コマンド検索` left focus on the same `Edit` and produced no utterance; the palette list
  is apparently arrow-key/Enter driven. The arrow-key/Enter path is **not measured** here.
  Evidence: `uia/palette-tabs.json`.
- **F4 (NIT — PrintWindow partial frame).** The Brief and History `PrintWindow` captures
  show the dialog over an unpainted shell background (a known artefact of this host's
  `PrintWindow`). The dialog content and the simultaneous UIA trees are complete; no visual
  defect is claimed from those images.
- **F5 (NIT — probe timing).** The 12-step main Tab sequence records one repeated/racy
  focus transition; the per-step `focus_before`/`focus_after` fields preserve the exact
  observed order.

## Not measured

- **Braille output** — no braille display; config `display=noBraille`. The task's
  "spoken/braille" requirement is satisfied by the spoken log, not by braille.
- **NVDA speech-viewer window** — not opened by the final config; probe returned
  `window_not_found`. The IO log is the primary output.
- **Audio waveform / actual sound** — the log is the exact text passed to the OneCore
  synth; no audio capture was made.
- **NVDA object navigation, read-all of static text, menus, add-ons** — only focus/Tab,
  UIA `SetFocus`/`Invoke` and the listed shortcuts were driven.
- Other NVDA versions/synths, mixed DPI, High Contrast, Japanese IME composition, and
  product/UX acceptance.

## Reproduction

The harness is `harness/driver.py` (test-only; imports the repo's
`apps/desktop/devtools/runtime_integration/run_acc1_native.py` and
`probe_window.ps1`). `harness/chrome_windows.py` minimizes/restores Chrome with handles
only. The driver expects the v0.1.0 release exe at
`target/nvda-at/release/cos-desktop.exe`, NVDA portable at
`%LOCALAPPDATA%\nvda-at\portable`, and writes to this directory. It was run with:

```text
python target/nvda-at/driver.py --no-cleanup
```

The exact build/run/cleanup records are `session.json`, `install.json`,
`cleanup.json`, `hashes.txt` and `leak-scan.json`. `release/README-RELEASE.md` is the
release artifact's own README.

## Cleanup

Desktop exited 0, Runtime stopped cleanly (exit 0), ports 8155/8188 closed, NVDA exited
(no remaining `nvda` process), and Chrome was restored. The portable executables, config,
logs and the installer were removed; **two NVDA COM proxy DLLs**
(`IAccessible2proxy.dll`, `ISimpleDOM.dll`) remain under
`%LOCALAPPDATA%\nvda-at\portable\lib\2026.3.1\x64` because they are mapped into the
running Chrome, DeepSeek Harness and SteelSeriesGGClient processes. They are inert and
will be removable after those processes restart; no `nvda` process remains. See
`cleanup.json`. The isolated Runtime work directory (DB copy and per-run secrets) was
removed; the raw NVDA log was deleted after hashing/filtering.

## What this evidence does not claim

It is not product acceptance, not a screen-reader quality certification and not a
statement that every surface is fully accessible. It measures the listed utterances and
focus transitions on one release artifact, one NVDA build, one synth and one host, and it
records the three focus-containment findings above.
