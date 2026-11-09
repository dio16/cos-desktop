# MP-ACC1 native closure: artifact display, STOP, missing media and native surfaces (2026-11-10)

Bounded **native acceptance campaign** for [DC-MP-ACC1](../../../architecture/DESKTOP_DESIGN_CONTRACTS.md#dc-mp-acc1),
closing the residuals named in plan §19.318/§19.319 after the Runtime RC-11
command/media projection (`943e6b21`) and the journal-attribution fixes
(`b794b637`, `0378fa40`) and the Desktop Brief-receipt paint (`a8f332a`).
Evidence-only test-harness work: no product code, no Runtime file, no plan edit,
no push. This is **not product acceptance**; ACC2 remains the Product owner's.

## Verdict

| # | Scenario | Verdict | Exact observed result |
|---|---|---|---|
| 1 | Golden generation (receipt → dispatch → live projection → terminal → OUTCOME) | **PASS** | `task-ab47119a8bec467a8e128d7b82f1220a`: `COMPLETED` / `SUCCESS_ACCEPT`; artifact `sha256:eea85a24…3908a58`, 1,363,011 B, PNG 1024×1024; **0** `409 RESYNC_REQUIRED` in 1,986 serving responses; epoch `419e40cf…0fb9550` unchanged across the mid-generation Desktop restart |
| 1b | Native artifact surface displays the actual image | **PASS** | `メディア画像（記述子のダイジェスト検証済み）: 低解像度プレビュー。原寸の詳細表示・1:1ではありません。`; app requested descriptor **200** then blob **200**; computed SHA-256 equals the descriptor digest; image viewport present (`全体表示 (0)`/`物理ピクセル1:1 (1)`) |
| 2 | STOP on a live ACTIVE generation | **PASS** | capability `STOP_TASK: SUPPORTED`; control enabled; command receipt `停止要求を受け付けました`; explicit receipt lookup `停止要求の受付状況を確認` → `停止要求を受け付けました`; projected state `CANCELLED` (completion claimed only from the projection) |
| 3 | Missing media after the descriptor was observed and the image displayed | **PASS** | displayed first; isolated CAS blob (1,363,011 B, digest-matching) removed; descriptor still 200, blob fetch **404**; after a clean Desktop restart/reload the native surface states `画像を利用できません: この成果物のメディアが見つかりません。`; no bytes fabricated |
| 4 | Native History / Evidence / Flow / Compare | **PASS with PARTIAL evidence** | History **PASS** (open → load 1 row → Enter → close → reopen, row is this task's `run-…-i0`); Evidence **PARTIAL** (detail sections present but groups not projected: `判断の根拠 — 利用不可: 選択中の候補に判断の根拠はありません。` / `改善 — 利用不可: この反復には改善記録がありません。`); Flow **PASS** (`画像生成` node, hierarchy CREATION, edge list present); Compare **PASS** (pair selection: this task's candidate selected; `比較対象が変更されました…`; A/B panes and `A側を変更`/`B側を変更`/`比較を閉じる` exposed; the compare images were still `画像は未表示です`) |
| 5 | Multi-recipe | **NOT_RUN** | authenticated catalog exposes **one** recipe (`Production still (Krea2)` / `recipe-cos-production-still`); a second recipe cannot be run without changing the catalog |
| 6 | Further generations | **NOT_RUN** | ComfyUI crashed after the first generation and cannot restart (see *Generation plane*); exact error retained; no synthetic image or success claimed |

Overall session verdict: **PASS** for the measured steps, with the named
residuals below. The golden generation and the native display are a **split
measurement**: generation/projection/OUTCOME come from the live generation run,
the display from a bounded follow-up session on the *same* isolated DB copy and
CAS under a fresh serving epoch (the record says so explicitly; no generation
was re-run for the display).

## Environment

- **Desktop:** `C:/dev/cos-desktop`, head `1b2fdc8885340cda744eed0765e2d90a1897d9b4`
  (docs/evidence commits only; product code is unchanged since `a8f332a`, the
  Brief-receipt fix). Release binary `target/release/cos-desktop.exe`,
  **21,114,368 B**, sha256
  `sha256:59e015425b00ed33f22c55761ca67682a9bc5d74d6d62c71c599fc8cf08e1360`
  (built from `a8f332a`; not rebuilt here, so the earlier `target/release`
  ACL/delete-lock residual cannot affect this run).
- **Runtime:** `C:/AI/orchestrator`. The golden generation ran at
  `b794b63786ac0cea342d7d1cb72e57cd6974baec`; the final native session ran at
  `e1bc3e68944f4c4658efa7310dbab1dc8698fd5d` (the checkout was advanced by
  another lane at 00:51:31 during the campaign). RC-11 `943e6b21` and the
  journal fixes `b794b637`/`0378fa40` are ancestors of both. The seven
  pre-existing unrelated dirty entries were never staged or modified.
- **Isolation per run:** fresh read-only DB copy of `generation.sqlite`
  (11,493,376 B, sha256 `f8771100…bfb3e3d`), copied presets, own
  config/logs/CAS/outputs, port **8132** (main) / **8133** (injections),
  per-run `ORCHESTRATOR_AUTH_SECRET` and `ORCHESTRATOR_COS_MEDIA_HANDLE_SECRET`,
  redirected RT8 discovery descriptor. The Desktop launched with **no**
  `COS_DESKTOP_RUNTIME_ENDPOINT` and **no** `COS_DESKTOP_RUNTIME_TOKEN` — only
  the per-run issuance secret and the discovery path. Tokens and media handles
  are never retained.
- **Generation plane:** ComfyUI **0.35.1** was started for this campaign with
  the documented `scripts/start_services.ps1` (flags `--lowvram
  --reserve-vram 1.5 --cache-lru 10 --preview-method taesd
  --use-pytorch-cross-attention`), `/system_stats` 200, and the first
  generation completed. The process then died; every restart now fails with
  `ModuleNotFoundError: No module named 'comfy_aimdo.storage'`
  (`comfy-aimdo 0.5.3` does not provide the submodule the updated ComfyUI
  imports). Ollama `/api/tags` 200 with 6 models, untouched.

## Capability material (test-only seed, no projection written)

The Runtime composes `CAPABILITIES` only when a `capability_snapshot_sets` row
exists for the task's run; the production-derived copy has none, so the native
gates would stay `capability not projected` forever. The harness seeds **one
Rust-validated authoritative snapshot set** into the isolated copy with the same
method as the RT5 lane (`capability_registry.store_snapshot_set`; the process
runs under `COS_RUNTIME_PROCESS_ROLE=generation-worker`, so `Database.init()`
never claims the observation epoch — verified: the epoch row is unchanged and
the ledger stayed serving-owned). The harness never writes a projection, a
receipt or a state: RC-11 derives command/media support from the task status and
the Artifact projection at every cut. Observed projection in the golden session:

```text
commands: STOP_TASK UNAVAILABLE ("task is terminal; STOP_TASK has no live state to act on")
          REUSE_TASK UNSUPPORTED ("REUSE_TASK is not supported by this Runtime")
media:    ORIGINAL/THUMBNAIL SUPPORTED, PREVIEW/METADATA UNSUPPORTED (fixed reasons)
availability SUPPORTED, capabilities_ref correlates with TASK.capabilities_ref
```

## Native media display (record-artifact.json)

- `受付済み — 観測待ち` painted after Brief submit; task resolved from the live
  `TASK` projection (receipt + projection agree).
- After the dock navigated to the task's candidate, the Runtime access log shows
  the Desktop's own `GET /api/cos/media/sha256%3Aeea85a…` **200** and
  `GET /api/cos/media/mh1.…` **200**; the UIA image area reports
  `メディア画像（記述子のダイジェスト検証済み）: 低解像度プレビュー。…` with the
  viewport controls present. The harness's own descriptor→handle→blob fetch
  returned 1,363,011 B whose SHA-256 equals the descriptor digest.
- The displayed representation is explicitly labelled a **low-resolution
  preview** by the app (the admitted decode class), not the 1:1 original;
  the app never claims the original.

## STOP (record-stop-active.json)

- The task was a live `ACTIVE`/`RUNNING` task in the reused isolated DB; the
  capability projection reported `STOP_TASK: SUPPORTED` (no accepted stop
  command) and the native control was enabled (`タスクを停止`).
- Clicking it sent the command and painted `停止要求を受け付けました` and
  `Desktop内 · 停止要求を受け付けました — 詳細を表示`; the separate bounded
  lookup (`停止の受付状況`) re-read the same accepted receipt. No automatic
  resend.
- The **projected** task state then changed to `CANCELLED` (recorded
  transition). Completion is claimed only from that projection.
- The reused task was created by a previous session's Desktop principal, so the
  isolated copy's `creative_tasks.created_by` was rebound to the measured
  session principal (registry order: the app credential is issued last) before
  the command. This is explicit test setup (recorded as `test_only_rebind`);
  without it the Runtime correctly answered `403 stop belongs to the task owner`
  and no projected transition happened.

## Missing media (record-missing-media-reload.json)

1. The image was displayed first (`メディア画像（…）…`), with the descriptor and
   blob both 200 and the CAS blob hash equal to the descriptor digest.
2. Only the **isolated copy's** blob was removed
   (`blobs/ee/eea85a…3908a58`, 1,363,011 B).
3. Descriptor still 200; the blob fetch was refused **404**; the harness's own
   fetch reported `blob_refused` and wrote no file.
4. The Desktop was closed cleanly (`WM_CLOSE`, exit 0) and relaunched (fresh
   credential, same discovery, same epoch for that Runtime process), which forces
   a reload with no in-memory texture: the artifact surface states
   `画像を利用できません: この成果物のメディアが見つかりません。` (`MISSING`).
   No image or placeholder bytes were fabricated.

## Resync and epoch

- Golden generation: **0** `409 RESYNC_REQUIRED` (`HTTP/1.1` histogram:
  1,986 × 200 in the serving access log; zero 409/unknown-owner/state-diff
  answers). The two Desktop launches (initial and mid-generation restart) both
  discovered epoch `419e40cfe45e4ea297ee63bde0fb9550`; the single
  `cos_desktop_observation_epoch` row was serving-owned and unchanged. The
  journal-attribution fix (`b794b637` + `0378fa40`) therefore yields no
  unknown-owner or state-diff resync in this run.
- The follow-up sessions each observed **0** resyncs on their own publications.

## Native surfaces (record-artifact.json → `steps.surfaces`)

The rail row is not a UIA `Button` (no Invoke/Selection pattern), so selection
now uses a synthetic pointer click inside its UIA bounds and is verified by the
inspector's `目的 — 利用可能: <task_ref>` scope line.

- **History — PASS:** opened from the inspector action, loaded
  `実行履歴。1件。上下キーで移動、Enterで開きます。`, the row resolved to this
  task's run `run-cos-task-ab47119a…-i0` (phase `RUNNING`, plan
  `plan-cos-task-ab47119a…-i0`), Enter activated it, the surface closed and
  reopened.
- **Evidence — PARTIAL:** the evidence detail sections are present and the
  inspector states an explicit unavailable reason
  (`判断の根拠 — 利用不可: 選択中の候補に判断の根拠はありません。`,
  `改善 — 利用不可: この反復には改善記録がありません。`); no group disclosure
  (`… 根拠は N 件 …`) is projected for this candidate, so there is nothing to
  expand. Recorded as PARTIAL, not as a fabricated group.
- **Flow — PASS:** `処理の流れ` surface shows the graph node
  `図の処理：image-generation。階層：CREATION。ノードID：creation-1。` and the
  `接続関係の一覧` group with the freshness notice.
- **Compare — PASS (pair selection measured, images not displayed):** the
  completed task's candidate was selected as a compare target; the surface
  exposes A/B panes, `A側を変更`, `B側を変更`, `比較を閉じる`, and the notice
  `比較対象が変更されました。この候補の認可済み画像はまだ表示されていません。`
  Both panes reported `画像は未表示です` during this bounded window. The second
  pair selection resolved to the candidate-group header (`反復回数: 第0回`)
  rather than a second distinct candidate, which is recorded as measured.

## Multi-recipe

**NOT_RUN.** The authenticated catalog returned exactly one recipe
(`Production still (Krea2)`); the run record retains the catalog response.
Running a second recipe would require changing the catalog or the Runtime,
which is outside this bounded campaign.

## Generation-plane failure (exact)

```text
ComfyUI 0.35.1 /system_stats 200 (started 23:14 with scripts/start_services.ps1)
first generation completed (artifact above)
later restart:
  File "…\ComfyUI\comfy\utils.py", line 26, in <module>
    import comfy.storage
  File "…\ComfyUI\comfy\storage.py", line 7, in <module>
    import comfy_aimdo.storage
ModuleNotFoundError: No module named 'comfy_aimdo.storage'
installed: comfy-aimdo 0.5.3 (no storage submodule)
```

Consequences, recorded honestly: a second full generation, a fresh-generation
STOP race, and the corrupt-blob (digest-mismatch) variant are **NOT_RUN**. The
STOP and missing-media measurements therefore reuse the already-measured task
and its canonical artifact instead of fabricating a new one. ComfyUI was not
started at the end of the campaign (its prior stopped state is restored);
repairing the third-party install was not attempted in this bounded task.

## Leak scan

`session.json` → `leak_check`: **287 files checked**, patterns `mh1.…`,
`Bearer <token>`, 48-hex secret-like values → **0 findings**. Media handles in
records and the reconstructed access log are replaced with
`<redacted-media-handle>`; no token, context id, cursor or per-run secret is
stored. The harness now redacts records before writing
(`redact_document`).

## Cleanup

Every run's Desktop closed with `WM_CLOSE` (exit 0) and every isolated Runtime
exited cleanly (`exit 0`); ports **8132/8133** are free, discovery descriptors
are removed, credential-store targets are absent (`not_found`, honest
non-deletion), no `cos-desktop`/isolated-Runtime process remains, and ComfyUI
is not listening on 8188 (prior stopped state). Ollama was untouched.

## Files

- `session.json` — the merged campaign session (verdicts, provenance, leak scan,
  cleanup); `record-artifact.json`, `record-stop-active.json`,
  `record-missing-media-reload.json`, `record-golden-generation.json`.
  `session-partial.json` is an early abandoned partial session (verdict
  `NOT_RUN`) kept for traceability and is **not** part of the measured result.
- `golden-generation/` — reconstructed generation record material: the task's
  isolated DB access log (sanitized), the **commit-sanitized copies** of the
  Desktop logs (`desktop-main.log.sanitized.txt`,
  `desktop-relaunch.log.sanitized.txt`; the raw `*.log` files are gitignored and
  retained outside the repository), and the retained UIA/PrintWindow
  screenshots with their SHA-256 values.
- `logs/<run>/`, `uia/<run>/`, `shots/<run>/`, `artifact.png` — per-run
  sanitized logs and native evidence. `manifest.json` pins the committed files'
  size and SHA-256, **except** its own self-entry (stale by construction) and
  the gitignored raw logs, which are not in the repository.

`record-golden-generation.json` is marked `reconstructed`: the original record
written by the generation run was cleared before this campaign's evidence
directory was finalized. Every field is re-derived from that run's own isolated
DB copy, serving access log, Desktop logs and retained artifacts; the run
directory is `target/acc1-native/20261109T233420-golden`.

## Reproduction

```text
cd C:/dev/cos-desktop
# 1st (blocked now by the ComfyUI dependency break)
C:/AI/orchestrator/.venv/Scripts/python.exe -B ^
  apps/desktop/devtools/runtime_integration/run_acc1_native.py ^
  --scenario golden --no-build --evidence-dir docs/development/evidence-published/acc1-native-closure-latest
# bounded follow-ups on a measured run's isolated DB/CAS
… --scenario artifact            --source-run target/acc1-native/<run> --task-ref <task> --no-build
… --scenario stop-active         --source-run target/acc1-native/<run> --task-ref <task> --no-build
… --scenario missing-media-reload --source-run target/acc1-native/<run> --task-ref <task> --no-build
```

Requires `C:/AI/orchestrator/.venv`, ports 8132/8133 free, and a built release
`cos-desktop.exe`; the artifact/stop/missing-media variants do not need ComfyUI
or Ollama. The `--no-build` flag is used because the binary provenance is pinned
above.

## What is NOT measured

- A second full generation, a fresh-generation STOP race, the corrupt-blob
  (digest-mismatch) variant, and multi-recipe breadth: **NOT_RUN** (ComfyUI
  dependency break / one-recipe catalog; exact reasons above).
- Evidence group disclosure with available groups (only the explicit
  `利用不可` state is projected for this candidate).
- Compare A/B image display (panes report `画像は未表示です` in this window).
  **Follow-up 2026-11-10:** this was a product defect in the production Compare
  media wiring, not a media-availability state; [compare-panes-latest](../compare-panes-latest/README.md)
  fixes it at `3e381ca` and observes one compare pane displaying the same
  digest-verified admitted preview as the artifact viewer (port 8134, fresh
  DB/CAS copy). Showing both panes simultaneously still needs a two-candidate
  task, which this closure DB does not contain.
- Start / Next Iteration (DD-25 command semantics remain blocked).
- Screen-reader/IME/High-Contrast/DPI acceptance, and original-resolution (1:1)
  display (the app labels the displayed class a low-resolution preview).
- Product acceptance and ACC2 remain the Product owner's.
