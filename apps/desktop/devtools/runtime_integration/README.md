# Live Desktop <-> Runtime integration harness (RT2/RT3/ACC1 baseline)

Repeatable, fixture-free integration run between the release `cos-desktop` binary and a
local Runtime started from `C:/AI/orchestrator`.

```powershell
# Run the harness twice. It builds the release Desktop itself (so the binary and the
# recorded source state cannot drift), then writes the evidence under
# docs/development/evidence-published/rt-integration-latest/.
C:/AI/orchestrator/.venv/Scripts/python.exe `
    apps/desktop/devtools/runtime_integration/run_integration.py --runs 2

# Reuse an existing release binary instead of rebuilding:
#   ... run_integration.py --runs 2 --no-build
```

The harness holds `C:/tmp/cos-desktop-native.lock` for the whole run (owner/pid/at in
`OWNER.txt`, removed in a `finally`).  Only processes it started are terminated.

## What one run does

| Phase | Action | Evidence |
|---|---|---|
| 1 | Copy `C:/AI/orchestrator/db/generation.sqlite` through a read-only SQLite backup handle into `target/rt-integration/<label>/`, write an isolated config YAML, start `orchestrator_app.api` on `127.0.0.1:8124` with a per-run `ORCHESTRATOR_AUTH_SECRET` (memory only). | `runtime.log.sanitized.txt`, db-copy SHA-256 |
| 2 | `POST /api/cos/desktop/credentials` (X-Auth-Secret) for the Desktop and for the harness; derive the expected TASK projection from `POST /api/cos/desktop/observation`. | credential principal/expiry, task count, expected title/status |
| 3 | Launch the release binary with `COS_DESKTOP_RUNTIME_ENDPOINT`/`COS_DESKTOP_RUNTIME_TOKEN` in the child environment and a fresh LOCALAPPDATA under the workspace target; no fixture flag. | `uia-connected.json`, `connected.png` |
| 4 | Stop the Runtime; wait for the explicit withheld state. | `uia-withheld.json`, `withheld.png` |
| 5 | Restart the Runtime (new process/epoch); verify the Desktop stays withheld without a fresh credential. | `uia-restart-withheld.json`, `restart-withheld.png` |
| 6 | Close the Desktop with `WM_CLOSE`; require the observation-shutdown and preferences-save confirmations and no surviving process. | `app-connect.stderr.sanitized.txt` |
| 7 | Issue a fresh credential, relaunch, require the real projection again, close cleanly. | `uia-recovered.json`, `recovered.png`, `app-recovery.stderr.sanitized.txt` |

`session.json` aggregates both runs with per-scenario verdicts, the determinism comparison,
the binary/Runtime/database hashes, and any findings.  Tokens and the issuance secret are
never written to a file; log excerpts are redacted and the harness fails if token material
is found anywhere under the evidence directory.

## RT3 full matrix harness (ports 8135/8136)

`run_rt3_matrix.py` extends the two-run RT3/RT8 baseline (`run_rt3_rt8.py`) to
the full DC-MP-RT3 reconnect/resync/epoch matrix. It is stdlib-only, starts one
isolated Runtime on `127.0.0.1:8135` and (after the epoch/port switch) one on
`127.0.0.1:8136`, copies the production database read-only once, and drives the
release Desktop binary with discovery only (no endpoint/token environment, no
fixture flag):

```powershell
python apps/desktop/devtools/runtime_integration/run_rt3_matrix.py --long-seconds 180
# evidence: docs/development/evidence-published/rt3-matrix-latest/
# reduction for debugging: --quick (30 s idle window)
```

Scenarios (each records exact statuses, counts, epochs and cursors):

1. `gap_closed_resync` — a real TASK status change whose journal history is
   purged atomically in the isolated copy forces `409 RESYNC_REQUIRED`; the
   client performs one closed resync through `GET /api/cos/desktop/snapshot`
   (new cursor) and the missed projection is recovered; no retry loop.
2. `cursor_overflow_resync` — the journal is advanced past the 10,000-row
   retention bound, forcing the bounded overflow answer; the Runtime prunes and
   the client recovers through the fresh snapshot.
3. `desktop_clean_restart` — clean close mid-observation, relaunch; the
   relaunch opens a fresh observation with a cursor never used before.
4. `desktop_forced_restart` — forced process-tree close, relaunch; same checks.
5. `runtime_epoch_change_desktop_restart` — the Runtime is stopped and
   restarted on the alternate port while the Desktop is closed; the relaunch
   rediscovers the new origin/epoch and opens a fresh observation.
6. `epoch_change_under_load` — a live TASK projection stream is applied, the
   Runtime restarts under the same process, the withheld state is observed, and
   the same process recovers (bounded reauthentication + fresh observation, no
   relaunch).
7. `long_observation_idle` — a 180 s idle session; request continuity is
   measured without mid-window UIA enumeration (a PowerShell UIA tree walk can
   stall the UI-driven poll loop), plus start/end UI state, one cursor, no
   401/409/credential/command request and no duplicate task row.
8. `auth_change_bounded_reauth` — the credential registry is pushed past its
   bound three times through the supported issuance API; each push revokes the
   Desktop credential and the client makes exactly one replacement, retries the
   read (fails closed with `403 PRINCIPAL_MISMATCH` for the new principal's old
   grant), then reopens a fresh observation. Three replacements total.
9. `auth_change_exclusion_honest` — a fourth push exhausts the per-process
   replacement budget: the 401 is passed through, no credential request follows,
   and the explicit `情報源なし / データ非公開` / `データは非公開 — 現在の許可がありません`
   state is rendered. The strip's `再接続する` action is invoked and does not
   fabricate a recovery.
10. `post_exclusion_relaunch_recovery` — a fresh Desktop session reauthorizes
    from the operator issuance secret and recovers the projection.

Isolation/cleanup notes:

- The automatic-replacement budget is per **process runtime binding**, not per
  observation session: the epoch-change recovery consumes one of three, so the
  campaign records an explicit relaunch before the auth matrix (relaunch itself
  consumes no replacement because the stored credential is valid).
- Credential-store cleanup uses the exact Windows Credential Manager target
  `desktop-observation.cos-desktop/observation/<sha256(origin)[:32]>` (the keyring
  `{user}.{service}` composition). `cmdkey /delete` matches the whole target, so
  deleting only the bare service string never removes the stored entry and a stale
  credential then causes an avoidable startup 401 in a later run. The shared
  `run_integration.cleanup_credential_store` now attempts the composed target first
  and the bare one second and records each attempt's exit code and outcome
  (`deleted` / `not_found` / `failed`), so evidence can distinguish a real deletion
  from "already absent"; `run_acc1_native.py` and `run_rt3_rt8.py` inherit it and
  `run_media_integration.py` carries the same fixed copy. `run_rt3_matrix.py` keeps
  its local composed-target logic. `cleanup_observation_credentials.py` reproduces
  the cleanup for explicit origins (with a throwaway composed-entry round-trip that
  proves the composed delete), and `run_integration.py --credential-self-test`
  checks the target construction offline.
- Both Runtime instances are stopped through the isolated launcher lifespan, the
  evidence directories are leak-scanned, and the ports must be free at the end.
  Raw logs and the isolated database copy stay under `target/rt3-matrix/run1/`
  (documented in the evidence README).

## MP-ACC1 round-2 survival campaign (restart recovery + same-process reconnect)

`run_acc1_round2.py --scenario runtime-restart` is the scenario used to measure
the round-2 provider survival repair (Runtime resume of a durable `ACTIVE`
attempt after a serving restart, Desktop same-process recovery under a new
epoch). Two harness details changed for that campaign:

- the duplicate-provider-submission assertion is measured from durable receipt
  identity (receipt ids, ComfyUI prompt ids, attempt ids and run count
  before/after the restart). The round-2 key `no_automatic_resubmission` was
  `not generation_terminal`, which inverts once survival succeeds, and is no
  longer used as a duplicate claim;
- the post-terminal UIA check binds the strip status to this task's inspector
  title, so another task's `COMPLETED` row cannot be read as this task's
  terminal state. Every UIA snapshot also keeps its PrintWindow image.

`verify_restart_survival.py` is the split measurement that follows a run: it
re-uses the run's isolated DB copy/CAS in a separate short-lived Runtime epoch,
resolves the task's own artifact through the projected `OUTCOME.selected_refs`,
fetches it through the RC-13 media descriptor/handle/blob path, compares the
SHA-256 with the projection and the `artifact_blobs` row, and re-checks that the
provider receipt/prompt/attempt/run identity did not change:

```powershell
C:/AI/orchestrator/.venv/Scripts/python.exe -u `
  apps/desktop/devtools/runtime_integration/verify_restart_survival.py `
  --source-run target/acc1-native/<run> --task-ref task-... `
  --restart-record docs/development/evidence-published/<dir>/record-runtime-restart.json `
  --evidence-dir docs/development/evidence-published/<dir>
```

Evidence for the measured campaign:
`docs/development/evidence-published/acc1-restart-survival-latest/`.

## Boundaries

- Applies no product changes; it only starts the existing Runtime and Desktop binaries.
- `PrintWindow` (with `PW_RENDERFULLCONTENT`) is used for screenshots; the window is never
  foregrounded and `CopyFromScreen` is never used.
- UIA is read for names/ids only; no screen-reader, IME, High Contrast or DPI claim.
- The production database is only read; every Runtime run uses its own copy.
- A Runtime restart loses the in-memory credential registry, so the recorded expectation is
  withheld-until-relaunch; a recovered projection without a fresh credential would be
  recorded as a deviation, not worked around.
