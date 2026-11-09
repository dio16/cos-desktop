# RC-11 live command/media capability support — measured evidence (LIMITED)

Scope: an isolated Runtime (fresh copy of `C:/AI/orchestrator/db/generation.sqlite`, own port **8141**, per-run secrets in memory only, isolated config/CAS/logs, redirected discovery descriptor) after the RC-11 extension. The measured task is an existing real task with an available image artifact; a second existing task with an accepted input but no runs measures the media-unavailable direction. The captured `TASK`/`CAPABILITIES` payloads were passed to a test-only harness that runs the real `app_core::capability` gate code.

**Verdict: `LIMITED`.** The supported STOP/media path is live-measured over the Runtime's own CAPABILITIES projection and the real Desktop gate code; real external grant issuance/revocation, a non-image available artifact and server-scope create remain out of scope or `NOT_MEASURED`.

## Source binding

- Runtime head `d6e0ba202f4068860c85208f9ea6e3b3b681bfcb` (dirty tracked entries recorded in `record.json`; none staged).
- Desktop head `7d52b9712bc42e00e152adf7b63ea3fb1e198ea3` (tree `c6b0013f75e674d4e69b455bef2328b4bdcae6af`).
- Harness files: `apps/desktop/devtools/runtime_integration/run_rc11.py` `sha256:6a847bac92430762cad8713cb77b89083cfca74bc802917290b013383494ef09`; `crates/app-core/tests/rc11_live_capability_gates.rs` `sha256:7bdc9bee6796aea027edd131727bb62a5e4e9104906fa9068d138a421f4cfa1c`.
- Database: source `sha256:fbfe3ababb9f53dfb1c373774ce99bdfed813d7b64ad743164fa6adfbc0c56f6`; isolated copy before seeding `sha256:f87711008a546c31caf75ed6d24c3a5c95b1d824c8b43868f13e1e852bfb3e3d`; after seeding `sha256:da974cf9ad5aa6f3514a6844af1e14d053d3d82b621a932aab82e322b8509ec0`.
- Runtime epoch `f713eb4f09774e18b2be1fdd460f1b9a`; discovery descriptor removed on clean shutdown: `True`.

## Observed CAPABILITIES projections

| State | Task | `capabilities_ref` (short) | Revision | Availability | STOP_TASK | REUSE_TASK | ORIGINAL | THUMBNAIL | PREVIEW | METADATA |
|---|---|---|---|---|---|---|---|---|---|---|
| `terminal-image` | `task-80c8632b4bb843bf93189a9cdb79df19` | `510dd2accfa1cbdf` | `8` | `SUPPORTED` | `UNAVAILABLE` | `UNSUPPORTED` | `SUPPORTED` | `SUPPORTED` | `UNSUPPORTED` | `UNSUPPORTED` |
| `live-active` | `task-80c8632b4bb843bf93189a9cdb79df19` | `510dd2accfa1cbdf` | `10` | `SUPPORTED` | `SUPPORTED` | `UNSUPPORTED` | `SUPPORTED` | `SUPPORTED` | `UNSUPPORTED` | `UNSUPPORTED` |
| `stop-pending` | `task-80c8632b4bb843bf93189a9cdb79df19` | `510dd2accfa1cbdf` | `11` | `SUPPORTED` | `UNAVAILABLE` | `UNSUPPORTED` | `SUPPORTED` | `SUPPORTED` | `UNSUPPORTED` | `UNSUPPORTED` |
| `no-artifact` | `task-e70275e23f784dbda3f49eaf9332b442` | `b0620e2e169d2985` | `14` | `SUPPORTED` | `UNAVAILABLE` | `UNSUPPORTED` | `UNAVAILABLE` | `UNAVAILABLE` | `UNSUPPORTED` | `UNSUPPORTED` |

Every payload is TASK-scoped and its `capabilities_ref` equals the task's reference. Exact payloads are in `projections/` and `gates/capture.json`.

## Desktop gate decisions (real `app_core::capability` code)

`gates/decisions.json`; the Rust harness asserts every expectation and the Python harness cross-checks the recorded decisions.

| State | stop | reuse | original | thumbnail | preview | metadata | create (server scope) |
|---|---|---|---|---|---|---|---|
| `terminal-image` | disabled `capability_unavailable` | disabled `capability_unsupported` | enabled `supported` | enabled `supported` | disabled `capability_unsupported` | disabled `capability_unsupported` | disabled `capability_not_projected` |
| `live-active` | enabled `supported` | disabled `capability_unsupported` | enabled `supported` | enabled `supported` | disabled `capability_unsupported` | disabled `capability_unsupported` | disabled `capability_not_projected` |
| `stop-pending` | disabled `capability_unavailable` | disabled `capability_unsupported` | enabled `supported` | enabled `supported` | disabled `capability_unsupported` | disabled `capability_unsupported` | disabled `capability_not_projected` |
| `no-artifact` | disabled `capability_unavailable` | disabled `capability_unsupported` | disabled `capability_unavailable` | disabled `capability_unavailable` | disabled `capability_unsupported` | disabled `capability_unsupported` | disabled `capability_not_projected` |

The facts the projection carries and the gate decisions agree: stop is offered only for the live state with no accepted stop command; media `ORIGINAL`/`THUMBNAIL` follow the projected available image artifact; `REUSE_TASK`, `PREVIEW` and `METADATA` stay disabled with their projected reasons; create is not projected (catalog-authoritative).

## Revocation

Revocation behavior is unchanged and covered by Runtime pytest: the projection-level `UNAVAILABLE` plus reason denies every command/media gate before the entry is read, so a revoked projection cannot enable an action. This run did not re-seed a revocation receipt.

## What was live-measured vs NOT_MEASURED

- Measured: the Runtime CAPABILITIES producer's TASK-scope `commands`/`media` derivation over a live isolated Runtime, the terminal/live/stop-pending STOP states, the image-artifact `ORIGINAL`/`THUMBNAIL` support, the no-artifact `UNAVAILABLE` direction, and the real `app_core::capability` gate decisions.
- `NOT_MEASURED`: a non-image available artifact (all production artifacts are `image/png` and artifact rows are immutable; covered by Runtime pytest), a server-scope CAPABILITIES projection (the Runtime emits TASK scope only; create stays catalog-authoritative), and real external authority issuance/revocation.
- The Desktop GUI binary was not launched; the capture and gate decisions are protocol-level/headless.

## Cleanup

The isolated Runtime listener was stopped and the isolated working copy removed (`record.json` `cleanup`); no listener on port 8141 remains. Evidence contains no secrets (leak scan in `record.json`).
