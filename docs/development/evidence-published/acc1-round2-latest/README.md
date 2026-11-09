# MP-ACC1 / RT3 round 2: post-repair generation, revoked-credential STOP and provider task survival (2026-11-10)

Bounded native acceptance wave on the release Desktop binary against an isolated
Runtime started from `C:/AI/orchestrator`. This pack is **evidence-only**: no
product code was changed by this task, nothing was pushed, and no result is
inferred from UI text alone. Product acceptance (ACC2) remains the Product
owner's.

## Verdict

| # | Task | Verdict | Exact observed result |
|---|---|---|---|
| 1 | Post-repair generation through the native Brief | **PASS** | Task `task-4e0596048e8640b29fcd5fc9fada550a`: `COMPLETED` / `SUCCESS_ACCEPT` (formal outcome `sha256:f486f713…a000`); artifact `sha256:fcbfd019…f659d`, **1,502,447 B**, PNG 1024×1024, digest matches the descriptor; native surface `DISPLAYED` (low-resolution preview label); serving epoch `fb92c665…a657` unchanged; **0** resyncs |
| 2 | Native STOP after Desktop credential revocation during a live generation | **PARTIAL** | STOP command requests were sent: `POST /api/cos/desktop/commands` **200 then 403**; the observation path returned `deltas 401`, `credentials/refresh 401`, then `deltas 403` / `observation 403` and a fresh `observation 200`; **no accepted `stop_task` row**; the UI never claimed `停止要求を受け付けました`; projected state **`COMPLETED`** (not `CANCELLED`). The exact 401-on-command case did not occur |
| 3 | Provider task survival across an isolated Runtime restart during a real generation | **NOT_RUN for same-process Desktop reconnect / PARTIAL overall** | Runtime stopped cleanly (10.81 s, exit 0) and restarted under a **new epoch** `597675ae…445b`; the durable task stayed `ACTIVE` with a durable ComfyUI receipt `028e359b-…3c29`; ComfyUI history for that prompt later reported `success` with one output image, yet the Runtime persisted **no outcome** and did not resubmit. The same Desktop process did **not** reconnect automatically; the explicit `再接続する` action did not recover it. A **fresh Desktop session** on the same DB copy recovered the `ACTIVE` task under epoch `83714d4f…e0bb` (`restart-followup` **PASS**). Generation survival to a projected terminal state was **not** achieved |

Session verdict: **PARTIAL** (`golden` PASS, `stop-revoked` PARTIAL,
`runtime-restart` NOT_RUN, `restart-followup` PASS). The split `runtime-restart`
+ `restart-followup` pair is deliberate: the first record measures the loss and
the failed same-process reconnect; the second measures the durable recovery
path in a fresh session. Survival is **not** claimed from either record.

## Environment and provenance

- **Desktop binary measured:** built from a clean detached worktree at the
  pre-state commit `eb29f62f7c07c4ce0c1af01f3e915ee50e42000a`
  (tree `c65cb633…4140`), 21,120,512 B,
  sha256 `ec4d7402674f2f47b57d180c692cbf89943833e7c62b48482d41175a45190f67`.
  Build: `cargo build --locked --release -p cos-desktop` with a dedicated
  `CARGO_TARGET_DIR`; only the harmless linker warning `LNK4044
  (/fuse-ld=lld ignored)` was emitted. See [provenance.json](provenance.json).
  The exact measured binary is retained (gitignored) at
  `target/acc1-round2-head-binary/cos-desktop.exe` with its SHA-256; the build
  worktree/target are temporary.
- **Why a pinned worktree:** the shared `dev` checkout advanced while this
  campaign ran (another lane committed `3e381ca`, `ff15fd5`, `d2d5612` and
  rebuilt `target/release/cos-desktop.exe` at 04:42 from uncommitted Compare
  changes). An initial pack built on that moving binary was invalidated and
  superseded; the final pack above uses only the clean `eb29f62` build. The
  main checkout head at finalization was `d2d5612…7928`; this task's commit
  adds harness/evidence only.
- **Runtime:** `C:/AI/orchestrator`, head `e1bc3e68944f4c4658efa7310dbab1dc8698fd5d`;
  the seven pre-existing dirty entries were never staged or modified.
- **Generation plane:** ComfyUI **0.44.0** with `comfy-aimdo 0.6.0` and
  `comfy-kitchen 0.2.41`, started with the documented
  `scripts/start_services.ps1` flags (`--lowvram --reserve-vram 1.5
  --cache-lru 10 --preview-method taesd --use-pytorch-cross-attention`),
  `/system_stats` **200**, argv confirmed in
  [record-golden.json](record-golden.json) (`health.direct_preflight`). Ollama
  `/api/tags` 200 with 6 models throughout.
- **Isolation:** each run copied `db/generation.sqlite` through a read-only
  SQLite backup into its own work directory, with its own CAS/logs/outputs,
  per-run `ORCHESTRATOR_AUTH_SECRET` and media-handle secret, ports **8132**
  (main) / **8133** (injection) and a redirected RT8 discovery descriptor. The
  Desktop was launched with **no** endpoint/token environment, only the per-run
  issuance secret and the discovery path. Tokens, context ids and media handles
  are never retained in the evidence.

## 1 — Post-repair generation (PASS)

Run directory: `target/acc1-native/20261110T051341-golden` (raw logs retained
outside the repository). Detail: [record-golden.json](record-golden.json),
[artifact.png](artifact.png).

- The native Brief created `task-4e0596048e8640b29fcd5fc9fada550a`
  (`ACC1-NATIVE-golden 20261110-051341 production still`). The accepted receipt
  was **not painted** in this run; the harness resolved the task from the live
  `TASK` projection (`task_ref_source = "live TASK projection (receipt not
  painted)"`). This is recorded as observed, not as a generation failure.
- Projection timeline: `ASSIGNED` → `ACTIVE`/`RUNNING` at 05:14:35 → terminal
  `COMPLETED` at 05:16:05 (revision 268). A Desktop restart was injected at
  05:14:45 (WM_CLOSE exit 0, relaunch with the same discovery/issuance secret);
  the serving epoch stayed `fb92c665bc7a49968b5c5e91e5baa657` and the harness
  recorded **0** `409 RESYNC_REQUIRED`.
- Terminal outcome: `formal_status = COMPLETED`, `stop_reason = SUCCESS_ACCEPT`,
  selected artifact `sha256:fcbfd0195b0f930df778fb9d6e18535eef6fcd318dd3eb88c8ba5a0087ef659d`.
- The canonical media fetch returned **1,502,447 B**, PNG **1024×1024**, and its
  computed SHA-256 equals the descriptor digest. The native artifact surface
  reported `メディア画像（記述子のダイジェスト検証済み）: 低解像度プレビュー。…`
  with the viewport controls present (`native_view_state = DISPLAYED`).
- Native surfaces at terminal: History **PASS**, Flow **PASS**, Compare
  **PASS**, Evidence **PARTIAL** (explicit `利用不可` reasons, no group
  disclosure) — same shape as the closure campaign.

This closes the "further generation NOT_RUN after repair" residual with one
real generation, a terminal projected state and digest-matching bytes. It does
not claim multi-generation breadth or a second recipe.

## 2 — GUI STOP after revocation (PARTIAL)

Run directory: `target/acc1-native/20261110T053811-stop-revoked`. Detail:
[record-stop-revoked.json](record-stop-revoked.json).

- The task was a live `ACTIVE`/`RUNNING` generation
  (`task-43660b72f284446e9a7795d4eead59ad`, created via the native Brief at
  05:38:11). One test-only authoritative capability set was seeded for the
  drain-created run; the Runtime itself projected `STOP_TASK: SUPPORTED`
  (05:39:11) and the native control was enabled (`タスクを停止`).
- At 05:39:23 the isolated control channel revoked **every** Desktop credential
  held by the serving process (`revoked_count = 2`; principals only, no token
  values). The native STOP control was invoked at 05:39:29 (click matched the
  `タスクを停止` button).
- Exact command path observed in the serving access log (in log order):
  `POST /api/cos/desktop/commands` **200**, `POST /api/cos/desktop/commands/lookup`
  **200**, `deltas` **401**, `credentials/refresh` **401**, `credentials` **200**,
  `deltas` (old cursor) **403**, `observation` **403**, fresh `observation`
  **200**, then `POST /api/cos/desktop/commands` **403**. The app logged
  `Runtime credential reauthorized after rejection; the session reopens a fresh
  observation`.
- **No accepted `stop_task` command row exists** in the isolated DB; the only
  `cos_commands` row for the task is the original `create_task`. The UI at
  05:39:31 showed `情報源なし / データ非公開`, `停止要求の状況を確認できません`,
  `現在表示できる情報がないため、以前の要求の状況は表示しません。` — it never
  showed `停止要求を受け付けました`. The projected state became
  `COMPLETED` with `SUCCESS_ACCEPT` at 05:39:36; no `CANCELLED` state was
  observed.
- Interpretation: the command path was exercised, but the authorization
  refusal arrived as **403** (old-grant/principal mismatch after the bounded
  reauthorization — the same closed behavior documented by the RT3 matrix), not
  as a 401 on the command itself; the 401s occurred on the observation/refresh
  channel. The UI did **not** fabricate stop completion and the task was not
  stopped. The exact 401-on-command case remains unobserved, which is why this
  task is PARTIAL rather than PASS.

The harness retried the Brief once after an earlier run's Brief surface closed
before field entry (recorded in `steps.brief.attempts`); the recorded attempt
submitted normally. A failed entry submits nothing, so no duplicate task was
created.

## 3 — Provider task survival across a Runtime restart (NOT_RUN for same-process reconnect; recovery measured separately)

Primary run directory: `target/acc1-native/20261110T051927-runtime-restart`.
Detail: [record-runtime-restart.json](record-runtime-restart.json); follow-up:
[record-restart-followup.json](record-restart-followup.json).

- A real generation was started through the native Brief
  (`task-a6d96a1678e845919d88c1406bbe15c5`, `ACC1-NATIVE-RESTART
  20261110-051927 production still`). Its ComfyUI submission receipt
  `028e359b-1edb-4893-a11f-e37074ad3c29` was already durable in
  `worker_provider_submission_receipts` before the restart; `/queue` showed the
  prompt running.
- At 05:21:01 the isolated Runtime was stopped (clean, **10.81 s**, exit 0; its
  discovery descriptor removed, port freed). It was restarted at 05:21:24 on
  the **same DB copy** under epoch `597675ae1ba04c2d9bc78234ff7b445b`; the old
  epoch was `62c1960a26624e21b9fa2340cecd7aaa`, and the new discovery descriptor
  advertised the new epoch/pid.
- The same Desktop process did **not** reconnect: it rendered
  `情報源なし / データ非公開` with `Desktop内 · 観測情報を利用できません — 再接続する`;
  no automatic reconnect was observed in 45 s, and invoking `再接続する` at
  05:22:13 did not reopen the observation (no Desktop credential/observation
  requests followed in the serving log). The explicit recovery action only
  re-subscribes locally in this state; it did not fabricate a credential or a
  projection.
- Durable state: the task row is still `ACTIVE`, the provider receipt is
  durable, and **no outcome** exists. ComfyUI history for the prompt later
  reported `status = success` with **1** output image, but the Runtime never
  claimed it; the drain performed **no automatic resubmission** (the restart
  reconciliation maps this window to a manual/ambiguous disposition rather than
  replaying the provider attempt).
- **Fresh-session recovery (`restart-followup`, PASS):** a new isolated Runtime
  on a copy of the same DB (epoch `83714d4fb6fd4292b65e195a626ee0bb`) and a
  fresh Desktop session with a fresh credential connected by discovery, showed
  the same task as `ACTIVE` / `RUNNING`, with the durable receipt and no
  artifact/outcome. This proves the durable row and the recovery path, not
  generation survival.

Loss is therefore explicit: provider work completed after the restart but its
result was never handed off; the in-flight generation did not reach a projected
terminal state. No survival is claimed.

## Leak scan

- The harness scans the whole evidence directory after every invocation and
  records one stage per invocation in `session.json → leak_checks` (7 stages).
  Final stage: **300 files checked, 3 per-run secrets checked, 0 findings**.
- An additional generic scan (`leak-scan.json`) checked every evidence file for
  `Bearer …`, `mh1.…` media handles, 48-hex secret-like values and
  `ORCHESTRATOR_AUTH_SECRET` / `COS_DESKTOP_RUNTIME_ISSUANCE_SECRET`
  assignments: **0 findings**.
- Records are redacted before writing; runtime/Desktop log copies are sanitized
  with the per-run secrets and media-handle patterns; screenshots/UIA dumps
  contain no credentials. The per-run secrets are never persisted.

## Cleanup

Detail: [cleanup.json](cleanup.json).

- ComfyUI was started for the campaign (03:33:33, first `/system_stats` 200;
  log `Starting server` at 03:36:26), stopped after the first pack (04:59:17),
  restarted for the pinned-binary campaign, and stopped again at **05:48:17**
  (`taskkill /PID 13256 /T /F`). Final state: **no 8188 listener, no ComfyUI
  `main.py` process**. Ollama is untouched and healthy (`/api/tags` 200, 6
  models).
- Final checks at 05:48:15: ports **8132 closed, 8133 closed, 8188 closed**; no
  `cos-desktop.exe`, no `isolated_runtime.py`, no `comfy_worker_process`
  process; the global discovery descriptor `%LOCALAPPDATA%\COS\runtime.json`
  is **absent**; every campaign run's redirected `runtime-discovery.json` is
  **absent**; `C:/tmp/cos-desktop-native.lock` is **absent**.
- Every run closed its Desktop with `WM_CLOSE` (exit 0) and its Runtime cleanly
  (`exit 0`); credential-store cleanup used the composed Windows Credential
  Manager target and recorded `deleted`/`not_found` honestly. Raw logs and the
  isolated DB copies stay under `target/acc1-native/…` (gitignored).
- The pinned build worktree/target are temporary campaign material; the binary
  hash and build command are pinned in [provenance.json](provenance.json).

## Files

- `session.json` — merged campaign record (verdicts, provenance, per-run
  cleanup, leak-scan stages); `record.json` — all four run records.
- `record-golden.json`, `record-stop-revoked.json`,
  `record-runtime-restart.json`, `record-restart-followup.json` — the four
  measured runs.
- `artifact.png` — canonical bytes fetched from the isolated CAS for the
  golden generation (digest-matching).
- `provenance.json` — pinned binary source/commit/tree/build and the concurrent
  `dev` advance; `cleanup.json` — final listener/process/discovery proof;
  `leak-scan.json` — generic pattern scan.
- `logs/<run>/` — sanitized Runtime/Desktop logs; `uia/<run>/` — UIA dumps;
  `shots/<run>/` — PrintWindow screenshots with SHA-256 in the records.
- `manifest.json` — size/SHA-256 of every committed file except itself.

## Reproduction

```text
# 1. clean pinned build of the recorded pre-state (once)
git worktree add --detach target/acc1-round2-head eb29f62
cd target/acc1-round2-head
CARGO_TARGET_DIR=C:/dev/cos-desktop/target/acc1-round2-head-target \
  cargo build --locked --release -p cos-desktop

# 2. generation plane (documented flags), then verify /system_stats 200
powershell -ExecutionPolicy Bypass -File C:/AI/orchestrator/scripts/start_services.ps1

# 3. campaign (golden + revoked STOP + restart; then the fresh-session follow-up)
C:/AI/orchestrator/.venv/Scripts/python.exe -u \
  apps/desktop/devtools/runtime_integration/run_acc1_round2.py \
  --scenario golden --scenario stop-revoked --scenario runtime-restart \
  --desktop-exe C:/dev/cos-desktop/target/acc1-round2-head-target/release/cos-desktop.exe \
  --no-build --evidence-dir docs/development/evidence-published/acc1-round2-latest

C:/AI/orchestrator/.venv/Scripts/python.exe -u \
  apps/desktop/devtools/runtime_integration/run_acc1_round2.py \
  --scenario restart-followup \
  --source-run target/acc1-native/<restart run> --task-ref <ACTIVE task> \
  --desktop-exe C:/dev/cos-desktop/target/acc1-round2-head-target/release/cos-desktop.exe \
  --no-build --evidence-dir docs/development/evidence-published/acc1-round2-latest
```

The harness imports the committed `run_acc1_native.py` by default. During this
campaign it loaded a pinned copy of that file (HEAD `eb29f62`) from
`target/acc1-round2-pinned/` because the shared working tree was being edited
concurrently; the pinned copy is not part of the commit.

## What is NOT measured / residuals

- **The exact 401-on-command case.** The command request was answered `200`
  then `403` after the bounded reauthorization; 401s were observed on the
  observation/refresh channel. A command attempt that itself receives 401
  remains unobserved.
- **Same-process Desktop reconnect after a Runtime restart.** It did not
  happen automatically, and `再接続する` did not recover it; recovery required a
  fresh Desktop session (measured). Whether a longer wait or a different
  trigger would reconnect the same process is not measured.
- **Provider task survival to terminal.** The generation did not survive; the
  durable receipt/row did, and the provider output was never handed off.
- **Brief accepted-receipt paint.** It was not painted in the golden run; the
  task was resolved from the live projection. The earlier campaign observed the
  receipt paint, so this is a run-level observation, not a regression claim.
- Second recipe / multi-generation breadth, Start/Next Iteration (DD-25),
  screen-reader/IME/High Contrast/DPI acceptance and original-resolution (1:1)
  display remain outside this wave. Product acceptance and ACC2 remain the
  Product owner's.
