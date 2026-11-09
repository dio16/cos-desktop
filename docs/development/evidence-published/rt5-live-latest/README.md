# MP-RT5 live capability grant/revocation — measured evidence (LIMITED)

Scope: an isolated Runtime (fresh copy of `C:/AI/orchestrator/db/generation.sqlite`, own port **8134**, per-run secrets in memory only, isolated config/CAS/logs, redirected discovery descriptor) seeded with a Rust-validated authoritative capability snapshot set, observed through the Desktop observation protocol; the captured `TASK`/`CAPABILITIES` payloads were then passed to a test-only harness that runs the real `app_core::capability` gate code.

**Verdict: `LIMITED`.** The grant, latest-set selection, revocation and the Desktop gate decisions were live-measured. The Runtime CAPABILITIES producer emits no command/media entries (and only `TASK` scope), so a supported STOP/media/server-scope create grant is **`NOT_MEASURED`**.

## Source binding

- Desktop base head `fa069b8fb41ec77cb23a7e81304f9f6044362e64` (tree `e888651f…`); the two harness files below were uncommitted at measurement time (their sha256 is recorded in `record.json`):
  - `apps/desktop/devtools/runtime_integration/run_rt5.py` — `sha256:c087530e…6a7adb57`
  - `crates/app-core/tests/rt5_live_capability_gates.rs` — `sha256:b99521f0…f157c4`
- Runtime head `d6e0ba202f4068860c85208f9ea6e3b3b681bfcb`; no Runtime product change and none of the seven pre-existing dirty entries were staged.
- Database: source `sha256:fbfe3aba…bc0c56f6`; isolated copy before seeding `sha256:f8771100…bfb3e3d` (11,493,376 B); copy after seeding `sha256:71295d37…c9aa61b1`.
- Runtime endpoint `http://127.0.0.1:8134` (`endpoint_sha256_16` `sha256:8e7bc6781bf5520f`), epoch `a749e863d2e049e192502028670325ff` (`epoch_sha256_16` `sha256:97df69d81420b4b0`), discovery descriptor published after start and removed on clean lifespan shutdown (`discovery_sha256` `sha256:9f3700e2…3d9db491`).

## Authoritative seeded capability sets

Each set was written through the Runtime's own `capability_registry.store_snapshot_set` (Rust `contract-core` validates the descriptor and computes the descriptor/set digests). The measured task is an existing real task from the copied database (`task-80c8632b…`, artifact `art-v1-33c104b4…`); three read-model runs were inserted into the isolated copy only, to bind each set.

| Set | Run | Capability | Version | Descriptor digest | Snapshot-set digest (`capabilities_ref`) |
|---|---|---|---|---|---|
| S1 | `run-rt5-db79df19-i1` | `img-gen-v1` | `1.0.0` | `sha256:3d8f42c3…c3105c7e` | `sha256:8271c47e…5204c6217` |
| S2 | `run-rt5-db79df19-i2` | `img-gen-v2` | `1.1.0` | `sha256:df883ec1…5cc7a1a2` | `sha256:1bc7e1c7…a2c6a9fde8` |
| S3 | `run-rt5-db79df19-i3` | `img-gen-v3` | `1.2.0` | `sha256:5f394047…51f099a6` | `sha256:1450d1ee…967ae2ed8b` |

Every snapshot carries `schema_version 1`, `source "local"`, effects `["compute","local_write"]`, permissions `["artifact.write"]`, `auth_requirement.mode "none"`, and resource `{estimated_vram_mb 1024, estimated_runtime_s 30, cost_budget 10, timeout_s 120}`; the issuer authorization authorizes exactly those effects/permissions and version. Full snapshot/authorization material and the store receipts are in `record.json` (`seed.sets`).

## Observed CAPABILITIES projections

`gates/capture.json` holds the exact post-decode `TASK`/`CAPABILITIES` payloads; per-state copies are in `projections/`.

| State | `capabilities_ref` | Projection revision | Availability | Reason | commands | media | features |
|---|---|---|---|---|---|---|---|
| `unseeded` | — (task has no `capabilities_ref`) | — | — | — | — | — | — |
| `grant-s1` | `…8271c47e` | `10` | `SUPPORTED` | — | `[]` | `[]` | `img-gen-v1` `SUPPORTED` |
| `grant-s2-latest-set` | `…1bc7e1c7` | `13` | `SUPPORTED` | — | `[]` | `[]` | `img-gen-v2` `SUPPORTED` |
| `revoked` | `…1bc7e1c7` | `14` | `UNAVAILABLE` | `capability revocation is active` | `[]` | `[]` | `img-gen-v2` `SUPPORTED` (entry) |
| `revoked-new-set-s3` | `…1450d1ee` | `17` | `UNAVAILABLE` | `capability revocation is active` | `[]` | `[]` | `img-gen-v3` `SUPPORTED` |

Every payload is `scope {"kind":"TASK","task_id":"task-80c8632b…"}`, carries constraints `auth.mode=none`, `cost_budget=10`, `estimated_runtime_s=30`, `estimated_vram_mb=1024`, `timeout_s=120`, and the task's `capabilities_ref` always equals the projection identity. The `revoked` state keeps the entry-level `SUPPORTED` value while the projection itself is `UNAVAILABLE`; the gate denies on projection availability (no false-enabled).

## Desktop gate decisions (real `app_core::capability` code)

`gates/decisions.json` (produced by the ignored test `rt5_live_capability_gates`, exit 0, capture cross-check `match`; the same decisions were reproduced after `cargo fmt`).

| State | stop (`STOP_TASK`) | media (`ORIGINAL`) | create (server scope) | feature gate |
|---|---|---|---|---|
| `unseeded` | disabled `capability_not_projected` | disabled `capability_not_projected` | disabled `capability_not_projected` | — |
| `grant-s1` | disabled `capability_unknown` ("この操作（STOP_TASK）…投影されていません") | disabled `capability_unknown` | disabled `capability_not_projected` | `img-gen-v1` enabled `supported` |
| `grant-s2-latest-set` | disabled `capability_unknown` | disabled `capability_unknown` | disabled `capability_not_projected` | `img-gen-v2` enabled `supported` |
| `revoked` | disabled `capability_unavailable` ("capability revocation is active") | disabled `capability_unavailable` | disabled `capability_not_projected` | `img-gen-v2` disabled `capability_unavailable` |
| `revoked-new-set-s3` | disabled `capability_unavailable` | disabled `capability_unavailable` | disabled `capability_not_projected` | `img-gen-v3` disabled `capability_unavailable` |

The media gate is evaluated before any descriptor/byte fetch and denies in every state, so the harness requested no media descriptor; the GPUI call-site ordering with an **enabled** live media entry is not reachable because the Runtime projects no media support. The create gate uses only the sole server-scope projection, and the Runtime emits none (TASK scope only), so create remains catalog-authoritative as designed.

## Revocation

A genesis revocation receipt (`revocation_set_id revocation-set-0`, sequence 0, issuer `rt5-harness-authority`, reason `SECURITY_RECALL`, fixed audit `issued_at 2026-11-09T00:00:00Z`) targeted the S1 descriptor and was validated by the Runtime's Rust contract; `append_revocation_receipt` stored it and advanced the pointer to revision 1 (receipt `sha256:5d93097f…d9dc1`, reproducible across runs). The task-less revocation journal row made the next delta read answer **`409 RESYNC_REQUIRED`** (the protocol's designed resync, not a silent partial update); a fresh observation then published `UNAVAILABLE` with reason `capability revocation is active` and a new projection revision (`13 → 14`). Publishing S3 under the active revocation changed identity and revision again (`17`) while staying `UNAVAILABLE`.

## What was live-measured vs NOT_MEASURED

Measured: authoritative set storage and identity; projection emission (identity/revision/availability/features/constraints/scope); observer latest-set selection; revocation → `UNAVAILABLE` + projected reason + revision change; delta resync on revocation; Desktop gates over the real payloads (disabled directions and the feature grant); no false-enabled state; clean shutdown and no leaked secrets.

`NOT_MEASURED`:
- a supported `STOP_TASK` command entry — the Runtime producer hardcodes `"commands": []`;
- a supported `ORIGINAL` media entry — it hardcodes `"media": []`;
- any server-scope CAPABILITIES projection — it emits `TASK` scope only;
- grant issuance or revocation by a real external authority — the receipt is harness-seeded (though Rust-validated and appended through the Runtime helper);
- GPUI/native gating and media ordering with an enabled live entry; AX/screen-reader.

Residual: the Runtime CAPABILITIES producer reads a non-schema `capability_name` field (authoritative snapshots therefore project their `capability_id`) and never composes command/media support; closing those is a separate Runtime task. The plan records this as the honest RT5 residual.

## Reproduction

```text
C:/dev/cos-desktop> C:/AI/orchestrator/.venv/Scripts/python.exe ^
    apps/desktop/devtools/runtime_integration/run_rt5.py
```

The harness itself invokes `cargo test --locked -p app-core --test rt5_live_capability_gates -- --ignored --nocapture` with `RT5_GATE_CAPTURE`/`RT5_GATE_DECISIONS_OUT`. A direct gate-harness run needs those environment variables and a capture document from `target/rt5/<stamp>/rt5-gate-capture.json`. The harness requires port 8134 to be free, the Runtime venv Python, and the Runtime source checkout; it copies the production DB through a read-only SQLite backup, seeds only the copy, and stops its own Runtime (exit 0) before returning.

Leak scan: 8 generated evidence files (projections, gates, logs, `session.json`, `record.json`), 19 secret/opaque-capability values, 0 findings; tokens, context/cursor/scope identifiers are recorded only as 16-hex sha256 prefixes.
