# TESTING — COS Desktop

- **Status:** Accepted quality policy; gates and coverage in **Observed** bootstrap implementation; full native/GPU matrix remains Proposed/Deferred below. §9 (added 2026-10-08) records post-bootstrap rendering/performance/visual validation ownership.
- **Date:** 2026-09-21 (bootstrap gates and layers); §9 addendum 2026-10-08
- **Scope:** Bootstrap tests are offline/fixture-only and never require <runtime-root>. Separately authorized future integration tests use real Runtime endpoints, never direct DB access.
- **Inputs:** [repository architecture](../architecture/REPOSITORY_ARCHITECTURE.md), [dependency rules](../architecture/DEPENDENCY_RULES.md), [protocol draft](../architecture/DESKTOP_PROTOCOL.md), [ADR-0001](../decisions/ADR-0001-GPUI.md), canon sections 63–66.

## Status vocabulary

| Marker | Meaning |
|---|---|
| **[Observed]** | Verified on this workstation by command output or file read on 2026-09-21. |
| **[Proposed]** | Recommended; awaits the stated gate. |
| **[bootstrap-after]** | Executable only after toolchain install + workspace creation + committed lockfile. |
| **[Deferred]** | Not in bootstrap; trigger stated. |
| **[Unverified]** | Upstream fact not yet proven locally. |

## 1. Exact quality gates **[Observed for bootstrap crates; command evidence in BOOTSTRAP_REPORT]**

Run from the repository root, in this order; every command uses `--locked` where supported. Binary package selector: `cos-desktop`, not the older planned `desktop`.

**Inner edit loop, not a completion route:** `pwsh scripts/quality.ps1 -CheckOnly` runs formatting, metadata, dependency checker/self-test and `cargo check --locked --workspace --all-targets`. Clippy, test execution, normal build and Native smoke are explicitly `NOT_RUN`; they remain required at completion. The no-switch invocation retains every declared gate. Profiles and cache/target handling follow [BUILD_AND_RUN §4.1](BUILD_AND_RUN.md#41-storage-bounded-development-workflow).

| # | Gate | Command | Failure policy |
|---|---|---|---|
| 1 | Formatting | `cargo fmt --all -- --check` | Fix formatting; never merge with a failing fmt gate. Requires the `rustfmt` component. |
| 2 | Resolution / metadata | `cargo metadata --locked --no-deps --format-version 1` | Proves the manifest and lockfile parse and resolve deterministically; input for the dependency-rules check (see §4). |
| 3 | Lints | `cargo clippy --locked --workspace --all-targets -- -D warnings` | Warnings are errors for all workspace crates (dependencies are built but not linted). |
| 4 | Tests | `cargo test --locked --workspace` | All unit/integration tests across the workspace. |
| 5 | Build | `cargo build --locked --workspace` | The ordinary build must succeed with the pinned toolchain and pinned sources. |
| 6 | Native startup smoke | `cargo run --locked -p cos-desktop -- --fixture` and plain `cargo run --locked -p cos-desktop` (Windows) | Window opens, renders, responds to keys/buttons/close cleanly; fixture mode captures screenshot/AX and explicit Fixture indicator; default startup honestly reports Runtime not connected. Manual gate during bootstrap; see §6 for automation posture. |
Operational rules:

- The lockfile is committed; when a legitimate dependency change occurs, refresh the lock once without `--locked`, review the diff, commit it, and return to `--locked` for all gates.
- If the global environment carries unexpected flags (see [BUILD_AND_RUN](BUILD_AND_RUN.md) §5), the gate result is only valid with the environment recorded alongside it.
- Each gate run records: exact command, toolchain version, resolved revision, and result. A failure report without the exact command is not actionable and is rejected in review.

## 2. Test layers and ownership

Each layer lives in its behavior owner; cross-layer tests use the mock client without forbidden dev-dependencies. Assert observable contracts, not wiring/defaults/mock echoes; follow [dependency rules](../architecture/DEPENDENCY_RULES.md).

| Layer | Owner crate | Must assert (examples) | Must not do |
|---|---|---|---|
| **Ownership** | `ui-model` (with `app-core` contracts) | Viewed candidate changes never emit a formal-selection command; compare pair never implies ranking; `Best-so-far` and `Formal Selected` render from runtime projections only; stale connection keeps last-known canonical values, labeled stale, and never turns ACTIVE into FAILED; removed identities become an explicit unavailable view; a sent/accepted Stop shows pending intent until a canonical projection confirms completion; new candidates/iterations do not steal focus/scroll/zoom/compare state. | Re-implement scoring, ranking, or outcome logic in the UI; derive canonical values by sorting UI state. |
| **Protocol serde** | `desktop-protocol` | Golden JSON round-trips for every projection/envelope; required-field and reference validation; integer counters stay decimal strings; UTF-8 JSON only, no base64 media in control frames; size/bounds policy (1 MiB max frame, 8 MiB max staged bundle, 256-frame / 8 MiB queue) rejects declared-oversize input before allocation; unknown-field behavior per protocol §5 (ignored when non-critical, rejected when they would bypass validation). | Treat fixture snapshots as runtime guarantees; invent fields not in the protocol document. |
| **Version negotiation** | `desktop-protocol` | Ranges are `{major, min_minor, max_minor}` with inclusive bounds; highest mutually supported pair is selected and fixed for the session; disjoint ranges produce `VersionIncompatible` with no side effects; multiple majors are separate entries; unknown state-changing events trigger incompatibility/resync while unknown informational events may be skipped without losing cursor continuity; version equality of app/daemon is never required. | Guess a compatible version; silently downgrade; auto-bump versions. |
| **Mock runtime** | `cos-client` | Atomic observation: snapshot staged off-model, completeness/digest checked, single swap, cursor advances only after full application; delta transactions apply atomically with `base_revision`/`new_revision`, duplicates ignored, same-cursor-conflicting-content is a protocol error; gap/out-of-order/missing predecessor → `ResyncRequired` (full snapshot path, not a partial fallback); overflow cancels the stream, keeps the last fully applied cursor, marks stale, and backs off; epoch change or restart without durable state → full snapshot. | Reorder or repair deltas speculatively; advance the cursor on receipt; build an unbounded buffer. |
| **Fixture provider** | `cos-client` (`MockCosClient`, bootstrap plan stage 6) | The fixture provider serves scripted snapshots/events/receipts through the same client interface; malformed/unknown/gap/restart/overflow inputs are exercised without reimplementing Domain transitions; no automatic fallback from a failed real connection; fixture identities stay in the isolated `fixture:` namespace. UI-facing tests assert the persistent **Fixture** indicator and that unavailable commands are explicitly unavailable, never a fake success. | Let components read fixture files directly; fabricate canonical Outcome/stop completion; treat fixture success as runtime proof. |
| **Reconnect** | `cos-client` | Connection state machine `disconnected → negotiating → synchronizing → live`, with `stale/reconnecting` and `incompatible/unauthorized` outcomes; backoff with jitter; local disconnect cancels observation only, never Task execution; mutating commands are not auto-resent without a valid dedupe guarantee, and `ResultUnknown` is surfaced rather than resolved into success/failure. | Treat transport failure as a Task failure; replay commands on reconnect; generate a new command id to hide a lost response. |
| **Navigation** | `app-core` | Command routing and navigation transitions with a mock client; view/selection state transitions for the shell; connection lifecycle driving capability gating (unknown capability = disabled). All headless, no GPUI. | Contain any GPUI import or any Domain semantics. |
| **GPUI component** | `desktop-gpui` | Widget-level behavior on the native path using the framework’s own test support (`gpui` `test-support` / `TestAppContext`; upstream README documents the test macro family **[Unverified]**) — focus/interaction/layout behaviors as they are implemented. Windows-first. | Encode canon semantics into widget tests; assert pixels as semantic proof. |
| **Startup smoke** | `apps/desktop` | Binary starts on Windows, opens one window, renders, and exits cleanly with the pinned toolchain/revision (gate 6). | Substitute for product acceptance (canon §65–66). |

Boundary note: `desktop-gpui` tests need the pinned GPU path and therefore run on Windows; `desktop-protocol`, `cos-client`, `app-core`, and `ui-model` must stay GPUI-free so their tests also run on Linux.

## 3. Platform matrix

| Platform | Scope | Rationale |
|---|---|---|
| **Windows (primary)** | All gates including GPUI component tests and the native startup smoke. | The bootstrap product target and the only platform where the GPUI/Direct3D path is exercised. |
| **Linux (optional, not baseline)** | Pure-Rust crates only if a concrete portability need justifies the lane | Do not create the lane by default or expand the matrix without evidence of need. |
| **macOS** | Out of scope for bootstrap. | No target decision or hardware; revisit only if the product target expands. |

## 4. Dependency-rules checking **[Observed implementation]**

`scripts/check_dependency_rules.py` enforces [DEPENDENCY_RULES.md](../architecture/DEPENDENCY_RULES.md) from `cargo metadata`, across normal/dev/build kinds, every target, renames, cycles, GPUI isolation, runtime-checkout paths, git pins/revision unity and reviewed external sources. `scripts/tests/test_check_dependency_rules.py` executes the required negative/positive regression examples. `scripts/quality.ps1` gates 1–5 in order. Metadata alone is still not the allowlist: gates run both the metadata parse and the checker/self-test.

## 5. Determinism rules

- No network, no live runtime, no `<runtime-root>` path, no database. The mock runtime in `cos-client` is the only counterpart.
- Golden fixtures are small, committed, and reviewed like code; they live with `desktop-protocol` tests.
- No reliance on wall-clock sleeps; use explicit stream/cursor control and, where needed, mock time.
- Bounded everything: frame/bundle sizes, retry counts, backoff caps — oversized input is rejected before allocation, per the protocol’s bounds.
- Failures must print the exact command and the observed vs expected behavior; a test that cannot explain a plausible bug it would catch does not belong in the suite.

## 6. CI status **[Observed definitions; service execution deferred]**

`.github/workflows/ci.yml` is implemented as a Windows-primary pipeline with pinned toolchain, gates 1–5, metadata plus checker/self-test, no matrix expansion and contents-read permissions. **Bootstrap history (2026-09-21):** repository creation/remote/push and remote CI execution were then unauthorized, and no service-side execution was reported. That historical authorization statement does not describe the current remote. **This docs-only hardening:** service-side CI execution/status is not verified and no current CI PASS is claimed; report it separately from local document validation.

## 7. Explicitly out of scope now

- **Performance targets.** At bootstrap (2026-09-21) no performance assertion or micro-benchmark was part of the gates, and canon §37–38 targets were design goals only. **Current policy:** those design targets remain owned by [Rendering Architecture](../architecture/RENDERING_PERFORMANCE_VISUAL_ARCHITECTURE.md) §10 as calibration hypotheses — the bootstrap gates still do not enforce them, and phase-specific gates are ratified through R0–R10 ([companion §13](../architecture/RENDERING_PERFORMANCE_VISUAL_ARCHITECTURE.md#13-accepted-migration-sequence-phase-specific-validation-gates)). The 2026-10-08 rendering investigation has since produced external/native measurements ([evidence](TERN_BINARY_RENDERING_ANALYSIS.md)); they are evidence, not CI gates. Ownership map in §9.
- **Golden Creative Workflow** end-to-end tests (canon §64) — product phase, after real runtime integration.
- **Real runtime integration tests** — require runtime-owned implementation and the evidence described in `docs/architecture/DESKTOP_PROTOCOL.md` §7 (atomic snapshots, durable dedupe, authentication, restart recovery, media interoperability). Fixture tests must not be presented as those proofs.
- **Native accessibility, packaging/update certification** (canon §63) — outside bootstrap evidence. Post-bootstrap keyboard/AX/DPI/IME validation starts with R1 and continues through every affected rendering phase (§9 here); packaging/update certification remains later work.
- **Media decode/cache tests** — outside the bootstrap test suite; the future media crate remains deferred, not a prerequisite to conducting image feasibility experiments. Post-bootstrap Decode / Upload / Eviction validation is owned by [Rendering Architecture §6.1](../architecture/RENDERING_PERFORMANCE_VISUAL_ARCHITECTURE.md#61-image-feasibility-gates-r4) and its R4 exit contract in §13. No new crate, test layer or CI job is introduced by this docs-only task.

## 8. Evidence expectations

Each completed gate records command, toolchain/revision and observed result; native smoke records screenshot/log and input/close behavior. Review minimal-window evidence before workspace expansion, then review the complete suite before bootstrap completion. Do not require the future full-workspace suite before the workspace exists.

For an explicitly authorized Refine Gate, preserve the accepted ID, fixed candidate/request/configuration identity, real check outcomes, private logs and required artifacts. Close its storage lifecycle through the [Gate-only workspace disposal procedure](../integration/REFINE_TASKERN_EXECUTION_BOUNDARY.md#successful-gate-workspace-disposal): retain the pinned source under a durable authorized Git reference and verify stored evidence before/after exact workspace disposal. Integrity verification is not a build or live-source revalidation. Failed/recovery material and delegated execution/Refinement checkouts are excluded; raw evidence remains [private](evidence/README.md#public-and-private-material).

## 9. Post-bootstrap rendering / performance / visual validation **[Accepted ownership; addendum 2026-10-08]**

The gates in §1–§6 are unchanged and remain the repository-wide quality policy. This section assigns ownership so rendering/performance/visual validation does not fork into competing documents. It introduces no numeric target and copies no SLO value.

| Concern | Single owner |
|---|---|
| General repository quality gates, test layers, platform matrix, determinism, CI definitions (§1–§6 here) | **This document** |
| Performance budgets, calibration hypotheses, counter definitions, workload sizes, scheduling/cache defaults | [Rendering Architecture](../architecture/RENDERING_PERFORMANCE_VISUAL_ARCHITECTURE.md) §10 |
| Rendering/performance/visual test strategy (golden/pixel, performance, stress/recovery, native interaction/accessibility, observability) | [Rendering Architecture](../architecture/RENDERING_PERFORMANCE_VISUAL_ARCHITECTURE.md) §11 |
| Reproducible workload/manifest/instructions and evidence retention; machine-readable result/endpoint contract | [Rendering Architecture](../architecture/RENDERING_PERFORMANCE_VISUAL_ARCHITECTURE.md#101-reproducible-benchmark-contract-r0) §10.1 and [§11.1–§11.2](../architecture/RENDERING_PERFORMANCE_VISUAL_ARCHITECTURE.md#111-windows-scheduling-endpoints-and-starvation-attribution-r0) |
| Phase exit criteria, regression gates and rollback conditions (R0–R10) | [Rendering Architecture](../architecture/RENDERING_PERFORMANCE_VISUAL_ARCHITECTURE.md) §13 |
| Measured evidence (Release baseline, native/probe measurements, decode capability matrix) | [Tern binary rendering analysis](TERN_BINARY_RENDERING_ANALYSIS.md) |
| Architecture decision (GPUI shell plus narrowly scoped artifact/graph rendering; pin KEEP) | [ADR-0007](../decisions/ADR-0007-RENDERING-PERFORMANCE-VISUAL-ARCHITECTURE.md) |

Operational rules:

- Bootstrap gates do **not** enforce any performance SLO; they enforce formatting, resolution/metadata, lints, tests, build and native startup smoke.
- The 2026-10-08 rendering investigation produced external/native measurements outside the CI suite (temporary probe and decoder harnesses). Those measurements are evidence; they do not automatically become CI gates, thresholds or release criteria.
- A rendering/performance/visual phase becomes completion-eligible only through that phase's exit evidence and regression gate in the companion §13, ratified after R0.
- Changing a numeric target means editing the companion §10 only; other documents link to it instead of restating it.
- Rendering/performance work must not weaken §1: a change that fails fmt, metadata/dependency checks, clippy, tests, build or native startup smoke is rejected regardless of benchmark results.
- Future R0 must retain replayable definitions, exact invocation/fixture/reset instructions and versioned non-sensitive results with benchmark hashes; historical temporary probes do not establish that workflow. The contract is docs-only now: no benchmark tool, source/test/CI implementation or numeric SLO is added here.
- Native Present/input stress and continuous visual equivalence use the companion §11.1/§11.3; absent features and unsupported endpoints remain explicitly unmeasured. A phase cannot pass by dropping keyboard/IME/AX behavior, required information, fidelity or visible updates.
