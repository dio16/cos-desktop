# COS Desktop development rules

The sole canonical product/UX design is [docs/architecture/COS_DESKTOP_PRODUCT_UX_ARCHITECTURE.md](docs/architecture/COS_DESKTOP_PRODUCT_UX_ARCHITECTURE.md). Preserve its semantics; record physical details/decisions in architecture docs and ADRs. Logical workspace diagrams do not authorize a monorepo. Consult [README](README.md) for the document map. Binary package is `cos-desktop`; invoke quality/startup selectors with `-p cos-desktop`, not `-p desktop`.

## Scope and authority

- Desktop owns GPUI shell, presentation, UI Model, app-core, cos-client, media presentation/cache, OS/window integration, diagnostics/accessibility/performance and independent release lifecycle.
- COS Runtime owns REOS/COS Domain, Task semantics, DMLG/GraphRevision, immutable ExecutionPlan, workers, CAS/artifact canonical identity, Evaluation/Evidence/Refinement/Provenance/Knowledge, Formal Outcome, persistence, future cosd and protocol server/canonical projections.
- No COS semantics in UI: never derive Task Status, evaluation truth, best/winner, formal selection/outcome, GraphRevision, plan, artifact identity or stop completion locally.
- No direct Runtime DB/artifact DB/internal config access, internal crate/path dependency, submodule, nested repository, merged release/history or hardcoded internal Python launcher.
- No cross-repository mutation without a separate explicit task. COS Runtime is source inspection/development discovery only unless integration testing is explicitly authorized.
- Documentation preparation does not authorize bootstrap implementation. New implementation requires a new explicit instruction; this file is not permission to begin it.

## Dependencies

Responsibility order: `cos-desktop` binary → desktop-gpui → ui-model → app-core → cos-client → desktop-protocol. It is not adjacency-only. The sole direct dependency allowlist is [DEPENDENCY_RULES.md](docs/architecture/DEPENDENCY_RULES.md), including normal/build/dev/target-specific edges; `scripts/check_dependency_rules.py` enforces it in CI/local gates. Follow that table; update its checker together, never copy an independent table here. No cycles. No needless type re-export chains or duplicate Domain definitions.

GPUI and texture/resource management remain at desktop-gpui/binary startup boundary. ui-model and app-core are GPUI-free; cos-client is GPUI/UI-model/COS-internal-free; desktop-protocol has no higher-layer or Domain dependencies. UI Runtime operations go through app-core, never directly to cos-client. Direct DTO imports from desktop-protocol are permitted by the table and do not authorize Runtime calls. Introduce media/platform only with real responsibilities, never empty crates.

## State, protocol and safety

- Separate typed local presentation state from read-only Domain projections. Viewed and Compare Selected are local; Best-so-far and Formal Selected are Runtime projections.
- Task Status, Inner Phase, Iteration, Artifact Availability, Connection and Formal Outcome are independent axes. Desktop disconnect is not Task failure; Stop sent/accepted is not Stop completion.
- Protocol-first integration: contract → server → client → compatibility → migration. Future desktop-protocol Rust types are bootstrap's single machine-readable source; generated schemas are derivatives. Existing Runtime contracts/validation/identity remain authoritative behind adapters.
- Implement atomic snapshot/subscription and bounded resync/replay per [Protocol Draft](docs/architecture/DESKTOP_PROTOCOL.md), not separately raced fetch/subscribe. Never unsafe automatic command retry after lost response.
- Fixture mode must be explicit and visible. No automatic fallback to mock after connection failure; mock success is not real runtime success. Components cannot read fixture files; scripted provider must not reimplement Domain transitions. No fake successful stubs for unsupported features.
- Main-thread non-blocking: input/layout/paint/focus/small updates only; IPC, serialization, media decode, graph layout, filesystem/network/search/thumbnail work runs in background with bounded cancellation-aware queues.
- No control-plane binary/base64 images. Treat media/metadata as untrusted. No arbitrary path descriptor, metadata execution, plaintext credentials or secrets in projections/logs/diagnostics/crash reports. Server validates commands; disabled UI is not access control.

## Development workflow

- Follow [the storage-bounded build workflow](docs/development/BUILD_AND_RUN.md#41-storage-bounded-development-workflow): use `cargo check` or `quality.ps1 -CheckOnly` during the edit loop, one ordinary local target, and the unchanged full quality/Native route at completion. Independent Gate targets remain isolated; finish the [terminal retention procedure](docs/integration/REFINE_TASKERN_EXECUTION_BOUNDARY.md#build-storage-and-terminal-retention) within the Caller grant instead of accumulating targets/toolchain copies. No blanket deletion of build trees, paused-task executables or immutable evidence.
- Gate-only source retention: `work/repo` is temporary verification scratch, not a second permanent source/evidence archive. For a newly authorized successful Gate, MUST plan and finish the [successful-Gate workspace disposal procedure](docs/integration/REFINE_TASKERN_EXECUTION_BOUNDARY.md#successful-gate-workspace-disposal): retain the pinned Git source and durable private evidence, check native status/inspect/verify, remove only the approved exact workspace, then verify evidence integrity again. Refine 0.3.1 does not do this automatically.
- Never apply that disposal rule to failed/cancelled/interrupted/nonterminal/quarantined/cleanup-incomplete workspaces, delegated executions or Refinement sessions. Missing deletion authority, retained Git objects, evidence, process cleanup or producer compatibility means HOLD for the storage step, not deletion, another full copy, a new Gate or a rewritten receipt. No retrospective bulk cleanup or private-history publication follows from the rule.
- Read the current Taskern/Refine Skills and matching producer contracts before their operations. A fixed candidate, allowed checks/environment and explicit Caller authorization are required; no Gate success, evidence integrity or storage cleanup implies product acceptance, development-code promotion or release permission.

## Completion and quality

Test affected observable behavior before completion. For subsequent code work require fmt, dependency structure/allowlist check, clippy, tests, build and real native startup smoke as specified in [TESTING.md](docs/development/TESTING.md):

```text
cargo fmt --all -- --check
cargo metadata --locked --no-deps --format-version 1
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked --workspace
```

These commands are implemented gates for code work (native startup smoke verified separately on Windows). Metadata alone does not enforce the custom allowlist; also run the dependency checker and its self-test: `python -B scripts/check_dependency_rules.py --self-test` and `python -B scripts/check_dependency_rules.py`. `pwsh scripts/quality.ps1` runs gates 1–5 in documented order. No warnings left unresolved, no panic as normal control flow, no casual unwrap/expect, meaningful public types, no unnecessary lint framework. Own-code unsafe is prohibited by default (`unsafe_code = "forbid"` in workspace/package lints); any necessary exception needs explicit architecture review, narrow scope and documented safety proof before use. Third-party unsafe is not claimed to be eliminated.

- Update architecture/ADR/development docs when contracts or actual commands change. Label Observed, Accepted, Proposed, Deferred accurately. Never call unverified behavior successful; CI build is not window startup, fixture recovery is not real Task survival, and technical bootstrap is not product UX approval. Full Artifact/Compare/Flow work requires separate operable prototype and UX validation. Bootstrap includes local commits and Cargo.lock; LICENSE/NOTICE are Apache-2.0/Copyright 2026 dio. Repository publication state is external to this checkout and must not be inferred from local documentation. Create BOOTSTRAP_REPORT.md only with actual implementation evidence.
