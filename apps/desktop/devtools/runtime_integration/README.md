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

## Boundaries

- Applies no product changes; it only starts the existing Runtime and Desktop binaries.
- `PrintWindow` (with `PW_RENDERFULLCONTENT`) is used for screenshots; the window is never
  foregrounded and `CopyFromScreen` is never used.
- UIA is read for names/ids only; no screen-reader, IME, High Contrast or DPI claim.
- The production database is only read; every Runtime run uses its own copy.
- A Runtime restart loses the in-memory credential registry, so the recorded expectation is
  withheld-until-relaunch; a recovered projection without a fresh credential would be
  recorded as a deviation, not worked around.
