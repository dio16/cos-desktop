# MP-ACC1 round-2 provider survival — combined fix verification (2026-11-10)

End-to-end verification of the combined repair for the round-2 provider survival
defect measured in
[`acc1-round2-latest`](../acc1-round2-latest/README.md):

- Runtime `068c18ca` (`7d96fdee` after the 08:09 amend; product-code identical):
  resume a durable `ACTIVE` provider attempt after a serving restart.
- Desktop `f90f0c5` (+ measurement commit `cf235cb`): recover a lost Runtime
  session in the **same** Desktop process.
- Compare `3e381ca`: acquire each Compare side through the media boundary.
  The measured surface here is the underlying RC-13 media path (artifact
  descriptor → handle → blob → digest), which is what Compare consumes.

## Verdict

**PASS** for the combined fix: the same Desktop process auto-recovered through a
clean Runtime stop/restart under a new epoch with no user action, and the Runtime
resumed the durable ComfyUI attempt to a projected `COMPLETED` /
`SUCCESS_ACCEPT` with a servable, digest-matching artifact and **no** duplicate
provider submission.

Two of three native runs reached the full chain (run1, run3). The third (run2)
did **not** reach a terminal state because ComfyUI itself failed the generation
(`comfy-aimdo` host buffer read, Windows error 1450) *before* the Runtime was
stopped — there was no in-flight provider work to survive. That run is recorded
as `PARTIAL` and is not a counterexample to the survival claim; it also exposed
one Runtime observability finding (F1 below). No fabricated survival claim is
made anywhere in this record.

| Run | Harness | Verdict | Why |
|---|---|---|---|
| run1 | `run_acc1_round2.py` (round-2 assertions) | **PASS** | mid-generation restart, auto reconnect, `COMPLETED`/`SUCCESS_ACCEPT`, artifact `5e9023e7…` verified in a split measurement |
| run2 | `run_acc1_round2.py` (+ screenshots/terminal instrumentation) | **PARTIAL** | ComfyUI execution error before the stop; task left `ACTIVE`, no outcome, no resubmit; Desktop still auto-reconnected |
| run3 | `run_acc1_round2.py` (+ instrumentation, provider reset) | **PASS** | same full chain as run1, plus the terminal state rendered in the same process (UIA bound to this task) and artifact `22c17612…` verified |

## Method

For each run, the existing round-2 harness started the release Desktop binary
(built from a clean detached worktree of `cf235cb`, see
[`provenance.json`](provenance.json)) with **discovery only** (redirected
observer discovery descriptor, no endpoint/token environment), one isolated
Runtime on `127.0.0.1:8132` (fresh read-only copy of the production database,
isolated config/CAS/work roots, per-run `ORCHESTRATOR_AUTH_SECRET`, media-handle
secret and issuance secret), and one real Brief generation through the native
surface. The harness stopped the Runtime with its documented graceful stop once
the durable ComfyUI submission receipt existed, restarted it on the same DB copy
under a new epoch, and measured the same Desktop process, the projected task,
the durable rows, ComfyUI history/queue, the UIA surface and PrintWindow images.

`verify_restart_survival.py` (this campaign) then re-used the post-restart DB
copy/CAS of run1 and run3 in a separate short-lived Runtime epoch to fetch the
task's own artifact through the protocol and compare bytes/digests; it is
explicitly a split measurement, not part of the same-process claim.

## (a) Same-process Desktop recovery — PASS (run1, run2, run3)

One Desktop process per run (`--- launch 1 ---` in the Desktop log), no relaunch
and no `再接続する` invocation (`manual_reconnect_required = false`; the action was
never pressed). Desktop log, run3 (run1 identical):

```text
Runtime discovered: server_identity=cos-runtime-orchestrator epoch=85fa9fed… pid=42120
Runtime rediscovery unavailable: runtime discovery descriptor was not published   (×6 during the outage)
Runtime reconnect unavailable: the Runtime discovery descriptor is unavailable    (×6 during the outage)
Runtime rediscovered: server_identity=cos-runtime-orchestrator epoch=d359d1d5… pid=44200
Runtime credential prepared for the reconnect attempt
Runtime reconnect installed; the observation reopens without a relaunch
Runtime credential reauthorized after rejection; the session reopens a fresh observation
```

The withheld state is honest and explicit before the restart
(`shots/run3/restart-withheld.png`):

```text
情報源なし / データ非公開
タスクの状態: 利用できません — 許可情報を取得できません
成果物の利用状態: 利用できません — 許可情報を取得できません
Desktop内 · 観測情報を利用できません — 再接続する
```

After the restart the same process shows the observed projection again
(`shots/run3/restart-reconnected.png`, `restart-terminal.png`):

```text
task-f3b3498669a44cfaa9650d18d2d2362d — ACC1-NATIVE-RESTART 20261110-090427 production still
タスクの状態: 完了（COMPLETED）
成果物の利用状態: 取得可能（AVAILABLE）（1件）
```

Epochs (DB `cos_desktop_observation_epoch`, read from the isolated copy):

| Run | before | after |
|---|---|---|
| run1 | `97263b592d3344428b033c084cfb00df` | `05a4b5d699a344f6a1233c4d9036be48` |
| run2 | `020c3b2be3734d82a3b6aba1c17636ab` | `666734eea22645e4adee36ef72db4605` |
| run3 | `85fa9fed8c3d4428bd8eecd6647b1f3d` | `d359d1d5fd554622a51974ef8f64e6a3` |

## (b) Runtime recovery of the durable ACTIVE task — PASS (run1, run3)

Both runs stopped the Runtime while `/queue` listed the durable prompt as
running (`history_present: false`, `queue_running: true`), restarted it under
the new epoch, and the drain resolved the same receipt and resumed the same
attempt. No new `/prompt` was issued and the artifact was produced from the
existing ComfyUI history.

| Field | run1 | run3 |
|---|---|---|
| Task | `task-bf345805d5f243c1b715dbe29a24097f` | `task-f3b3498669a44cfaa9650d18d2d2362d` |
| Durable receipt (before stop) | `38` / prompt `431ab9be-f0f8-40c5-8964-7621b0272b1f` | `38` / prompt `44813e64-ff14-4943-bbb4-b154952d37ec` |
| Runtime stop | exit 0, 10.59 s, port freed, descriptor removed | exit 0, 10.60 s, port freed, descriptor removed |
| Runtime restart | ready, epoch `05a4b5d6…` | ready, epoch `d359d1d5…` |
| Transition | `ACTIVE` → `COMPLETED` (08:36:59) | `ACTIVE` → `COMPLETED` (09:06:18) |
| Outcome | `COMPLETED`, `stop_reason = SUCCESS_ACCEPT` | `COMPLETED`, `stop_reason = SUCCESS_ACCEPT` |
| Outcome canonical digest | `sha256:9823a5e2…0f13` | `sha256:85e0d1b7…765a` |
| ComfyUI history after restart | `success`, 1 output image | `success`, 1 output image |
| Projection | `task_status = COMPLETED` | `task_status = COMPLETED` |

### Servable artifact and digest

`verify_restart_survival.py` fetched each task's own artifact (resolved through
the projected `OUTCOME.selected_refs`, not "the first available artifact")
through `GET /api/cos/media/{digest}` → handle → blob:

| Run | Evidence file | Bytes | SHA-256 (blob content digest) | DB blob length | PNG |
|---|---|---|---|---|---|
| run1 | [`artifact.png`](artifact.png) | 1 559 315 | `sha256:5e9023e730d3bd49184cfdab123d00b86478f57327b1eeb33edcb22eab424744` | 1 559 315 | 1024×1024 |
| run3 | [`artifact-run3.png`](artifact-run3.png) | 1 510 802 | `sha256:22c17612291e5762b0e45430d71bdebd2057ca9bc7c7a4cc373027503ae8c087` | 1 510 802 | 1024×1024 |

Fetched bytes equal the projected artifact digest and the durable
`artifact_blobs.content_digest` / `length` in both runs
(`digest_matches_projection: true`). The UIA also reported
`成果物の利用状態: 取得可能（AVAILABLE）（1件）` after recovery.

## (c) No duplicate provider submission — PASS (all runs)

Measured from durable identity before/after the restart, not from a terminal
verdict:

| Run | Receipt ids | ComfyUI prompt ids | Attempt ids | Runs | Intents | Verdict |
|---|---|---|---|---|---|---|
| run1 | `[38]` → `[38]` | same `431ab9be…` | `attempt-…-i0` | 1 → 1 | 1 | no duplicate |
| run2 | `[38]` → `[38]` | same `98c48660…` | `attempt-…-i0` | 1 → 1 | 1 | no duplicate |
| run3 | `[38]` → `[38]` | same `44813e64…` | `attempt-…-i0` | 1 → 1 | 1 | no duplicate |

No `-rN` attempt suffix appears, no second receipt/run/intent row exists, and
no resubmit marker appears in the serving log. (The Runtime does not log
outbound ComfyUI requests, so the `POST /prompt` count in the serving log is 0
for every run and is not used as evidence by itself.)

**Harness correction:** the round-2 scenario asserted
`no_automatic_resubmission = not generation_terminal`. After the Runtime repair
that key inverts whenever survival succeeds, so it can no longer mean "no
duplicate submission". This campaign replaced it with
`provider_receipt_identity_unchanged` / `no_duplicate_provider_submission` and
kept run1's record unchanged (it was produced with the old key); run3 is the
record produced by the corrected assertion.

## (d) Unexpected resyncs / errors — 0

Per restart window the serving log contains exactly the expected bounded
reauthentication and nothing else:

```text
GET  /api/cos/desktop/server                   200   (discovery descriptor re-read)
POST /api/cos/desktop/observation              401   (old credential unknown to the new process)
POST /api/cos/desktop/credentials/refresh      401   (refresh material also unknown)
POST /api/cos/desktop/credentials              200   (one replacement, bounded preparation)
POST /api/cos/desktop/observation              200   (fresh observation, new epoch)
```

| Run | epoch 1 | epoch 2 | 409 | 403 | 5xx |
|---|---|---|---|---|---|
| run1 | 151 × 200 | 65 × 200, **2 × 401** | 0 | 0 | 0 |
| run3 | 143 × 200 | 63 × 200, **2 × 401** | 0 | 0 | 0 |
| run2 | 200 only | 509 × 200, **2 × 401** | 0 | 0 | 0 |

The harness client recorded `resyncs: []` and `trace_errors: []`; the Runtime
logged no `database is locked` and no drain error lines. During the outage the
Desktop retried discovery/reconnect on its bounded schedule (6 pairs) and did
not fabricate a projection.

## run2 — provider-side generation failure (recorded, not a survival counterexample)

- The Brief generation for `task-d337e917a2d84a6399ea759bb5a39108` was submitted
  (receipt `38`, prompt `98c48660-8301-4885-bdf4-a937c0625d37` at 08:52:41) and
  ComfyUI failed it 6.82 s later while loading the KSampler weights:
  `aimdo: xfer_file_transfer_at: GetOverlappedResult failed error=1450`
  (Windows `ERROR_NO_SYSTEM_RESOURCES`) →
  `hostbuf_file_reader_read failed` → `history status = error`, 0 outputs.
- The error predates the Runtime stop (08:52:57), so there was no live provider
  attempt to survive. After the restart the task stayed `ACTIVE` with no
  outcome, no resubmit, unchanged receipt identity, and the Desktop still
  auto-reconnected and showed the withheld→recovered surface.
- The condition was environmental: a ComfyUI restart with the same documented
  flags reset it, and run3 then completed normally. Raw evidence:
  [`logs/run2-provider-error/`](logs/run2-provider-error/) and
  [`record-runtime-restart-run2.json`](record-runtime-restart-run2.json).

## Findings

- **F1 (ACTIONABLE NON-BLOCKING, Runtime, outside Desktop product):** the
  restart-recovery contract says an errored provider history records an
  *explicit issue*, but `run_cos_drain_once` only returns that issue in an
  in-memory list; `_cos_drain_loop` neither logs nor persists it. In run2 the
  provider-error task stayed `ACTIVE` with no outcome, no log line, no JSON row
  and no projection reason for the whole 90 s window. Product code was not
  changed by this campaign; reported for the Runtime owner.
- **F2 (ENVIRONMENT RESIDUAL, ComfyUI/comfy-aimdo 0.6.0):** the run2
  `hostbuf_file_reader_read` failure described above.
- **F3 (HARNESS, corrected here):** the round-2 duplicate-submission assertion
  `not generation_terminal` is invalid after the repair; replaced by durable
  receipt-identity measurement (see (c)).

## Leak scan

Every invocation ran the harness leak scan; the final aggregate scan covers all
text files in this directory for `Bearer …`, `mh1.…` media handles and the
per-run secrets, and additionally the credential-assignment patterns
`ORCHESTRATOR_AUTH_SECRET`, `COS_DESKTOP_RUNTIME_ISSUANCE_SECRET` and
`X-Auth-Secret` values. Result: see [`leak-scan.json`](leak-scan.json) —
**0 findings**. Records are redacted before writing; log copies are sanitized
with the per-run secrets and media-handle patterns. The per-run secrets are
never persisted.

## Cleanup

Default state before the campaign: ComfyUI stopped, no 813x listener, no
Desktop/Runtime process, no discovery descriptor. That state was restored. See
[`cleanup.json`](cleanup.json) for the exact final checks (ports, processes,
discovery descriptor, native lock) and the ComfyUI start/stop times.

## Files

- `session.json` — run1's own campaign record (harness output, verdict `PASS`,
  leak-check stages). `session-followup.json` — the aggregate for run1/run2/run3
  with epochs, outcomes, artifact digests, HTTP accounting and findings.
- `record-runtime-restart.json` — run1 (round-2 assertions);
  `record-runtime-restart-run2.json` — run2 (instrumented, provider failure);
  `record-runtime-restart-run3.json` — run3 (instrumented, PASS).
- `record-survival-verify.json`, `record-survival-verify-run3.json` — split
  artifact/servability/identity measurements for run1 and run3.
- `artifact.png` (run1), `artifact-run3.png` (run3) — fetched canonical bytes.
- `logs/runtime-restart/`, `logs/run2/`, `logs/run3/` — sanitized
  Runtime/Desktop logs; `logs/run2-provider-error/` — the ComfyUI error excerpt
  and the prompt's history status.
- `uia/runtime-restart/`, `uia/run2/`, `uia/run3/` — UIA probe outputs;
  `shots/runtime-restart/`, `shots/run2/`, `shots/run3/` — PrintWindow images
  (SHA-256 recorded in the records).
- `provenance.json` — Desktop/Runtime commits, clean build source, binary
  size/SHA-256, generation plane, harness hashes. `cleanup.json` — final state.
  `leak-scan.json` — final scan. `manifest.json` — size/SHA-256 of every
  committed file except itself.

## Reproduction

```text
# 1. clean release build of the recorded source state (dedicated target dir)
git worktree add --detach target/acc1-restart-survival-head cf235cb
cd target/acc1-restart-survival-head
CARGO_TARGET_DIR=C:/dev/cos-desktop/target/acc1-restart-survival-head-target \
  cargo build --locked --release -p cos-desktop --bin cos-desktop

# 2. generation plane (documented 8 GB flags), then verify /system_stats 200
powershell -ExecutionPolicy Bypass -File C:/AI/orchestrator/scripts/start_services.ps1

# 3. native restart scenario (one run per evidence directory)
C:/AI/orchestrator/.venv/Scripts/python.exe -u \
  apps/desktop/devtools/runtime_integration/run_acc1_round2.py \
  --scenario runtime-restart \
  --desktop-exe C:/dev/cos-desktop/target/acc1-restart-survival-head-target/release/cos-desktop.exe \
  --no-build --evidence-dir docs/development/evidence-published/acc1-restart-survival-latest

# 4. post-restart artifact/servability/identity split measurement
C:/AI/orchestrator/.venv/Scripts/python.exe -u \
  apps/desktop/devtools/runtime_integration/verify_restart_survival.py \
  --source-run target/acc1-native/<run> --task-ref task-... \
  --restart-record <record-runtime-restart.json> \
  --evidence-dir docs/development/evidence-published/acc1-restart-survival-latest
```

Raw per-run database copies and logs stay under the gitignored
`C:/dev/cos-desktop/target/acc1-native/`; the ComfyUI log copy stays at
`C:/dev/cos-desktop/target/acc1-restart-survival-comfyui.log`.
