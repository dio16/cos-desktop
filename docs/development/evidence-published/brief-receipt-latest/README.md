# MP-ACC1 Brief accepted-receipt paint — native re-measurement (2026-11-09)

Focused native re-measurement of the MP-ACC1 defect recorded in
[`acc1-native-latest/record-golden.json`](../acc1-native-latest/record-golden.json)
(`receipt_visible = false`, `task_ref_source = "live TASK projection (receipt not painted)"`):
after a Brief submit was accepted by the Runtime (command 200), the accepted
receipt was never painted and the created task could only be resolved through
the live `TASK` projection.

- **Verdict:** `PASS` for the receipt surface (one bounded run; generation and
  terminal surfaces are `NOT_MEASURED` here — the run stopped once the receipt
  was painted).
- **Run:** 2026-11-09T22:30:39+0900 → 2026-11-09T22:31:13+0900.
- **Desktop:** `C:/dev/cos-desktop`, committed head
  `a8f332a236191054e1ad277ca3e8fdbdb758165f` (the fix commit). Release binary
  `target/release/cos-desktop.exe`, **21,114,368 B**, sha256
  `sha256:59e015425b00ed33f22c55761ca67682a9bc5d74d6d62c71c599fc8cf08e1360`;
  `cargo build --locked --release -p cos-desktop` exited 0 from the committed
  sources and did not relink afterwards (binary identity unchanged).
- **Harness:** `apps/desktop/devtools/runtime_integration/run_acc1_native.py`
  with the test-only `--scenario receipt` variant (same isolated Runtime,
  discovery, per-run secrets and native UI path as the MP-ACC1 lane).
- **Runtime:** `C:/AI/orchestrator`, head
  `b794b63786ac0cea342d7d1cb72e57cd6974baec`. Isolation: fresh read-only DB
  copy of `generation.sqlite` (**11,493,376 B**, sha256 `f8771100…bfb3e3d`),
  copied presets (`e1c2b553…baae3c`), own config/logs/CAS/outputs, port
  **8132**, per-run `ORCHESTRATOR_AUTH_SECRET` and
  `ORCHESTRATOR_COS_MEDIA_HANDLE_SECRET`, redirected RT8 discovery descriptor.
  The Desktop was launched with **no** `COS_DESKTOP_RUNTIME_ENDPOINT` and
  **no** `COS_DESKTOP_RUNTIME_TOKEN`: only the per-run issuance secret and the
  discovery path. Tokens and secrets are never written to evidence.
- **Generation plane:** not started for this bounded run (ComfyUI
  `127.0.0.1:8188` unreachable, Ollama `/api/tags` 200 with 6 models; the
  Runtime reported `{"llm": null, "comfy": null}`). The receipt path does not
  require generation; the run stopped after the receipt was painted and did
  not wait for a terminal task.

## What was measured (native window)

| Step | Verdict | Observed |
|---|---|---|
| Isolated Runtime readiness + credential + catalog | **PASS** | `/api/ready` 200; credential issuance 200; catalog 200, `can_submit=true`, one recipe (`Production still (Krea2)`); initial observation open 200 (260 snapshot mutations, read revision 1). |
| Discovery-only native launch | **PASS** | Window `COS Desktop` (`Zed::Window`, 1296×839); UIA task rail and connection labels rendered; screenshot `sha256:5b81faa6…98f123`. |
| Brief UI entry and recipe selection | **PASS** | UIA `Invoke` opened `クリエイティブ・ブリーフ`; Goal / Direction entered through the click-then-`WM_CHAR` path; recipe selected; `ブリーフを提出` enabled. |
| Brief submit | **PASS** | UIA `Invoke` on `ブリーフを提出`; Runtime access log records `POST /api/cos/desktop/commands` **200**. |
| Accepted receipt painted | **PASS** | The Brief surface paints `受付済み — 観測待ち`, `受付タスク: task-3cf842eeb9f24e1f82d8163d0b08a716` and `受付済みのため編集できません`; UIA names tail and `PrintWindow` screenshot captured **before** any generation wait (see hashes below). Receipt painted at 22:31:05; the receipt carries the server task id. |
| Task appears only via observation | **PASS** | The harness's read:observe client resolves the same `task-3cf842eeb9f24e1f82d8163d0b08a716` from the live `TASK` projection (`ACTIVE` / `RUNNING`, projection revision 262) — the receipt and the projection agree, and no task row was fabricated locally. |

## Before / after

| | Before (`acc1-native-latest`, golden) | After (this run) |
|---|---|---|
| `receipt_visible` | `false` | `true` |
| `task_ref_source` | `live TASK projection (receipt not painted)` | `Brief receipt (受付タスク)` |
| Painted accepted receipt | absent | `受付済み — 観測待ち` + `受付タスク: task-…` |
| Task row origin | live `TASK` projection | live `TASK` projection (same task ref; the receipt is not a canonical task row) |

## Root cause and fix (product code)

- The submit path resolves as soon as the command channel **admits** the
  intent (client state `Pending`); the immediate receipt read can therefore only
  answer "pending". The accepted receipt arrives later on the observation
  publication (`command_statuses` view with `ReceiptAvailable` +
  `ReceiptState::Accepted`), but nothing applied it to the Brief form, so the
  form stayed in `送信済み` and no accepted banner was painted.
- `AuthorizationUnavailable` from a still-verifying read was also classified as
  a denial (`受付票を開示できません`), which claimed a Runtime rejection that
  never happened.
- Fix commit `a8f332a`:
  - `desktop-gpui`: the subscription now applies the command receipt carried by
    the current complete publication to the open Brief (`brief_apply_published_receipt`),
    so the accepted state is painted event-driven (no poll, no second submit);
    `AuthorizationUnavailable` is classified as pending, not denied; a late
    pending/unknown/denied answer never downgrades an accepted receipt; a later
    accepted answer may fill a missing task reference.
  - `ui-model`: `on_accepted` is definitive for its command (it may fill a
    missing task ref, never clear/downgrade it) and `on_unknown` cannot replace
    an accepted receipt.
  - The accepted banner (state label and `受付タスク` line) is an accessible
    `Role::Label` with name/value, so UIA and screen readers can read the
    accepted surface; it is still a receipt label, not a task row.
  - Tests: ui-model receipt transitions (accepted → painted state, no task
    observation, no downgrade), desktop-gpui classification/banner/publication
    tests, and an app-core adapter test that the accepted receipt reaches the
    publication without adding a canonical task row.

## UIA / screenshot hashes

| State | File | sha256 |
|---|---|---|
| Connected rail | `shots/receipt/connected-receipt.png` | `sha256:5b81faa6db49e9b67e9b33fddf916a17746b7567ab5e3af74ef38be56098f123` |
| Brief filled | `shots/receipt/brief-filled.png` | `sha256:263354a688f091cb349c378291914f1f2d2149de6bb66e2777e689d988e3280d` |
| Accepted receipt surface | `shots/receipt/receipt-accepted.png` | `sha256:190f813e99a725756b65089f9a5e53bb157d659d16544da28db6645364148325` |
| Submitted snapshot (same surface) | `shots/receipt/brief-submitted.png` | `sha256:190f813e99a725756b65089f9a5e53bb157d659d16544da28db6645364148325` |

The accepted-receipt UIA dump (retained as
[`uia/receipt/023-dump.json`](uia/receipt/023-dump.json), captured at
22:31:09; the `020`/`021` polls before the snapshot carry the identical
`names_sha256`) contains 75 elements with
`names_sha256 = sha256:be6dbf695c748e739ca583404b7aa4e1ae78bdb488dd43df63bc6c79fe83e955`;
its tail is:

```text
受付済み — 観測待ち
受付タスク: task-3cf842eeb9f24e1f82d8163d0b08a716
受付済みのため編集できません
受付票を再確認
ブリーフを提出（無効）
```

The named UIA result documents and screenshots are under `uia/receipt/` and
`shots/receipt/`; the raw per-poll UIA dumps are local scratch (not retained),
as in the MP-ACC1 lane.

## Leak scan

- The harness scanned **29** evidence files for **3** per-run capability/secret
  values plus `Bearer …` and `mh1.` handle patterns: **0 findings**
  (`session.json` → `leak_check.clean = true`).
- Runtime and Desktop logs are sanitized before retention
  (`logs/receipt/`); no token, context id, cursor or media handle is stored in
  plaintext. The evidence carries no per-run secret, and `_secrets` is stripped
  from the retained records.

## Cleanup

Both owned processes were closed cleanly: the Desktop received `WM_CLOSE`
(exit 0), the isolated Runtime stopped (exit 0), port 8132 is free, the
discovery descriptor is removed, and both credential-store target forms are
absent (`not_found`, honest non-deletion; the Desktop removed its stored
credential on explicit teardown). ComfyUI was never started for this run
(8188 closed); Ollama was untouched.

## What was NOT measured

- Generation through terminal, artifact/media surface, evaluation, formal
  outcome, History/Compare/Flow interaction: `NOT_MEASURED` here — the run
  stopped once the accepted receipt was painted, by design.
- The receipt-repaint path after a Desktop restart or credential revocation:
  not exercised in this run.
- Native screen-reader/IME/AX/DPI acceptance remains outside this run; the
  accepted banner is now exposed as an accessible label, but the full AX matrix
  is not re-claimed here.
- Product acceptance and ACC2 remain the Product owner's; this is focused
  defect evidence, not an acceptance record.

## Reproduction

```text
cd C:/dev/cos-desktop
C:/AI/orchestrator/.venv/Scripts/python.exe -B ^
  apps/desktop/devtools/runtime_integration/run_acc1_native.py ^
  --scenario receipt --no-build ^
  --evidence-dir docs/development/evidence-published/brief-receipt-latest
```

Ports 8132/8133 must be free, `C:/AI/orchestrator/.venv` must exist, and the
release binary must be built from the committed sources. The receipt variant
intentionally does not start or require ComfyUI; it stops after the accepted
receipt is painted.
