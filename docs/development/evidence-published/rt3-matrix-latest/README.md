# RT3 full reconnect/resync/epoch matrix — live evidence (2026-11-09)

Scope: the one-run matrix campaign in `session.json` / `run1/` that extends the
two-run RT3/RT8 baseline ([rt3-rt8-latest](../rt3-rt8-latest/README.md)) to every
DC-MP-RT3 scenario against an isolated Runtime and the release Desktop binary.
Ten scenarios ran: nine recovery/continuity checks passed, and the tenth
(`auth_change_exclusion_honest`) passed as the **expected honest exclusion** — the
"10/10" result is not ten recoveries and does not claim fabricated success. No
product code was changed and nothing was pushed.

## Command and environment

```powershell
# Actual invocation (uv-managed CPython 3.13.14; the harness is stdlib-only):
C:/Users/diobrando/AppData/Roaming/uv/python/cpython-3.13.14-windows-x86_64-none/python.exe `
    apps/desktop/devtools/runtime_integration/run_rt3_matrix.py --long-seconds 180

# Documented form (any Python 3.11+ on PATH):
#   python apps/desktop/devtools/runtime_integration/run_rt3_matrix.py --long-seconds 180
```

- Desktop: `C:/dev/cos-desktop`, head `0dbe3c5`, release binary
  `target/release/cos-desktop.exe`, `sha256:5acb4af8…2faf7`, 21,109,760 B, built
  `2026-11-09T05:01:38+0900`. The binary is byte-identical to the RT3/RT8
  campaign's binary; the four runtime/credential source files the evidence
  depends on are byte-identical to `1eb788f` (`credentials.rs`,
  `runtime.rs`, `main.rs`, `protocol_transport.rs`; hashes in
  `session.json` → `desktop.source_spot_checks`), and no product `src` commit
  follows `1eb788f` (later commits add tests, harnesses, evidence and docs only).
- Runtime: `C:/AI/orchestrator`, head `d6e0ba20`, Python 3.13.14 venv. The seven
  pre-existing dirty Runtime entries (four tracked edits plus three `devplan`
  JSON files) are recorded by path in
  `session.json` → `runtime.dirty_paths`; none was staged or used.
- Desktop launch: discovery only — no `COS_DESKTOP_RUNTIME_ENDPOINT`, no
  `COS_DESKTOP_RUNTIME_TOKEN`; `COS_DESKTOP_RUNTIME_ISSUANCE_SECRET` and a
  redirected `COS_DESKTOP_RUNTIME_DISCOVERY_PATH` are the only connection inputs
  (`endpoint_passed_to_desktop` / `token_passed_to_desktop` are false in
  `session.json`). No fixture flag.

## Isolation

| Item | Value |
|---|---|
| Endpoints | `http://127.0.0.1:8135` (primary), `http://127.0.0.1:8136` (alternate lane) |
| Runtime epochs | main `fed08b62…`; alternate start 1 `44045286…`; alternate start 2 (epoch change under load) `945a984b…` |
| Discovery descriptors | 8135 `sha256:31b2fcd5…`, 8136 start 1 `sha256:9efe7122…`, 8136 start 2 `sha256:36a14482…` (schema 1, loopback origin, `ready=true`, removed on clean lifespan shutdown) |
| Database source | `C:/AI/orchestrator/db/generation.sqlite`, `sha256:fbfe3aba…bc0c56f6` |
| Database copy (start) | `target/rt3-matrix/run1/generation-copy.sqlite`, `sha256:f8771100…bfb3e3d`, 11,493,376 B |
| Database copy (end) | `sha256:336b86e5…f9891ede`, 12,685,312 B, journal rows 0 (the copy carries the test-only projection writes; the production DB was only read) |
| Projection | 39 TASKs, 260 snapshot mutations, target `task-00eeff78…`, title sha256 `sha256:5c807757…`, snapshot `snapshot-4c5cdd9e…`, trace-only context/scope/cursor hashes `33383c19…`/`86686f03…`/`2cb41a7e…` (see `session.json` → `projection`) |

The harness copies the production DB through a read-only SQLite backup handle,
writes an isolated config (`db_path`/`log_dir`/`output_root` redirected), starts
`isolated_runtime.py` with a per-run `ORCHESTRATOR_AUTH_SECRET` (memory only) and
`COS_RUNTIME_ORIGIN`/`COS_RUNTIME_DISCOVERY_PATH` redirected under
`target/rt3-matrix/run1/`. All scenario-specific journal/status mutation is a
test-only write in that copy (documented per scenario below), never in the
production DB and never in the Runtime repository.

## Scenario results

All ten scenarios passed in one run (`session.json` → `summary.verdicts`):

| # | Scenario | Verdict | Exact observation |
|---|---|---|---|
| 1 | `gap_closed_resync` | PASS | One TASK status change (`COMPLETED` → `CANCELLED`) was journaled and its history purged atomically (head 0 → 2, one row deleted, 0 rows left). The Desktop delta got `409` (Runtime log: `RESYNC_REQUIRED: task change journal no longer covers this cursor`), then **one** `GET /api/cos/desktop/snapshot` `200` with a new cursor; the missed `中止（CANCELLED）` state was rendered; 0 further conflicts in an 8 s settle window. |
| 2 | `cursor_overflow_resync` | PASS | The journal was advanced by 10,001 synthetic rows (head 2 → 10,003) past the 10,000-row bound. The Desktop delta got `409` (`… task change journal exceeds the retention bound`) and the Runtime pruned to **0 rows**; **one** `GET /api/cos/desktop/snapshot` `200` recovered the projection; 0 further conflicts in 8 s. |
| 3 | `desktop_clean_restart` | PASS | `WM_CLOSE` mid-observation → exit 0 with `observation shutdown confirmed` (preferences-save confirmation is best-effort under the bounded shutdown contract and was not required), Runtime remained ready, relaunch rendered the real projection, a fresh cursor never used before was minted (reuse=false). |
| 4 | `desktop_forced_restart` | PASS | Process tree killed (exit 1, `SUCCESS: … terminated`), no close markers, Runtime stayed ready, relaunch rendered the projection with a fresh cursor (reuse=false). |
| 5 | `runtime_epoch_change_desktop_restart` | PASS | Desktop closed, Runtime stopped cleanly (descriptor removed) and restarted on 8136 with epoch `44045286…`; relaunch rediscovered `http://127.0.0.1:8136` (descriptor `sha256:9efe7122…`), opened a fresh observation (new cursor) and rendered the projection. 0 Desktop requests to the old origin after close. |
| 6 | `epoch_change_under_load` | PASS | A live TASK projection stream (`CANCELLED` → `COMPLETED`, 3 delta polls applied) was followed by a clean Runtime stop: withheld state in 1.96 s (`情報源なし / データ非公開`, `データは非公開 — 現在の許可がありません`). The Runtime restarted on 8136 with a new epoch `945a984b…`; the **same PID** (28,756) recovered without relaunch: 1 `401`, 1 refresh `401`, 1 issue `200`, 1 fresh observation `200`, 0 commands, reauthorization line present. |
| 7 | `long_observation_idle` | PASS | 181.08 s live window: 621 delta polls, **one** cursor, max inter-poll gap **0.369 s**, 0 `401`/`409`/observation reopen/command requests, epoch stable, 11/11 liveness checks alive, title occurrence count constant (4 — the accessible UI exposes the selected title in several places; the count did not grow). No UIA enumeration ran mid-window because a PowerShell UIA tree walk can stall the UI-driven poll loop; start and end UI states were both `情報を受信中` with the expected status label. |
| 8 | `auth_change_bounded_reauth` | PASS | Three registry-pressure bursts (71/69/72 issuances past the 64-entry bound; supported issuance API, no test-only Runtime hook) each revoked the Desktop credential. Each attempt: delta `401` → refresh `401` → issue `200` → retried delta `403` `PRINCIPAL_MISMATCH` → resume open `403` → **fresh observation `200` with a new cursor**, exactly **one** `Runtime credential reauthorized…` line. 3 replacements total, 0 command requests. |
| 9 | `auth_change_exclusion_honest` | PASS | A fourth burst (64 issuances) exhausted the per-process replacement budget: delta `401` passed through, **0** new reauthorization lines, **0** credential requests in the following 10 s, 0 commands. UI rendered `タスク情報はありません`, `情報源なし / データ非公開`, `接続・情報の鮮度: データは非公開 — 現在の許可がありません`, and `Desktop内 · 観測情報を利用できません — 再接続する`. The reconnect strip action was invoked and did **not** fabricate a recovery (still excluded; 0 credential and 0 delta requests after). |
| 10 | `post_exclusion_relaunch_recovery` | PASS | A fresh Desktop session (relaunch) reauthorized from the issuance secret: 1 refresh `401`, 1 issue `200`, 1 observation `200`, new cursor, same epoch `945a984b…`; the projection rendered again. |

The event streams behind the table are in `run1/access-8135.json` /
`run1/access-8136.json` (request counts by status and route) and the sanitized
Runtime logs, which retain the exact `RESYNC_REQUIRED` and
`PRINCIPAL_MISMATCH` lines.

## Observed semantics and limitations

- **Replacement budget is per process runtime binding, not per observation
  session.** The epoch-change recovery consumed one of the three replacements, so
  the harness records an explicit Desktop relaunch before the auth matrix. The
  relaunch consumes no replacement because the stored credential is still valid
  (`reauth_line_count_after_relaunch = 0`), and the auth matrix then measures the
  full 3 + exclusion bound in one fresh process. This is an *observed* precision
  of the DC-MP-RT3 "per session" wording.
- **A replacement after registry eviction fails closed.** The newly issued
  credential has a new server-managed principal, so the old grant cannot be
  inherited: the retried read and the resumed open answer `403
  PRINCIPAL_MISMATCH` (3 + 3 Runtime log lines) before the client opens a fresh
  observation and re-reads the projection from a fresh snapshot. No stale
  projection is admitted; this is a limitation of recovery without a Runtime
  restart, not a silent recovery.
- **Credential-store cleanup target (test infrastructure).** The keyring service
  is composed as `desktop-observation.cos-desktop/observation/<sha256(origin)[:32]>`.
  The pre-existing `cleanup_credential_store` helper in `run_integration.py`
  deletes only the bare `cos-desktop/observation/<hash>` string, which never
  matches, so stale credentials survive between runs and cause an avoidable
  startup `401`. `run_rt3_matrix.py` deletes the exact composed target (both
  forms are attempted and recorded); the shared baseline helper is unchanged.
- **UIA enumeration perturbs the live poll loop.** A PowerShell UIA tree walk can
  stall the UI-driven observation loop for seconds. The long-observation scenario
  therefore measures request continuity without mid-window UIA enumeration and
  records only start/end UI states; the earlier full-run variant that sampled UI
  every 15 s recorded up to ~7 s harness-induced gaps and was not used for the
  committed claim.
- **Journal/status writes are test-only.** The gap, overflow, projection-stream
  and status assertions use direct SQLite writes in the isolated copy (the
  schema has journal triggers on `creative_tasks`); no Runtime endpoint was added
  or changed for this campaign.

## Leak scan

The harness scans every evidence file for every value it minted (run secret,
each harness-issued credential token, 279 values in this run) and for
bearer-token and operator-secret assignment patterns: **0 findings, clean**. The
Desktop's own tokens are never written to any log, request line or UI dump. This
README was written after the automated scan and contains no token, handle or
secret (only the environment-variable names are mentioned); a manual re-scan of
the final evidence directory is clean.

## Cleanup

- Both isolated Runtimes stopped through the lifespan (exit 0, clean), ports
  8135 and 8136 free, no Desktop process left (`session.json` → `cleanup`).
- The credential entries for both origins were removed by exact target (8136
  deleted; 8135 had none because the last session's teardown revoke deleted it).
- The isolated `LOCALAPPDATA` was removed; the DB copy, config and raw logs are
  retained under `target/rt3-matrix/run1/` (not committed, documented here) for
  review. Raw logs are not committed because they can be large; the sanitized
  copies in `run1/` are the committed record.

## Residuals

- Real-provider generation/task survival during a Desktop restart remains
  `NOT_RUN`: no generation provider was invoked (terminal tasks in the copy; the
  projection stream is a test-only TASK status write). "Task survival" is not
  claimed.
- The native GUI command path was not driven, so a live command resend after a
  `401` remains `NOT_RUN`; the campaign records **0** command requests and relies
  on the client's command-non-retry unit tests, not on a live command probe.
- Real token TTL expiry, OS/installer lifecycle registration, GPU/media
  presentation, screen reader/IME/DPI and product/UX acceptance are outside this
  campaign.
- The `403 PRINCIPAL_MISMATCH` → fresh-observation transition is measured but is
  not a supported seamless-principal-rotation path; a future protocol revision
  would be needed to make a reissued credential inherit an existing grant.

## Evidence files

`session.json` aggregates the run (harness command, bounds, source binding,
isolation hashes, per-scenario details, access summaries, cleanup, residuals,
leak scan, file hashes). `session.sha256` pins the final session document.
`run1/` holds the sanitized Runtime logs (8135/8136), the per-instance Desktop
stderr extracts, the UIA dumps and the access summaries — 32 hashed payload files
plus `session.json` (pinned by `session.sha256`) and this README.
