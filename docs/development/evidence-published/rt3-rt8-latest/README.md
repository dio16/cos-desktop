# RT3 automatic reauthorization + RT8 runtime discovery — live evidence

Scope: the two-run campaign in `session.json` / `run1/` / `run2/` that restarted an isolated Runtime under the same Desktop process and exercised the RT8 discovery descriptor.

## Scenario results

All five scenarios passed in both runs: `discovery_connect`, `restart_recovery_without_relaunch`, `descriptor_lifecycle`, `task_survival_after_desktop_exit`, `clean_close`. The recorded run-file hashes match the committed files (18/18), the Desktop was launched with no endpoint and no token environment (`endpoint_passed_to_desktop=false`, `token_passed_to_desktop=false`, `run_secret_recorded=false`), and the leak scan found no bearer tokens, `mh1.` handles or secrets.

## Source binding

- Desktop: the session records base head `01903e7` plus the 16 changed/added product/config/docs files; every recorded file sha256 matches the `1eb788f` blobs, including the untracked `apps/desktop/src/discovery.rs`, so the Desktop product source is source-bound to `1eb788f`. The binary sha256 `5acb4af8…` matches the still-present `target/release/cos-desktop.exe` (21,109,760 B).
- Desktop build: `desktop.build.performed=false` — the campaign reused the pre-built binary; the source-file equivalence above is the compensating evidence.
- Runtime: the session records only `{root: C:/AI/orchestrator, head: d490704d, python}` and no dirty/untracked snapshot. `runtime_discovery.py` arrives in the committed Runtime commit `32b6cef`, so the live campaign ran with uncommitted Runtime code and **cannot prove byte equality to `32b6cef`**. The committed Runtime commit contains the same descriptor publisher and passes the targeted suite (`96 passed`); the live Runtime source binding remains **`NOT_MEASURED`**.

## Residuals

- RT3 full matrix (gap/overflow/auth-change bursts, Desktop re-launch combinations, long observation) is `NOT_RUN`.
- A real killed-Runtime stale descriptor was not measured live — the live campaign used a graceful stop; the stale-descriptor structure/unavailable path is unit-tested.
- Windows service/installer registration and product lifecycle acceptance are not implemented or claimed.
- Media byte acquisition is out of this campaign's scope (projection recovery only; RT6 covers media).
- Only two runs were recorded.
