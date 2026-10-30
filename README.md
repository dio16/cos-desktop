# COS Desktop

Native GPUI bootstrap implementation. Run `cargo run --locked -p cos-desktop -- --fixture` for the explicitly marked fixture shell; default startup honestly reports that no Runtime is connected. Full build/test gates are in [TESTING](docs/development/TESTING.md); actual commands and startup evidence are in [BOOTSTRAP_REPORT](docs/development/BOOTSTRAP_REPORT.md).

## Repository identity

| Item | Value |
|---|---|
| Project | COS Desktop |
| Repository | [`dio16/cos-desktop`](https://github.com/dio16/cos-desktop) |
| Runtime | COS Runtime, maintained in a separate repository |

```text
COS Desktop
    ↓
Versioned Desktop Protocol
    ↓
COS Runtime
    ↓
COS Domain
```

Direct dependencies on Runtime internal crates, databases, artifacts or Python entry points are forbidden. Desktop consumes the versioned Desktop Protocol only; no particular local checkout path is required.

Branch policy: develop, repair, commit and push directly on `dev`; do not create feature/task/review branches. Follow the [development-branch policy](docs/integration/REFINE_TASKERN_EXECUTION_BOUNDARY.md#development-branch). Stable `main` promotion through a PR and release publication require separate explicit authority; independent Gate checkouts remain detached.

## Publication and historical evidence

This repository contains publication snapshots of the stable `main` and development `dev` branches. Their code baselines remain distinct; publication does not promote unfinished development work to `main`.

The predecessor repository and its original Git history remain private. Raw build logs, environment captures, screenshots, approval receipts and private review sessions are intentionally excluded. See [historical evidence and verification limits](docs/development/evidence/README.md) before relying on a historical claim or candidate commit ID. Publication changes no product contract, task disposition or approval requirement.

Documentation uses generic path placeholders instead of an individual workstation's directories. Substitute your own local paths; Desktop must never depend on a Runtime source checkout.


## Canonical design and status language

The sole full product/UX canon is [COS_DESKTOP_PRODUCT_UX_ARCHITECTURE.md](docs/architecture/COS_DESKTOP_PRODUCT_UX_ARCHITECTURE.md). It was moved unchanged from the root; the root no longer contains an independent copy. Physical repository details remain in [repository architecture](docs/architecture/REPOSITORY_ARCHITECTURE.md). The [Desktop Design Canon](docs/architecture/DESKTOP_DESIGN_CANON.md) owns cross-cutting Desktop responsibilities and decisions, and [Desktop Design Contracts](docs/architecture/DESKTOP_DESIGN_CONTRACTS.md) owns component behavior and acceptance. The implementation plan translates these contracts to gated work; it is not a second design authority.

All companion documents use:

- **Observed:** directly inspected source/environment/state, with dated evidence; not a claim of runtime execution.
- **Accepted:** policy decided for subsequent work, not implemented behavior.
- **Proposed:** draft/candidate needing validation or further decision.
- **Deferred:** outside documentation/bootstrap scope as specified.

## Read first

1. [Desktop Design Canon](docs/architecture/DESKTOP_DESIGN_CANON.md): architectural authority, cross-layer roles, ownership, state/transport boundaries and DD approval registry (**design source**).
2. [Desktop Design Contracts](docs/architecture/DESKTOP_DESIGN_CONTRACTS.md): 62 canonical `DC-MP-*` behavior/API/failure/acceptance contracts (**design source**).
3. [Implementation master plan](docs/development/IMPLEMENTATION_MASTER_PLAN.md): **derived execution index** with priorities, DAG, readiness/gates, evidence and crosswalks; does not own design and grants no implementation.
4. [Documentation preparation report](docs/development/DOCUMENTATION_PREPARATION_REPORT.md): actual results, preservation, limitations and decisions.
5. [Getting started](docs/development/GETTING_STARTED.md): prerequisite observations and safe entry order (historical tables + 2026-10-08 current-status banner).
6. [Bootstrap plan](docs/development/BOOTSTRAP_PLAN.md): the ten-stage order that produced the current workspace; already executed, not authorization to start new work.
7. [Development rules](AGENTS.md): ownership, boundaries and completion requirements.

## Architecture

- [Desktop design canon: authority, interfaces, component roles, DD gates](docs/architecture/DESKTOP_DESIGN_CANON.md)
- [Canonical component specifications and acceptance (62 DC-MP contracts)](docs/architecture/DESKTOP_DESIGN_CONTRACTS.md)
- [Repository ownership and UI state](docs/architecture/REPOSITORY_ARCHITECTURE.md)
- [Direct dependency allowlist](docs/architecture/DEPENDENCY_RULES.md)
- [Desktop Protocol Draft](docs/architecture/DESKTOP_PROTOCOL.md)
- [Process/thread/security/diagnostics model](docs/architecture/PROCESS_MODEL.md)
- [Rendering/performance/visual technical companion (Accepted direction; Proposed calibrated details)](docs/architecture/RENDERING_PERFORMANCE_VISUAL_ARCHITECTURE.md)

## Runtime integration

- [Current COS Runtime survey](docs/integration/CURRENT_COS_RUNTIME_SURVEY.md)
- [Required Runtime changes](docs/integration/COS_RUNTIME_REQUIRED_CHANGES.md)
- [Protocol single-source ownership](docs/integration/PROTOCOL_OWNERSHIP.md)
- [Local development and cross-repository workflow](docs/integration/LOCAL_DEVELOPMENT_TOPOLOGY.md)
- [Refine/Taskern public execution boundary and development-task mapping](docs/integration/REFINE_TASKERN_EXECUTION_BOUNDARY.md) — source/CLI-qualified, development use distinct from future COS product integration

## Development and decisions

- [Implementation master plan](docs/development/IMPLEMENTATION_MASTER_PLAN.md) — **derived execution plan** with 62 contract pointers, milestones, dependency DAG, R0–R10 and RC-01–17 crosswalks, readiness audit
- [Adopted implementation audit](docs/development/IMPLEMENTATION_STATUS_AUDIT.md) and [ADR-0014 scope/resumption decision](docs/decisions/ADR-0014-YAGNI-SCOPE-AND-RESUMPTION.md) — current Caller permissions, image-first initial-release scope, bounded Decode and retained deferrals; historical verification is not upgraded by approval
- [Design canon normalization review](docs/development/DESIGN_CANON_NORMALIZATION_REVIEW.md) — docs-only migration verification, reviewer limitations, contract/DAG/gate preservation
- [Planned build/run procedure](docs/development/BUILD_AND_RUN.md)
- [Testing/quality/CI acceptance](docs/development/TESTING.md)
- [Exact Tern binary and current COS rendering evidence](docs/development/TERN_BINARY_RENDERING_ANALYSIS.md)
- [Architecture/plan finalization and fresh Windows evidence](docs/development/ARCHITECTURE_PLAN_FINALIZATION_REPORT.md) — quality gates, native smoke, endpoint limits and open approvals
- [Projection/graph/COW baseline measurements](docs/development/PROJECTION_MEASUREMENT_2026-10-08.md) — public APIs, source hashes, corpus scaling and memory limits; no algorithm change
- [ADR-0001 GPUI](docs/decisions/ADR-0001-GPUI.md)
- [ADR-0002 Repository separation](docs/decisions/ADR-0002-REPOSITORY-SEPARATION.md)
- [ADR-0003 Process separation](docs/decisions/ADR-0003-PROCESS-SEPARATION.md)
- [ADR-0004 Protocol boundary](docs/decisions/ADR-0004-PROTOCOL-BOUNDARY.md)
- [ADR-0005 Local transport](docs/decisions/ADR-0005-LOCAL-TRANSPORT.md)
- [ADR-0006 Media boundary](docs/decisions/ADR-0006-MEDIA-BOUNDARY.md)
- [ADR-0007 Bounded GPUI presentation and artifact-specific rendering](docs/decisions/ADR-0007-RENDERING-PERFORMANCE-VISUAL-ARCHITECTURE.md)

## Boundaries and proof limits

Desktop owns presentation/client; Runtime owns canonical Task/Artifact/Evaluation/Graph/Plan/Outcome/stop semantics. Desktop never reads Runtime DB/CAS/internal configuration or imports internal crates. Existing frontend APIs are reused through runtime-owned conversion rather than ignored.

Bootstrap uses explicitly selected, visibly marked fixture data. No fallback from failed real connection to mock. Native window/shell/fixture/recovery tests cannot establish real Task continuity, real server authentication or real media integration. Full Artifact/Compare/Flow implementation needs separate interactive prototype and UX validation.

