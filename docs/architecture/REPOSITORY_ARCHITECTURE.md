# Repository architecture

Status: **Accepted** physical ownership and constraints; workspace in **Observed** bootstrap implementation. 2026-09-21.

## Authority and applicability

The sole product/UX canon is [COS_DESKTOP_PRODUCT_UX_ARCHITECTURE.md](COS_DESKTOP_PRODUCT_UX_ARCHITECTURE.md). Its text is unchanged. Its sections 25–26 describe logical responsibilities, not permission to create a monorepo. This document instantiates physical ownership without replacing product semantics. The enduring invariants are Artifact-first, Task-centric, Evidence-first, Progressive Disclosure, Canonical truth outside UI, Process-separated, Main-thread non-blocking, Capability-driven, Local-first, Replaceable boundaries, Recoverable, Explainable.

Creative Task → Artifact → Evaluation → Evidence → Refinement → Iteration → Formal Outcome remains the meaning chain. Desktop displays projections and sends commands; it does not become the canonical owner.

| Repository | Exclusive responsibility |
|---|---|
| Runtime: COS Runtime (separate repository) | REOS; COS Domain; Creative Task canonical semantics; Versioned DMLG; Graph Kernel; Immutable Execution Plan; Worker Runtime; Artifact canonical identity/CAS; Evaluation; Evidence; Refinement semantics; Provenance; Knowledge; Formal Outcome; Persistence; runtime authority; future cosd; Desktop-facing canonical projections; protocol server |
| Desktop: `<desktop-root>`, target `dio16/cos-desktop` | Native GPUI app; shell; Work Surface; Artifact presentation; Compare UX; Flow/DMLG visualization; Inspector; Creative Loop Dock; Command Palette; Desktop-local state; UI Model; COS client; media presentation/cache; OS integration; windows; installer/updater; diagnostics; accessibility; native performance; independent release lifecycle |

Never move Task Status, Formal Outcome, Evaluation result, Best candidate/Winner, Formal selection, GraphRevision, ExecutionPlan, Artifact canonical identity or Stop completion authority into Desktop. Do not create `cosd`, `artifact-service`, or `provider-adapters` in this repository. Desktop's future `media` crate is presentation/cache, not the canonical artifact service.

Forbidden: monorepo conversion; Runtime submodule; Desktop beneath Runtime; merged histories/releases; direct dependencies on Runtime internal crates; Runtime DB/artifact DB/internal config access; launching internal Python using a hardcoded checkout path. `<runtime-root>` in historical development instructions is illustrative, never a production endpoint.

## Implemented minimal shape

Implemented workspace members: `apps/desktop`, `crates/desktop-gpui`, `crates/ui-model`, `crates/app-core`, `crates/cos-client`, `crates/desktop-protocol`. `crates/media` and `crates/platform` remain deferred and are not scaffolded. Root also has Apache-2.0 LICENSE/NOTICE, toolchain/config, fixtures, scripts, Windows CI, and docs. `Binary package name is cos-desktop`; actual invoked selector is `-p cos-desktop`, not the earlier planned `-p desktop`.

| Planned path | Responsibility |
|---|---|
| `apps/desktop` | Entry point, configuration validation, dependency injection/composition |
| `crates/desktop-gpui` | GPUI shell, widgets, input/focus, rendering, GPU texture/resource lifetime |
| `crates/ui-model` | Pure presentation state and projection-to-view adaptation; no GPUI |
| `crates/app-core` | Navigation, command orchestration, preferences, capabilities, connection lifecycle; no GPUI |
| `crates/cos-client` | Transport-independent client interface and observation/recovery; fixture provider behind the same boundary |
| `crates/desktop-protocol` | Versioned wire contract only, not Domain objects |
| `crates/media` (Deferred) | Bounded media retrieval/decode/cache, not GPU resources or canonical identity |
| `crates/platform` (Deferred) | OS integration, credential-store interfaces, platform adapters; no Domain semantics |

Responsibility order: `cos-desktop` binary → desktop-gpui → ui-model → app-core → cos-client → desktop-protocol. This is not an adjacency-only dependency rule. [DEPENDENCY_RULES.md](DEPENDENCY_RULES.md) alone defines allowed direct workspace edges. Adding cos-client isolates transport, not Domain authority.

## State ownership

Implemented: `ui-model::presentation::Presentation` owns Viewed and compare pair; app-core owns navigation/preferences/client lifecycle. Presentation holds typed `CandidateRef` values from `desktop-protocol` as local window pointers; Runtime Best-so-far/Formal Selected remain read-only projections. Projection data is separately typed/read-only to presentation consumers; use protocol types rather than copying Domain models.

| Meaning | Owner / behavior |
|---|---|
| Viewed | Window-local candidate being inspected; switching never sends formal selection |
| Compare Selected | Temporary pair; no ranking or selection authority |
| Best-so-far | Runtime projection; never computed by sorting UI scores |
| Formal Selected | Runtime Outcome projection; cannot be inferred from Viewed, compare pair or best-so-far |

Keep Task Status, Inner Phase, Iteration, Artifact Availability, Connection and Formal Outcome as independent typed axes. A stale connection leaves last-known canonical values labelled stale; it cannot turn ACTIVE into FAILED. A sent/accepted Stop command displays pending intent, not completed stop. Only Runtime projection establishes stop completion. New candidates/iterations must not steal focus, scroll, zoom, selected artifact or compare state. Removed identities become an explicit unavailable view rather than silently selecting a winner. Desktop preferences cannot persist authoritative Task/Outcome/Evaluation truth or plaintext credentials.

## Canonical interpretation and migration ledger

| Current reality / canon | Target | Gap | Migration path |
|---|---|---|---|
| Canon section 25 places Desktop and runtime responsibilities in one workspace diagram | Two independent repositories | Logical diagram could be mistaken for physical monorepo | Preserve diagram; apply ownership table and dependency rules here |
| Canon observation example uses `last_seen_revision` | Scoped stream cursor separate from per-projection revisions | One number cannot safely identify multiple projections/restarts | [Protocol Draft](DESKTOP_PROTOCOL.md) defines epoch/scope/cursor and atomic observation; server adapter required |
| Canon names Rust headless cosd | Independent runtime-owned cosd | Current implementation is surveyed separately, not assumed to be that daemon | Reuse current authoritative boundaries, request runtime-owned conversion; do not embed Python or Domain crates |
| Canon sections 65/67 require interactive prototype and UX validation | Product UX approved before full Artifact/Compare/Flow work | A technical window/shell does not prove UX | Bootstrap is boundary validation only; schedule separate operable prototype and UX review before production surfaces |
| Canon product MVP includes media, outcome workflow and packaging | Complete product acceptance remains intact | Bootstrap is deliberately smaller, not a replacement MVP | [Bootstrap plan](../development/BOOTSTRAP_PLAN.md) separates technical gates from later product work |

Actual runtime facts and remaining gaps are in [the survey](../integration/CURRENT_COS_RUNTIME_SURVEY.md), not inferred from this target. Bootstrap success never means product UX approval or completion of canonical UX validation.
