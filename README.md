# COS Desktop

Native GPUI bootstrap implementation. Run `cargo run --locked -p cos-desktop -- --fixture` for the explicitly marked fixture shell; default startup honestly reports that no Runtime is connected. Full build/test gates are in [TESTING](docs/development/TESTING.md); actual commands and startup evidence are in [BOOTSTRAP_REPORT](docs/development/BOOTSTRAP_REPORT.md).

## Repository identity

| Item | Value |
|---|---|
| Project | COS Desktop |
| Repository | `dio16/cos-desktop` |
| Runtime repository | COS Runtime (separate repository) |

```text
COS Desktop
    ↓
Versioned Desktop Protocol
    ↓
COS Runtime
    ↓
COS Domain
```

Direct dependency on COS Runtime internal crates, its database, its artifacts or its Python entry points is forbidden. Desktop consumes the versioned Desktop Protocol only.

Branch policy: `feature/*` → `dev` → `main` (PR into `main`). Stable baseline is `main`; day-to-day development happens on `dev`.

## Canonical design and status language

The sole full product/UX canon is [COS_DESKTOP_PRODUCT_UX_ARCHITECTURE.md](docs/architecture/COS_DESKTOP_PRODUCT_UX_ARCHITECTURE.md). It was moved unchanged from the root; the root no longer contains an independent copy. Physical repository details are in [repository architecture](docs/architecture/REPOSITORY_ARCHITECTURE.md), not edits to the canon.

All companion documents use:

- **Observed:** directly inspected source/environment/state, with dated evidence; not a claim of runtime execution.
- **Accepted:** policy decided for subsequent work, not implemented behavior.
- **Proposed:** draft/candidate needing validation or further decision.
- **Deferred:** outside documentation/bootstrap scope as specified.

## Read first

1. [Documentation preparation report](docs/development/DOCUMENTATION_PREPARATION_REPORT.md): actual results, preservation, limitations and decisions.
2. [Getting started](docs/development/GETTING_STARTED.md): prerequisite observations and safe entry order.
3. [Bootstrap plan](docs/development/BOOTSTRAP_PLAN.md): future ten-stage implementation, not authorization to start.
4. [Development rules](AGENTS.md): ownership, boundaries and completion requirements.

## Architecture

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

## Development and decisions

- [Planned build/run procedure](docs/development/BUILD_AND_RUN.md)
- [Testing/quality/CI acceptance](docs/development/TESTING.md)
- [Exact Tern binary and current COS rendering evidence](docs/development/TERN_BINARY_RENDERING_ANALYSIS.md)
- [ADR-0001 GPUI](docs/decisions/ADR-0001-GPUI.md)
- [ADR-0002 Repository separation](docs/decisions/ADR-0002-REPOSITORY-SEPARATION.md)
- [ADR-0003 Process separation](docs/decisions/ADR-0003-PROCESS-SEPARATION.md)
- [ADR-0004 Protocol boundary](docs/decisions/ADR-0004-PROTOCOL-BOUNDARY.md)
- [ADR-0005 Local transport](docs/decisions/ADR-0005-LOCAL-TRANSPORT.md)
- [ADR-0006 Media boundary](docs/decisions/ADR-0006-MEDIA-BOUNDARY.md)
- [ADR-0007 Bounded GPUI presentation and artifact-specific rendering](docs/decisions/ADR-0007-RENDERING-PERFORMANCE-VISUAL-ARCHITECTURE.md)

## Publication disclosure

Historical raw evidence is intentionally withheld; see [evidence and publication boundary](docs/development/evidence/README.md). This disclosure distinguishes historical records from the current publication snapshots. The stable and development branches retain their distinct code baselines; publication does not promote development product code to this stable branch.

## Boundaries and proof limits

Desktop owns presentation/client; Runtime owns canonical Task/Artifact/Evaluation/Graph/Plan/Outcome/stop semantics. Desktop never reads Runtime DB/CAS/internal configuration or imports internal crates. Existing frontend APIs are reused through runtime-owned conversion rather than ignored.

Bootstrap uses explicitly selected, visibly marked fixture data. No fallback from failed real connection to mock. Native window/shell/fixture/recovery tests cannot establish real Task continuity, real server authentication or real media integration. Full Artifact/Compare/Flow implementation needs separate interactive prototype and UX validation.

