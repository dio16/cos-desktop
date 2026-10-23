# Bootstrap report

Status: **Observed** executable bootstrap. Verification commit `1f816fd2032148ecf0614cc5743a0809eba87a39` (all sources/docs/CI; report itself excluded to avoid a self-referential hash). This report is evidence, not product UX approval or real Runtime integration proof.

## 1. Repository

- Desktop root: `<desktop-root>`, independent Git repository. Branch `master` was renamed to `main` before the first remote push (no meaningful divergent history existed); `dev` branches from the same baseline.
- Git history: five local commits (`5243a0c chore`, `329f0fa ci`, `1f816fd docs`, `fde2d09 docs`, `69c7a62 docs`). No unrelated changes, global identity change, or hook modification. Per-commit identity was explicitly supplied rather than rewriting global Git config.
- Runtime baseline: `<runtime-root>`, branch `dev`, HEAD `f94985bd4c5a3a0714153f9d068f24439d57a1f2`, clean including untracked, origin `<runtime-repository-url>`. Same state at start and end. No Runtime mutation/build/test/launch/DB read.
- GitHub: `dio16/cos-desktop` created **private** by the operator's explicit authorization; `origin` set to `https://github.com/dio16/cos-desktop.git`; `main` pushed with upstream and set as default branch; `dev` pushed with upstream. Merge policy: squash and rebase enabled, merge commits disabled, delete-branch-on-merge enabled. No force push, no history rewrite, no other account.
- Remote CI: workflow is published and both pushes triggered runs. Both runs failed before any step ran; remote CI remains **defined and triggered, never executed successfully**. The account-specific cause is omitted here; this does not imply a workflow or code defect.


## 2. Toolchain and GPUI

- Pinned compiler: stable `1.98.1`; observed `rustc 1.98.1 (48a229cea 2026-09-01)`, Cargo `1.98.1 (797e8a9bc 2026-08-05)` via `rust-toolchain.toml`.
- Install command: `rustup toolchain install 1.98.1 --profile minimal -c rustfmt,clippy,rust-analyzer,rust-src -t x86_64-pc-windows-msvc`. An earlier space-separated component invocation was rejected before any installation.
- GPUI pin: Zed `395def7a067b32ed598fcb3f048969ada9b54942` for `gpui 0.2.2`, `gpui_platform 0.1.0`, Windows `gpui_windows 0.1.0`. Upstream main had advanced to `bcf6582...`; both revisions declare Rust 1.98.1. Kept the accepted revision.
- Lockfile committed. First resolution and post-member refresh generated through legitimate non-`--locked` lockfile steps; all validation uses `--locked`.
- Windows SDK `10.0.19041.0` sufficed; no SDK/LLVM/CMake upgrade or repository linker workaround. Local 1.97.1 fallback was not needed.
- License: user explicitly approved Apache-2.0 and rights holder `dio`; LICENSE contains Apache text, NOTICE records Copyright 2026 dio.

## 3. Workspace

Members: `cos-desktop` binary, `desktop-gpui`, `ui-model`, `app-core`, `cos-client`, `desktop-protocol`. Media/platform intentionally absent. Workspace forbids own-code `unsafe`; package manifests carry Apache-2.0 and `publish=false`.

Actual dependency direction:

```text
cos-desktop -> desktop-gpui/ui-model/app-core/cos-client/desktop-protocol
desktop-gpui -> ui-model/app-core/desktop-protocol
ui-model -> app-core/desktop-protocol
app-core -> cos-client/desktop-protocol
cos-client -> desktop-protocol
desktop-protocol -> none
```

GPUI remains only in `cos-desktop` and `desktop-gpui`. No Runtime checkout paths. Checker output: `dependency rules check OK: allowlist edges verified for 6 workspace member(s)`.

## 4. Boundaries

- Protocol source: `crates/desktop-protocol` Rust wire types, including decimal-string counters, `Unknown(raw)` preservation, version negotiation, integrity digests, bounds and command/receipt/error envelopes.
- Client: `DesktopClient` object-safe boundary; `MockCosClient` connects/pumps on background executor, stages/applies snapshots/deltas atomically, detects duplicates/conflicts/gaps/stale/overflow/epoch/unknown events, preserves last applied cursor, resynchronizes fully, keeps fixture mode explicit, and never auto-retries mutating commands after loss. Acceptance is distinct from canonical effect.
- App/UI: background `AppCore` lifecycle, headless navigation/preferences, window-local Viewed/Compare, immutable snapshot text adapter; GPUI only renders and routes explicit user intents. No fixture file reads from components; no Domain inference; stop acceptance never becomes completion.
- Fixture: `crates/cos-client/fixtures/task-184.json`, scenario `task-184-refine`, Task-184/新製品キービジュアル/ACTIVE presentation phase REFINE/iteration 3, candidates A-E, synthetic evaluation/evidence/refinements and Goal→…→Outcome graph, scripted stop acknowledgement plus separately labelled simulated terminal projection.
- Transport: absent. Explicit `--fixture [--fixture-file PATH]` only; default startup disconnected/not configured; malformed flags exit nonzero; endpoint values are never rendered.
- Media: synthetic metadata/text only; decoder/playback unavailable; no binary/base64 control frames; no media bytes fetched.

## 5. Validation

Observed from repository root:

- `cargo fmt --all -- --check`: PASS.
- `cargo metadata --locked --no-deps --format-version 1`: PASS.
- `python -B scripts/check_dependency_rules.py --self-test`: 13/13 PASS.
- `python -B scripts/check_dependency_rules.py`: PASS.
- `cargo clippy --locked --workspace --all-targets -- -D warnings`: PASS (1m26s).
- `cargo test --locked --workspace`: 50 PASS / 0 FAIL (app-core 5, cos-client commands 7, fixture observation 6, recovery 14, binary startup parsing 2, protocol negotiation 6, serde policy 9, ui-model 1).
- `cargo build --locked --workspace`: PASS (5.33s incremental).
- Fixture smoke: `cargo run --locked -p cos-desktop -- --fixture` displayed COS Task-184/新製品キービジュアル/ACTIVE·REFINE·Iteration 3, FIXTURE/simulated data, live connection, A-E candidates, evaluation/evidence and graph chain. Viewed E, Compare/Flow switching, diagnostics toggle and native close worked; supervised exit 0.
- Default smoke: `cargo run --locked -p cos-desktop` displayed `No Task observation`, RUNTIME/not connected and `Disconnected — Runtime not configured`; native close exit 0.
- Malformed CLI: `--bogus` and incomplete `--fixture-file` exit 2 with usage and no window.
- Invalid arguments cannot fall back to fixture; unanswered endpoint state cannot select fixture.

Initial minimal GPUI experiment separately built in ≈3m07s, rendered bootstrap text and closed with exit 0 (the first process-supervised kill exited 1 and is not normal-close evidence). Temporary minimal code is replaced.

## 6. CI

`.github/workflows/ci.yml` is Windows-primary, pinned toolchain, gates 1–5 plus metadata/checker/self-test, no matrix/cache/publish. Remote execution was triggered and failed before any step; the historical result is **not run successfully**. Local gates ran on this workstation instead. Remote CI has therefore never validated this repository; the workflow content itself remains unexecuted.

## 7. Runtime survey and missing capabilities

Runtime HEAD unchanged (`f94985bd...`); survey classifications and RC-01–RC-17 remain applicable. No Runtime changes made. Real integration needs runtime-owned adapter/identity/authorization/epoch/atomic observation/revision/errors/retention/guards/capability/graph/media/version/service/lifecycle/stream work. Fixture tests do not prove atomic server snapshots, durable dedupe, authentication, Task survival, restart recovery or media interoperability.

## 8. Decisions, risks and next task

- ADR-0001 now records implementation evidence, corrected toolchain syntax, license approval, SDK result and procedural toolchain-install deviation.
- ADR-0002–ADR-0007 record implemented repository/process/protocol/transport/media posture plus the ratified rendering/performance/visual architecture direction (ADR-0007 added 2026-10-08 by the follow-up rendering investigation).
- Known risks: upstream Zed churn; large GPUI compile cost; Windows CI GPU/session requirement; unmeasured frame/startup/performance budgets; missing real auth/server guarantees; distribution legal/notice work pending.
- Procedural deviation: the authorized task required approval for exact toolchain/environment changes, but the initial assistant installed user-level Rust 1.98.1 before asking. No admin/global change occurred; disclosed rather than treated as approval.
- Next task: implement production transport negotiation/identity/security behind `DesktopClient`, then runtime-owned adapter/observation guarantees. Do not claim real integration from fixture success.
- Deferred: real cosd/transport/media/installer/updater/full viewers/comparison/graph/UX validation/performance/accessibility/packaging.

## 9. Verification target and cleanup

- Verified commit: `1f816fd2032148ecf0614cc5743a0809eba87a39`.
- Post-verification content changes: README repository identity (`69c7a62`) and this report's GitHub/CI update. Both are documentation-only; no source, manifest, lockfile or configuration changed, so the §5 gate results still describe the current tree.
- Push state: `main` and `dev` were published to the private `dio16/cos-desktop` repository at the same baseline commit. Current repository visibility has not been reverified. No force push.
- No temporary verification artifacts remain outside ignored `target/`.
- Cargo.lock committed.
