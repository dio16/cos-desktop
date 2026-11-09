# MP-ACC1 native golden workflow and bounded failure injections (2026-11-09)

This directory is a **bounded acceptance attempt** for [DC-MP-ACC1](../../../architecture/DESKTOP_DESIGN_CONTRACTS.md#dc-mp-acc1),
run through the **native release Desktop window** (UIA + `PrintWindow`) against an
isolated Runtime. It is evidence-only test-harness work, not product acceptance.
The earlier protocol-level records ([`acc1-latest`](../acc1-latest/session.json),
[`acc1-followup-20261109`](../acc1-followup-20261109/FOLLOWUP.md)) are unchanged.

- **Verdict:** `PARTIAL` (overall). Per run: golden `PARTIAL`, stop `NOT_RUN`,
  revoke `PASS`, missing-media `NOT_RUN`.
- **Started / finished:** 2026-11-09T11:04:42+0900 / 2026-11-09T11:50:18+0900
  (four runs; the golden run retained here started 11:44:16).
- **Desktop:** `C:/dev/cos-desktop`, head `5eeda7a6cf8e44c63a11dc38359d8e3943d60e7f`
  when the harness built and launched the binary. `cargo build --locked --release -p cos-desktop`
  exited 0 and reported the release profile up to date; the binary is
  `target/release/cos-desktop.exe`, **21,109,760 B**, sha256
  `sha256:5acb4af8b331192b2856692fd04cc527f67107a915d270bedd7f5320c5a2faf7`.
  Only test-harness/evidence files changed; no Desktop product code changed.
- **Runtime:** `C:/AI/orchestrator`, head `d6e0ba202f4068860c85208f9ea6e3b3b681bfcb`
  (the observation-epoch ownership fix). The seven pre-existing unrelated Runtime
  dirty entries were never staged or modified.
- **Harness:** `apps/desktop/devtools/runtime_integration/run_acc1_native.py` plus the
  test-only launcher `isolated_runtime_control.py`. Ports **8132** (golden) and
  **8133** (injections); a fresh DB copy per run
  (`11,493,376 B`, sha256 `f8771100…bfb3e3d`), copied presets, own config/logs/CAS/outputs,
  per-run `ORCHESTRATOR_AUTH_SECRET` and `ORCHESTRATOR_COS_MEDIA_HANDLE_SECRET`, and a
  redirected RT8 discovery descriptor. The Desktop was launched with **no**
  `COS_DESKTOP_RUNTIME_ENDPOINT` and **no** `COS_DESKTOP_RUNTIME_TOKEN`: only the per-run
  issuance secret and `COS_DESKTOP_RUNTIME_DISCOVERY_PATH`. Tokens and secrets are never
  written to evidence.
- **Generation plane:** direct preflight ComfyUI `0.35.1` `/system_stats` 200 with the
  documented argv and Ollama `/api/tags` 200 with 6 models; Runtime-reported health
  `{"llm": true, "comfy": true}`. ComfyUI was started for this attempt with
  `powershell -ExecutionPolicy Bypass -File C:/AI/orchestrator/scripts/start_services.ps1`
  and stopped afterwards (`cleanup-final.json`); Ollama was already running and was not
  changed.

## What was measured (native window)

| Step | Verdict | Observed |
|---|---|---|
| Isolated Runtime readiness + generation-plane health | **PASS** | `/api/ready` 200; Runtime health `llm=true, comfy=true` after 3 polls; epoch `6d155ac8…` stayed the serving process's epoch. |
| Discovery-only native launch | **PASS** | Window `COS Desktop` (`Zed::Window`, 1296×839); UIA task rail and connection labels rendered without endpoint/token env. |
| Brief UI open | **PASS** | UIA `Invoke` on the “新しいタスクを作成” button opened the `クリエイティブ・ブリーフ` dialog. |
| Goal / Direction entry | **PASS (with method caveat)** | The UIA `Edit` elements were clicked (synthetic `WM_LBUTTONDOWN/UP` inside their UIA bounds) and text was posted as `WM_CHAR`. Readback: 目的 `ACC1-NATIVE-golden 20261109-114416 production still`, 方向性 `cinematic mechanical desktop object, no text`. A pure UIA-focus-then-type path was **not used** (see harness note below). |
| Recipe selection | **PASS (method caveat)** | The catalog recipe row exposes `ControlType.Button` but **no Invoke/Selection pattern**; selection used a synthetic `WM_LBUTTONDOWN/UP` at its UIA bounding rectangle. Submit became enabled afterwards. |
| Brief submit | **UI defect — receipt never painted** | UIA `Invoke` on “ブリーフを提出” was accepted by the Runtime (200), but the **accepted receipt was never painted** (`受付タスク` absent); this is recorded as a UI defect/next-fix, not as submission success, and the task was resolved through the live TASK projection (`task_ref_source = live TASK projection (receipt not painted)`). |
| Golden task → terminal | **PASS** | Task `task-95a19eb4364e4d9d8a444e882469da9f`; live trace `ACTIVE/RUNNING` revision 263 → `COMPLETED/RUNNING` revision 266 with `formal_outcome_ref sha256:995a842e…21d34`; outcome projection `COMPLETED` / `SUCCESS_ACCEPT` (revision 271). `ASSIGNED` was not captured between the 1 s polls. |
| Desktop restart mid-generation | **PASS** | `WM_CLOSE` at `ACTIVE` (pid 53240, exit 0, no force); Runtime stayed ready and the task stayed `ACTIVE`; relaunch (pid 48992) discovered the Runtime, connected and displayed the task (`task_visible_after_relaunch=true`). Same serving epoch throughout; **no** epoch-change refusal and no Runtime restart. |
| Projection continuity | **PARTIAL** | Two recoverable `409 RESYNC_REQUIRED` answers on the harness read:observe client, each followed by a 200 observation reopen and continued state; `trace_errors=0`, `runtime_log_epoch_conflict=false`. |
| Artifact surface / image | **BLOCKED** | The app viewed the task's candidate and the artifact surface showed **`機能の利用可否情報が投影されていません（capability not projected）`**. The observation snapshot carries no `CAPABILITIES` projection (`{TASK 40, ITERATION 38, CANDIDATE 38, ARTIFACT 37, EVALUATION 38, OUTCOME 38, GRAPH 38}`), so the media gate refuses before any descriptor/blob request. This is a fail-closed state, not a fabricated image. |
| Canonical media (protocol) | **PASS** | RC-13 descriptor 200 → handle → blob 200: **1,370,406 B**, computed SHA-256 `sha256:34ecf76910104743ee98cab66dd2851afc95a98360d55396e5da908b5d3a77d5` equals the descriptor digest; PNG **1024×1024**; saved as [artifact.png](artifact.png). The native surface did not display it because of the capability gate above. |
| History / task read (protocol) | **PASS** | Task and history reads returned 200 through the harness client; native History/Compare/Evidence/Flow interaction is `NOT_MEASURED`. |

## Failure injections

| Injection | Verdict | Observed |
|---|---|---|
| (i) STOP_TASK during generation | **NOT_RUN** | The task reached `ACTIVE/RUNNING`; the native stop button rendered **“タスクを停止（現在は操作できません）”** (disabled). No command was sent; the capability projection does not authorize stop. Recorded `stop_button_enabled=false`, `stop_button_disabled=true`; screenshot hash `sha256:9bc21dd0…ae9786`. |
| (ii) Desktop restart mid-generation | **PASS** | See the golden run above: clean close, Runtime kept serving, relaunch resynced and showed the task; the task continued to `COMPLETED`. |
| (iii) Missing media | **NOT_RUN** | The isolated CAS blob was real and digest-verified before removal: `blobs/5e/5e9de2c5…8b1ae8`, **1,369,752 B**, equals the descriptor digest; removed; an immediate protocol fetch returned **404** (`blob_refused`) and no bytes were fabricated. Before any blob request the native artifact surface showed **`機能の利用可否情報が投影されていません（capability not projected）`** for the live task, so the missing-media state is not observable through the native surface. Screenshot hash `sha256:d489b2fa…491fc4`. |
| (iv) Credential revocation during a run | **PASS** | The isolated serving process revoked its in-memory Desktop credentials (`revoked_count=2`, principals `desktop:f9f8896d…`, `desktop:fd30e356…`; token values never recorded). Runtime access log shows `refresh 401` then `credentials 200`; the app log contains `Runtime credential reauthorized after rejection; the session reopens a fresh observation`; one `PRINCIPAL_MISMATCH: credential principal does not own this context` forced a fresh observation (designed observation reset), after which the task continued to `COMPLETED`. No denial state was reached because the issuance secret stayed valid. |

The Desktop token is never exposed by design, so the revocation was performed by the serving
process on its own registry; the app sees the same 401 the product revoke route produces.

## Screenshot / UIA hashes (selected, retained under `shots/` and `uia/`)

| State | File | sha256 |
|---|---|---|
| Connected rail | `shots/golden/connected-main.png` | `sha256:5b81faa6db49e9b67e9b33fddf916a17746b7567ab5e3af74ef38be56098f123` |
| Brief filled | `shots/golden/brief-filled.png` | `sha256:80857b0847a938f5bb65761f28774075155b5febfa838648c29cc0ac7a37d49e` |
| ACTIVE | `shots/golden/state-1-ACTIVE.png` | `sha256:7693d037abe3a19934e7d37091c851bb9d5aeda7cae9630d7ab34467973d2fa9` |
| COMPLETED / terminal | `shots/golden/state-2-COMPLETED.png` | `sha256:0c2a8ba1d77927cd5bff4ecd7d31444fbbb3275b9e707535feb6bc7461200644` |
| Task after relaunch | `shots/golden/task-relaunched.png` | `sha256:5a0f59bca208ec57cfa0e3c65de818d1fe8c9a03a4632e774e0e28de43c1f46f` |
| Artifact surface (capability gate) | `shots/golden/artifact-surface.png` | `sha256:9c5be67c1738ce1027bdd54f58107ae0e4bad32d64a1db6ae3a586ce739b6c01` |
| Revocation, 3 s after | `shots/revoke/revoke-after-3s.png` | `sha256:03c4a4a736dc69089c34b3c982208aebc39be7f1bfeb40877e62fd74b8a22f90` |
| Missing media surface (capability gate) | `shots/missing-media/missing-artifact.png` | `sha256:d489b2fa7a12bade0aa4af2e3ec587787bb9406a721664079fd01f6bf8491fc4` |

Every step in `record-*.json` records the UIA name list (or a bounded excerpt) and its
`names_sha256`, plus the exact window title/class/size; the per-state screenshots are retained
under `shots/` with their `sha256`. The raw per-poll UIA dumps are not retained here (they were
harness polling output; the run work directories under `target/acc1-native/` are local
scratch). Sanitized Runtime/Desktop logs are retained under `logs/`; per-run secrets, media
handles and bearer values are redacted before they are written.

## Leak scan

- The four-run batch scan checked **13** capability/secret values across **238** files; the
  final single-run rescan checked **3** values across **782** files, plus `Bearer …` and
  `mh1.` handle patterns. **No findings** in either scan (`session.json` records the final run;
  this README records both).
- The RC-13 descriptor is retained with its `access_handle`/URL capabilities replaced by
  `<redacted-media-capability>`; no token, context id, cursor or media handle is recorded in
  plaintext.
- The harness's credential-store cleanup deleted only the bare service target
  (`cos-desktop/observation/<hash>`), which never matches the Desktop's keyring-composed
  `desktop-observation.cos-desktop/observation/<hash>` entry: both isolated endpoints
  recorded `cmdkey` exit 1, “Element not found”, so **no stored entry was deleted by the
  run**. The follow-up record ([`cleanup-final-fix.json`](cleanup-final-fix.json)) lists both
  target forms for 8132/8133 as absent **before and after** the fixed helper ran (again
  `Element not found`, i.e. already absent; the Desktop removes its stored credential on
  explicit teardown). The fixed helper deletes the composed target first and proves it with a
  throwaway entry (exit 0, “Credential deleted successfully”).

## Cleanup

`cleanup-final.json` records the final operator-level check after the harness runs:
`8132`, `8133` and `8188` have **no listener**; no `cos-desktop`, `worker-session-runner` or
`isolated_runtime` process remains; ComfyUI (PID 28056) was stopped and the prior stopped
state restored; Ollama was untouched. Each harness run itself exited its Runtime cleanly
(exit 0), verified the port free, and removed its discovery descriptor.

`cleanup-final.json` also carries a correction: the harness's credential-store delete used
only the bare service target and never matched the composed entry, so the earlier
"entry deleted" statement was wrong. The follow-up
([`cleanup-final-fix.json`](cleanup-final-fix.json)) records the actual outcome per target
(composed and bare, exit code plus `deleted`/`not_found`) for both isolated endpoints, and the
shared `run_integration.cleanup_credential_store` now attempts the composed target first so
future runs do not leave a stored entry behind.

## What was NOT measured (`NOT_MEASURED` / `NOT_RUN`)

- **Native artifact display of the image**: blocked by the missing `CAPABILITIES` projection
  described above. The canonical bytes/digest are evidence for media acquisition, not for a
  displayed picture.
- **STOP_TASK** and **missing-media** through the native surface: both are gated by the same
  absent capability projection; the injections are recorded `NOT_RUN` with the exact observed
  reason, never simulated.
- `ASSIGNED` as a native screenshot state (it changed between the 1 s polls; the TASK
  projection timeline is retained instead).
- Native Compare, Evidence detail, Flow and History interaction; multi-recipe breadth;
  Start/Next Iteration (DD-25 command semantics remain blocked); screen-reader/IME/DPI/High
  Contrast claims; corrupt-blob (digest mismatch) variant.
- Product acceptance and ACC2: **remain Product-owner acceptance**, not implied by this
  record.

## Harness note (not part of the retained run evidence)

During harness development, a **UIA-focus-only** text-entry attempt (SetFocus on the Edit,
then `WM_CHAR`, without the synthetic click) aborted the release binary with
`thread caused non-unwinding panic. aborting.` after several characters. The committed
harness therefore clicks the field (which updates the app's shared input selection) before
posting characters. This is a **product robustness defect, not a harness artifact**: a
keyboard-only/screen-reader user can reach the focus+input path, so it is tracked separately
with its own exact reproduction. The click-first path is only a harness workaround for this
evidence run; it is not an acceptance substitute, and the native keyboard/IME/accessibility
path stays `NOT_MEASURED` until the panic is fixed and re-exercised.

## Reproduction

```text
cd C:/dev/cos-desktop
C:/AI/orchestrator/.venv/Scripts/python.exe -B ^
  apps/desktop/devtools/runtime_integration/run_acc1_native.py ^
  --scenario golden --scenario stop --scenario revoke --scenario missing-media ^
  --evidence-dir docs/development/evidence-published/acc1-native-latest
```

Requires ComfyUI on `127.0.0.1:8188` and Ollama on `127.0.0.1:11434`, ports 8132/8133 free,
`C:/AI/orchestrator/.venv`, and the built release `cos-desktop.exe`. The harness starts the
isolated Runtime, launches the release binary with discovery only, and stops everything it
owns; ComfyUI must be started/stopped by the operator as documented above.
