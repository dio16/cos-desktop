# COS Desktop development rules

The sole canonical product/UX design is [docs/architecture/COS_DESKTOP_PRODUCT_UX_ARCHITECTURE.md](docs/architecture/COS_DESKTOP_PRODUCT_UX_ARCHITECTURE.md). Preserve its semantics; record physical details/decisions in architecture docs and ADRs. Logical workspace diagrams do not authorize a monorepo. Read [Desktop Design Canon](docs/architecture/DESKTOP_DESIGN_CANON.md) for Desktop cross-layer authority/decision owners and [Desktop Design Contracts](docs/architecture/DESKTOP_DESIGN_CONTRACTS.md) for normative component behavior/API/failure/acceptance. The [Implementation Master Plan](docs/development/IMPLEMENTATION_MASTER_PLAN.md) is their **derived execution index** (readiness, gates, DAG, evidence), never a second design canon. Consult [README](README.md) for the document map. Binary package is `cos-desktop`; invoke quality/startup selectors with `-p cos-desktop`, not `-p desktop`.

## Scope and authority

- Desktop owns GPUI shell, presentation, UI Model, app-core, cos-client, media presentation/cache, OS/window integration, diagnostics/accessibility/performance and independent release lifecycle.
- The separate COS Runtime repository owns REOS/COS Domain, Task semantics, DMLG/GraphRevision, immutable ExecutionPlan, workers, CAS/artifact canonical identity, Evaluation/Evidence/Refinement/Provenance/Knowledge, Formal Outcome, persistence, future cosd and protocol server/canonical projections.
- No COS semantics in UI: never derive Task Status, evaluation truth, best/winner, formal selection/outcome, GraphRevision, plan, artifact identity or stop completion locally.
- No direct Runtime DB/artifact DB/internal config access, internal crate/path dependency, submodule, nested repository, merged release/history or hardcoded internal Python launcher.
- No cross-repository mutation without a separate explicit task. The Runtime source checkout is for inspection/development discovery only unless integration testing is explicitly authorized.
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

- Follow [the storage-bounded build workflow](docs/development/BUILD_AND_RUN.md#410-storage-bounded-development-workflow-2026-10-23): use `cargo check` or `quality.ps1 -CheckOnly` during the edit loop, one ordinary local target, and the unchanged full quality/Native route at completion. Independent Gate targets remain isolated; finish the [terminal retention procedure](docs/integration/REFINE_TASKERN_EXECUTION_BOUNDARY.md#build-storage-and-terminal-retention) within the Caller grant instead of accumulating targets/toolchain copies. No blanket deletion of build trees, paused-task executables or immutable evidence.
- Follow the [Taskern / Refine boundary](docs/integration/REFINE_TASKERN_EXECUTION_BOUNDARY.md) and current `taskern`, `refine`, `tern-chatgpt-review` Skills. Read only needed operation/reference guidance; installed binary contracts and actual CLI discovery outrank historical examples. Record Skill discovery, loading, invocation and execution separately. Missing/stale capability is a reported blocker, never a simulated success.
- Standard flow: Taskern-derived plan/context → existing Coding Agent implementation/test → Caller-authorized candidate commit → Refine 0.3 independent Gate → Tern review → separate Caller acceptance. Refine must not start a second implementation agent for this flow. Repair in the same Agent session; every changed candidate gets a new pinned commit and explicitly authorized Gate within the finite Caller grant.
- Taskern owns deterministic planning-contract/context validation, not implementation or acceptance. Its existing `adapt` emits delegated `TaskRequest`, not `GateRequest`; do not reuse old Refine execution requests as Gates, weaken DC acceptance or forbid independent regression tests.
- Gate checks a full pinned commit in an independent detached checkout; dirty/untracked source is excluded and FROZEN_WORKTREE is HOLD. Native preflight UNKNOWN/FAIL and exit 0 are not PASS. Gate success, stored integrity, external review and Caller acceptance are distinct; reconcile lost responses by accepted Gate ID before any new dispatch.
- Preserve candidate/request/evidence correlation, checks/logs/cleanup, budgets, review and disposition. Use the existing Tern conversation and review to 0 BLOCKING / 0 ACTIONABLE NON-BLOCKING / 0 EXCESSIVE, or HOLD; protect private data before submission. No automatic retry, per-task orchestration framework, dev integration, push, main merge or release permission follows.
- MP-COR1 is completed by its historical Caller acceptance, not a Refine 0.3 Gate; original records are [retained privately](docs/development/evidence/README.md#historical-records). Each user-gated step stops at completion and awaits the next explicit instruction. Documentation/workflow preparation does not authorize product code or change DD/RC/R0/UX/video approval conditions.

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

Design change order (mandatory): update the single owning Product Canon/ADR/Protocol/Design Canon/Design Contract through its authorized owner **first**, then update derivative implementation-plan task cards/status/DAG/crosswalks in the same change. Never add or change a behavioral requirement solely in the plan; each MP task must link exactly one DC-MP contract. Do not infer DD-01, UX, R0, video or Runtime approval from design freeze, a plan label or fixture evidence. Verify contract↔task link completeness, decision gates, relative links and DAG after any docs edit. Update architecture/ADR/development docs when contracts or actual commands change. Label Observed, Accepted, Proposed, Deferred accurately. Never call unverified behavior successful; CI build is not window startup, fixture recovery is not real Task survival, and technical bootstrap is not product UX approval. Full Artifact/Compare/Flow work requires separate operable prototype and UX validation. Bootstrap includes local commits and Cargo.lock; LICENSE/NOTICE are now Apache-2.0/Copyright 2026 dio by explicit approval. GitHub creation/push, visibility and privileged environment changes still require their own authorization/decision. Create BOOTSTRAP_REPORT.md only with actual implementation evidence.
