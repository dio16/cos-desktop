# TESTING — COS Desktop

- **Status:** Accepted test-quality and ownership policy. Commands and workflows are listed with their current scope; native/platform behavior and external Runtime evidence are reported only when actually exercised.
- **Date:** 2026-09-21 (bootstrap gates); 2026-10-08 (test policy); 2026-10-11 (existing-Agent / independent-Gate workflow)
- **Scope:** Offline tests use deterministic fixtures and mocks, never `<runtime-root>`, network access, or direct Runtime database access. Real Runtime integration requires explicit authorization and Runtime-owned implementation.
- **Inputs:** [repository architecture](../architecture/REPOSITORY_ARCHITECTURE.md), [dependency rules](../architecture/DEPENDENCY_RULES.md), [protocol draft](../architecture/DESKTOP_PROTOCOL.md), [ADR-0001](../decisions/ADR-0001-GPUI.md), canon sections 63–66.

## 1. Quality gates and impact routes

The repository keeps one declared quality workflow; adding a test category does not imply another CI job or platform lane. FAST feedback runs the relevant sequence from the existing workflow: `cargo fmt --all -- --check`; `cargo metadata --locked --no-deps --format-version 1`; `python -B scripts/check_dependency_rules.py --self-test`; `python -B scripts/check_dependency_rules.py`; `cargo clippy --locked --workspace --all-targets -- -D warnings`; affected tests (for example, `cargo test -p app-core local_disconnect_never_sends_stop_implicitly`); and `cargo build --locked --workspace`. The aggregate Windows entry point is `pwsh scripts/quality.ps1`, which runs the repository's declared gates in order. It resolves and proves a concrete Python interpreter and a concrete `cargo` before any gate runs (a bare `python` or an extensionless launcher is not proof on Windows), prints both resolved paths, and fails closed with an explicit message when a tool cannot be proven rather than skipping the gate; [BUILD_AND_RUN §4.14](BUILD_AND_RUN.md#414-mp-img2-gdi-comparison-probe-and-fail-closed-gate-entry-point-2026-10-24) records the details and [§4.11](BUILD_AND_RUN.md#411-fixed-interpreter-and-environment-rules-2026-10-24) the interpreter rules. The Windows native startup smoke is specified in §6. CI configuration and service-side execution status are distinct evidence; do not report a PASS unless observed.

**Inner edit loop (not a completion route):** `pwsh scripts/quality.ps1 -CheckOnly` runs format, metadata, dependency checker/self-test and `cargo check --locked --workspace --all-targets`. It deliberately does not run clippy, tests, normal build or Native smoke, and reports those as `NOT_RUN`. Use this or a crate-scoped `cargo check` for frequent feedback; it does not replace FAST/full quality, independent Gates, effect-specific tests or Native evidence. The no-switch invocation retains all existing gates.

| Route | Scope and impact |
|---|---|
| **FAST** | Relevant format, metadata, dependency checker/self-test, clippy, affected tests and normal build from the existing quality workflow; do not create separate jobs. |
| **INTEGRATION** | Offline fixture/mock flows across public crate seams; must not impersonate Runtime evidence. |
| **NATIVE** | Windows native UI/GPU/input/accessibility checks for each affected path and the declared full matrix where required. |
| **PERFORMANCE** | Reproducible workload and measurement workflow owned by [Rendering Architecture §10–11](../architecture/RENDERING_PERFORMANCE_VISUAL_ARCHITECTURE.md). |
| **RELEASE** | Full declared integration and native/platform matrix applicable to the release, with required evidence and reviewed failures. |
| **EXTENDED** | Explicitly scheduled long-running stress, recovery, or broader environmental checks; never silently omitted while reporting that route as passing. |

Routes describe impact and evidence, not a numeric test quota. E01 requires effect-specific native/visual regression checks on each changed native path. It does not require a full AX/IME/DPI/GPU matrix for every minor internal change; run the full declared matrix for cross-surface integration and release. Performance A/B comparisons must preserve equivalent quality and features; degraded modes are separate scenarios, not equivalent successful results. Performance routes do not substitute for correctness or native UX. The rendering strategy and its comparison/evidence contract are owned by [Rendering Architecture §11.3](../architecture/RENDERING_PERFORMANCE_VISUAL_ARCHITECTURE.md#113-continuous-visual-equivalence-gate).

**Documentation-only route:** verify changed-document relative links/anchors, the 62 MP↔DC mappings, exact hard prerequisites/DAG, status/card/table correspondence and preservation of decision/authorization gates. Record the diff, fixed review SHA and observed branch/worktree. Cargo/Windows/native/performance checks are `NOT_RUN` when no executable behavior changes; do not present historical results as reruns. Changes to executable commands/contracts must also exercise the affected public CLI or check. This route does not exempt code changes from the existing quality workflow or required effect-specific native evidence.

## 2. Test admission and assertion quality

Admit a test only when its review can state all of the following:

A. **Plausible bug:** name a concrete regression it would detect.
B. **Public contract:** identify the externally observable behavior being protected.
C. **Independent oracle:** expected behavior comes from a contract, independent fixture, or separately defined invariant—not production logic copied into the test.
D. **No duplication:** it adds a distinct guarantee rather than restating an existing stronger test.
E. **Refactor durability:** it should survive internal restructuring that preserves the contract.
F. **Why unit:** explain why a deterministic, cheap boundary unit is preferable or necessary; otherwise exercise the owning public seam at integration level.
G. **Mock boundary:** explain which real boundary behavior the mock exercises and which claims it cannot establish.
H. **Cost:** record why its maintenance/runtime cost is proportionate to the bug risk.

Integration-first is the default; there is no numeric unit-to-integration ratio. Pure deterministic, cheap boundary units are appropriate when their oracle is independent and the behavior is not better demonstrated across the public seam. Property/metamorphic tests MAY be used only when they add meaningful invariant coverage, use a reproducible seed, and need no new testing framework.
Examples of admissible cheap boundary units: protocol serde tests that assert exact public wire/error behavior without reproducing the production serializer; negotiation boundary cases for inclusive ranges and incompatibility; and deterministic bound checks that reject declared-oversize input before allocation. They are justified because the boundary contract is pure, cheap, and independently specified. Prefer integration when behavior crosses ownership boundaries; unit scope does not excuse testing private helper call counts.

Useful property/metamorphic cases include canonical wire round-trip stability for valid protocol values, malformed required-reference mutation remaining rejected, and cursor/revision invariant preservation under duplicate or reordered fixture inputs. Use fixed, reported seeds and bounded generated cases; these examples do not justify a new property-testing dependency or duplicate existing fixture integration coverage.

Do not admit production-clone oracles; getter/setter echo checks; trivial-constructor checks; derive checks; private-helper call counts; intermediate-state assertions without a public contract; container/source-text inspection; mock-echo assertions; unbounded snapshots; incidental logs; coverage farming; or skipped/conditional cases reported as passing. Exact wire bytes/fields, error identity, and identity/reference comparisons are allowed where they are themselves the contract. A skipped, unavailable, not implemented, not run, or not measured case MUST be reported separately and MUST NOT count as a pass.

## 3. Ownership, test location, and assertions

Place tests with the behavior owner. Assert externally observable contracts, not implementation wiring/defaults or mock echoes.

| Behavior | Single owner | Representative contract |
|---|---|---|
| Protocol serialization, validation, negotiation | `desktop-protocol` | Golden wire forms, required-field/reference validation, decimal-string counters, UTF-8 control frames, declared bounds, and unknown-field policy. |
| Observation, command, receipt, fixture and recovery boundary | `cos-client` | Atomic snapshots/deltas, cursor/revision invariants, bounded resync, honest receipt states, reconnect semantics and fixture isolation. |
| Application publication/routing | `app-core` | Public client seam, navigation/command behavior, fixture projection publication, disconnect behavior; headless and GPUI-free. |
| Presentation state | `ui-model` | Canonical projections rendered without re-deriving Domain semantics; local selection/focus/scroll state remains independent. |
| Native/GPU rendering and interaction | `desktop-gpui` | Windows native focus, input, layout/rendering and accessibility behavior using framework support where available. |
| Application startup/lifecycle | `apps/desktop` | Binary startup, window lifecycle and native smoke on the declared platform. |
| Real Runtime behavior | External Runtime, only when approved | Runtime-side atomicity, durable dedupe, authentication, restart recovery and media interoperability; local fixtures cannot prove these. |

Named INTEGRATION flow examples:

- `desktop-protocol` envelope/projection fixture → `cos-client` validation and atomic publication → `app-core` public snapshot/command boundary → `ui-model` presentation semantics; verify canonical identities and authority are retained without implementing a second semantic model.
- Explicit Stop intent through `app-core` → scripted `cos-client` receipt → canonical projection observation; acceptance, canonical reflection and terminal completion remain distinct.
- Connected explicit Fixture → disconnect/revocation → app-core public publication becomes unavailable until fresh exact scripted authorization; cos-client retains actual unresolved state internally and no mutating command is resent. This consumer proof establishes no real authenticated Runtime guarantee. Existing snapshot-only/local-disconnect tests migrate to the approved DC-MP-PUB1 public boundary when the separately instructed implementation cuts over.
- Reconnect/resync from fixture gap or epoch change → full observation snapshot; assert no partial repair or speculative canonical state.

Where Taskern/Refine workflow evidence is relevant, follow the [development boundary policy](../integration/REFINE_TASKERN_EXECUTION_BOUNDARY.md): Taskern plans/validates context, the existing Coding Agent implements/tests, Refine 0.3 independently checks a committed candidate, and Caller reviews and explicitly accepts/rejects. Neither Plan validity, Gate `SUCCEEDED`, integrity success, Tern review nor fixture results substitute for Caller or product/domain acceptance. MP-COR1's [accepted historical evidence](evidence/README.md#historical-records) remains delegated Refine 0.2 evidence, not a 0.3 Gate.

Examples of contract assertions include: viewing a candidate never emits formal selection; compare never implies ranking; canonical outcomes are never inferred from UI ordering; stale data remains labeled stale and does not turn ACTIVE into FAILED; removed identities become unavailable; accepted Stop remains distinct from canonical completion; and new projections do not steal local navigation state.

### 3.1 Candidate verification and repair

Use the current Skills and matching producer contracts referenced by the boundary policy rather than another test framework or copied request schema.

- Before Gate, run the affected public-contract tests and applicable repository quality/impact route in the existing Agent session. Keep independent oracles and durable regressions; neither a Taskern packet nor a Gate request may forbid contract-relevant regression tests or independent verification.
- Pin the exact candidate commit and allowed checks/outputs/environment/finite limits. Refine Gate runs those checks on an independent detached checkout, without launching an implementation agent; it does not ingest dirty/untracked changes. Unsupported FROZEN_WORKTREE is HOLD.
- Inspect native preflight diagnostics. `fail` and `unknown` are not PASS even when the CLI exits 0; runtime toolchain/SDK, output delivery and Caller authorization need their own evidence/decision. Preserve the static result rather than relabeling it.
- Keep the accepted Gate ID, request/candidate identity, per-check outcomes, bounded private stdout/stderr references, artifacts, cleanup and `gate verify` integrity result. Integrity verification does not rerun checks. Logs may contain private data/secrets; inspect/redact before external review.
- Close a successful authorized Gate's storage lifecycle using the [Gate-only workspace disposal procedure](../integration/REFINE_TASKERN_EXECUTION_BOUNDARY.md#successful-gate-workspace-disposal). Source retention is the full pinned commit under an authorized durable Git reference, not a per-Gate source archive; preserve private evidence and native integrity reports before/after exact workspace disposal. This does not change check acceptance, claim that `gate verify` revalidates live source, authorize deleting failed/recovery material or apply to delegated execution/Refinement.
- On an observed Gate failure, diagnose from its receipt/logs, repair in the **same Agent session**, rerun relevant checks and pin a **new candidate/new Gate** within the Caller grant. For a lost response, reconcile the accepted ID with `gate status`/`gate inspect`; do not blindly replay. Old failed/unknown/not-run evidence remains immutable and never counts as passing.
- Tern receives the fixed SHA/diff, design constraints and actual evidence in the existing review conversation after exact-SHA push to `origin/dev` under the [development-branch policy](../integration/REFINE_TASKERN_EXECUTION_BOUNDARY.md#development-branch). Apply valid findings with new commits on `dev` and repeat to **0 BLOCKING / 0 ACTIONABLE NON-BLOCKING / 0 EXCESSIVE**, or HOLD. Full DC acceptance and separate Caller disposition remain necessary; a candidate on `dev` does not imply acceptance, main merge, release or the next user-gated step.

## 4. Determinism, fixtures, and mocks

- Offline tests use no network, live Runtime, `<runtime-root>`, wall-clock sleeps or direct database access. Drive time/cursors explicitly or use mock time.
- Fixtures are small, reviewed, bounded inputs. Fixture snapshots are not Runtime guarantees, and fixture success is not production proof.
- Mock only the boundary required to isolate the behavior under test. A test MUST state the exercised boundary and must not claim behavior beyond it.
- Keep queues, frame/bundle sizes, retry counts and generated data bounded; reject declared oversized input before allocation where the protocol requires it.
- Failure output identifies the exact command and observed-versus-expected behavior; no test is justified by coverage percentage alone.

The fixture provider exercises the same `DesktopClient` boundary through scripted snapshots, events and receipts. It MUST NOT fabricate canonical completion, read fixture files from UI components, or silently replace a failed Runtime connection.

## 19. MP-R0A-S2B foreground observation admission

[The T01–T15 / N01–N07 admission](evidence/README.md#historical-records) owns this slice. Native observation is still pending: offline compilation, the local queue-boundary test and consumer regressions are not Native capture, and no frame, GPU, displayed or photon claim follows from them.

The independent consumer validates closed UTF-8 versioned data with duplicate/non-finite rejection, exact field sets, checked same-domain run-relative timestamps, bounded records/windows/durations, window-lifetime reuse and post-close fences, source-associated draw associations without a second counted draw, missing input window identity, explicit source/queue/retention loss scopes, and unsupported physical endpoints. A no-observation or failed-process disposition cannot become `OBSERVED`. `s2b_validate.py capture` reports wire validity only (`native_proven: false`); `bundle` replays a committed source-bound bundle.

## 5. Build, dependency, and CI posture

The repository quality workflow is the single place for metadata/dependency checking, clippy, workspace tests and build. `scripts/check_dependency_rules.py` and its regression suite enforce [DEPENDENCY_RULES.md](../architecture/DEPENDENCY_RULES.md); `scripts/quality.ps1` is the existing gate entry point. Preserve the existing workflow rather than proliferating automatically triggered category-specific jobs or platform matrices.

Build profiles, target reuse and cache policy follow [BUILD_AND_RUN §4.10](BUILD_AND_RUN.md#410-storage-bounded-development-workflow-2026-10-23). Prefer one ordinary local target and disposable, isolated verification output, not weaker completion checks. The [terminal retention procedure](../integration/REFINE_TASKERN_EXECUTION_BOUNDARY.md#build-storage-and-terminal-retention) preserves immutable evidence while distinguishing it from scratch build storage.

Windows is the primary native target. Linux pure-Rust testing MAY be run where portability value warrants it; it is not an automatic matrix expansion. macOS remains out of scope until a product target is approved. Record exact command, toolchain, resolved revision, environment relevant to the result, and observed result. A local document/code change or CI definition is not evidence of remote CI execution.

## 6. Native smoke and evidence reporting

Minimum native smoke runs from the repository root on Windows:

| Check | Invocation / exercise | Expected evidence |
|---|---|---|
| Fixture startup | `cargo run --locked -p cos-desktop -- --fixture` | Window starts, screenshot and accessibility/AX observation retained; visible Fixture indicator and fixture data are explicit. |
| Disconnected startup | `cargo run --locked -p cos-desktop` | Window starts without Runtime fallback; disconnected/unavailable state is visible and no fixture indicator or fabricated projection appears. |
| Keyboard and focus | Exercise declared primary keyboard navigation, focus traversal and return focus after activation. | Evidence of focused control and keyboard operation; unsupported/unimplemented behavior marked `NOT_IMPLEMENTED`. |
| Popup | Open, interact with and dismiss a popup using keyboard and pointer where supported. | Visible open/dismiss behavior, focus remains usable; `NOT_IMPLEMENTED` when capability absent. |
| IME | Exercise composition, commit and cancel with an available IME. | Screenshot/log or observation showing composition and final text; otherwise `NOT_MEASURED` (no IME) or `NOT_IMPLEMENTED` (unsupported). |
| DPI | Run at supported fractional/per-monitor scale and move/resize between available displays. | Scale/display and screenshot evidence; unavailable scale/display marked `NOT_MEASURED`, unsupported behavior `NOT_IMPLEMENTED`. |
| Close | Close through the native window control and verify clean exit. | Exit/close observation and retained log; close does not imply StopTask. |
| Multi-monitor | Move window across attached monitors and exercise native close/restore where available. | Monitor/scale and screenshot evidence; unavailable topology marked `NOT_MEASURED`. |

For approved MP-PUB1 implementation, additionally exercise actual GPUI consumers with a scripted live delta, revocation/unavailable transition, fresh authorized recovery and a delayed stale/derived result. Record visible Fixture labelling, shared complete publication across native consumer surfaces, independent local navigation/focus retention, responsive input during background pumping and clean close without implicit Stop. This verifies the publication path, not full Artifact/fullscreen-media/detached-window product UX or real Runtime permission. Public headless multi-subscriber tests do not replace required native observations. No new CI lane, numeric test quota or broader AX/IME/DPI certification is implied.

Keep each result explicit and separate: `NOT_IMPLEMENTED`, `NOT_RUN`, and `NOT_MEASURED` are not interchangeable and none is a pass. These smoke checks do not constitute the Golden Creative Workflow.

Native smoke and UI regression evidence are separate from Golden Creative Workflow acceptance. Golden Workflow is a product acceptance activity; native smoke does not complete or substitute for it. Likewise, fixture/mock verification is separate from Runtime caller acceptance. When an external Runtime evidence report exists, cite that report for Runtime claims; local test results alone cannot establish caller acceptance.

## 7. Deferred and external evidence

- **Golden Creative Workflow** end-to-end product acceptance requires its separately approved workflow and real Runtime integration.
- **Real Runtime integration** requires Runtime-owned implementation and evidence described in [DESKTOP_PROTOCOL.md §7](../architecture/DESKTOP_PROTOCOL.md). Until available and approved, report that boundary as unverified rather than inferring Runtime behavior.
- **Native accessibility and packaging/update certification** remain separate from bootstrap gates; native smoke records only the behavior actually exercised.
- **Media decode/GPU/production acquisition** remains deferred. MP-IMG1 introduces only local encoded acquisition/cache and representation keys; [its A–H / M01–M09 admission](evidence/README.md#historical-records) owns the affected public-seam cases. PNG signature recognition is not decode admission or a Product format promise. Native picker evidence must be actual Windows input/AX/pixels, never a mocked or unit-test substitute; absent live approval is NOT_RUN. No new CI lane or SLO is introduced.

## 8. Evidence requirements

Each completed route records exact invocation, environment/toolchain/revision, applicable matrix, result, and retained artifacts. Native evidence includes screenshot/log plus exercised inputs and close behavior when available. Distinguish test verification from caller/product acceptance, and name any limitation. Do not claim complete matrix coverage from one targeted regression or one fixture scenario.

Observed physical retention: [the evidence retention index](evidence/README.md#historical-records) records byte-verified screenshot deduplication without merging independent capture/run/event identities. Historical producer JSON and receipts remain unchanged. Removed historical capture paths resolve through that index; baseline Git revisions preserve the original path layouts.

## 9. Post-bootstrap rendering / performance / visual validation **[Accepted ownership; addendum 2026-10-08]**

The general quality gates and test policy above remain repository-wide. This section assigns rendering/performance/visual ownership without adding numeric targets or copying SLO values.

| Concern | Single owner |
|---|---|
| General quality gates, test admission, ownership, routes, determinism and CI posture (§1–§8) | **This document** |
| Performance budgets, calibration hypotheses, counter definitions, workload sizes, scheduling/cache defaults | [Rendering Architecture](../architecture/RENDERING_PERFORMANCE_VISUAL_ARCHITECTURE.md) §10 |
| Rendering/performance/visual test strategy (golden/pixel, performance, stress/recovery, native interaction/accessibility, observability) | [Rendering Architecture](../architecture/RENDERING_PERFORMANCE_VISUAL_ARCHITECTURE.md) §11 |
| Reproducible workload/manifest/instructions and evidence retention; machine-readable result/endpoint contract | [Rendering Architecture §10.1](../architecture/RENDERING_PERFORMANCE_VISUAL_ARCHITECTURE.md#101-reproducible-benchmark-contract-r0) and §11 |
| Phase exit criteria, regression gates and rollback conditions (R0–R10) | [Rendering Architecture](../architecture/RENDERING_PERFORMANCE_VISUAL_ARCHITECTURE.md) §13 |
| Measured evidence (Release baseline, native/probe measurements, decode capability matrix) | [Tern binary rendering analysis](TERN_BINARY_RENDERING_ANALYSIS.md) |
| Architecture decision (GPUI shell plus narrowly scoped artifact/graph rendering; pin KEEP) | [ADR-0007](../decisions/ADR-0007-RENDERING-PERFORMANCE-VISUAL-ARCHITECTURE.md) |

Operational rules:

- Bootstrap quality gates do not enforce a performance SLO. Performance evidence does not waive format, metadata/dependency, clippy, tests, build, or native smoke requirements.
- External/native measurements are evidence, not automatic CI gates, thresholds or release criteria.
- A rendering/performance/visual phase becomes completion-eligible only through the relevant phase exit evidence and regression gate in companion §13, ratified after R0.
- Numeric targets are owned by companion §10; other documents link rather than restate them.
- R0 retains replayable workload definitions, exact invocation/fixture/reset instructions and versioned non-sensitive results with benchmark hashes; historical temporary probes do not establish that workflow.
- Native presentation/input stress and continuous visual equivalence use companion §11.1 and §11.3; absent features and unsupported endpoints remain explicitly unmeasured. Do not pass by dropping keyboard/IME/AX behavior, required information, fidelity or visible updates.

## 10. MP-DOCK1 corrective qualification

The [R1 remote review](evidence/README.md#historical-records) supersedes the historical dock success claim. [Corrective test admission](evidence/README.md#historical-records) covers exact task/iteration-reference membership, explicit large membership, cancellation, iteration-only updates and reset. The [warning-free local quality log](evidence/README.md#historical-records) is headless evidence, not proof of visible scrolling.

Source-bound Native runs must separately establish N01 insert/removal retention, N02 10,000 explicit members with keyboard traversal beyond 20 rows and real wheel/pixel geometry, N03 equal iteration numbers across tasks, N04 iteration-only revision, N05 withholding/fresh admission, N06 authorized empty versus unavailable, and N07 absence of eager decode/ranking/remote commands. Compare uses the existing model seam; no Compare UI is introduced to manufacture evidence. Preserve source-to-binary provenance and failed runs; construction counts or AX alone cannot establish visible content.

Current disposition: [technical completion](evidence/README.md#historical-records) binds Native a11y repair product `f1d0db1406a01725ad4ff4e4ef1bef2e0f7d57e8`, [new warning-free quality](evidence/README.md#historical-records), [independent source Gate R2](evidence/README.md#historical-records) and [remote R4 PASS 0/0/0](evidence/README.md#historical-records): A/B TECHNICALLY COMPLETE, not Runtime/Product acceptance. The [source-bound Native crosswalk](evidence/README.md#historical-records) preserves N01–N07 actual pixels/AX/input/traces and all reviewed qualifications. [r4-large-explicit.json](evidence/README.md#historical-records) supplies 10,000 members across 157 iterations; [r13-revoke-large.json](evidence/README.md#historical-records) uses that admitted workload plus existing revoke/fresh-delta actions without invented delays or weaker protocol ceilings. Earlier workload/Native/control failures remain historical, never current PASS. No unchanged-source tests/Native/Gate are repeated for this documentation closure.

The actual Native regression exposed simultaneous active-descendant claims by Task Rail and Dock. [R09 admission amendment](evidence/README.md#historical-records) records the failing-before evidence and actual focused Rail/Dock/keyboard/AX verification after repair; no boolean forwarding or source-text test was added. [The Native qualifications](evidence/README.md#historical-records) retain the protocol-ineligible missing-iteration Native branch, model-only Compare proof, absent scrollbar, no-effect edge drag, one-pixel outer-bound discrepancy and exact fresh-grant/revision timing. A normal close with an input-delivery error must be reconciled against the owned child/window/exit before any retry. Native capture helpers pass the current run context explicitly; an ended `computer.run` facade must not be retained for later calls.

## 11. MP-SEC2 redaction qualification

[The A–H/S01–S08 admission](evidence/README.md#historical-records) owns the affected contracts. Free-text redaction is a fallback for the debug-only fixture trace; typed `PublicationError` categories remain the primary route and drop server/local free text. [Redaction units](evidence/README.md#historical-records) exercise credential/Bearer values, split assignments across whitespace/control characters including a sensitive key split from its separator, signed locators with query/fragment before, at and after the retained prefix, multi-token separated secrets with irrevocable rest-of-line fail-closed across 255/256/257/300-byte intervening tokens, quoted/absolute path values with quote-bound multi-word continuation, quoted/unquoted/standalone/key=value overlong-token continuation, long-first-path withholding, detached signature continuations, multi-token paths, path-like key=value fragments, opaque-identity preservation, numeric-field preservation, control-character line forging and the 512-byte complete-line bound; the stateful parser has a 256-byte token buffer and no full-input token vector. Trace tests prove disabled/exhausted routes never format and concurrent admission never exceeds the saturating quota. [The repaired-source fixture capture](evidence/README.md#historical-records) binds source `caa6313…` and its executable hash, emitted 15 bounded stderr lines (max 299 bytes including newline), uses `query_present` with no raw `query=` field, and kept the fixture JSONL trace healthy; the synthetic-sensitive-query branch is NOT_RUN without Native input. The existing 1024-event ring/gap test remains the stronger bound evidence and is cited rather than duplicated. The command-map key equals the receipt `command_id` before and after disconnect/regrant.

DD-22 retention/export/log-split contents remain unapproved and unimplemented. Windows crosswalk row 12 crash-diagnostics evidence is `NOT_RUN/HOLD`: no crash was induced and no Windows Error Reporting scope, ACL or security setting was changed. No real Runtime credential, A19, Product or release acceptance follows.

## 12. MP-FLOW1 graph inspection qualification

[The F01–F10 admission](evidence/README.md#historical-records) owns the affected contract. Unit oracles prove exact `GraphRef` preservation separate from projection revision, stable NodeId/EdgeId keys across reorder and search, changed-revision rederivation even at the same projection revision, semantic label/kind/endpoint copying, and case-insensitive filtering without topology or identity changes. B-01 remediation adds deterministic cancellation checks: `rows_cancellable` returns no partial result once cancelled (including cross-thread mid-scan cancellation), filtering runs on the GPUI background executor, and the complete admission identity (session generation, publication version, epoch, exact revision, query) rejects every superseded dimension before rows are stored; keyboard and selection helpers consume admitted rows only. [Workspace quality](evidence/README.md#historical-records) reports 252 Rust tests, 14 dependency tests and zero warnings. [Independent Gate R2](evidence/README.md#historical-records) `831d57fe-8468-4d7d-b3f7-2671a232cfda` SUCCEEDED on exact `6304885…` with 9/9 checks, 17 outputs, zero absent output and cleanup complete; Gate R1 remains historical for its superseded source. [Remote review R1](evidence/README.md#historical-records) returned 1 BLOCKING (B-01); its cancellable-worker remediation passed [Gate R3](evidence/README.md#historical-records) `c420427d-1224-4b4e-aa6d-e0deaac34d51` on exact `861c616…`. [Remote review R2](evidence/README.md#historical-records) then returned 1 BLOCKING (filtering still ran on the foreground executor) and 1 ACTIONABLE NON-BLOCKING (reused Native capture hashes); filtering now runs on the GPUI background executor, fresh 10,000-node captures identify PID/binary/timestamps, and [Gate R4](evidence/README.md#historical-records) `5f48eb42-5cfe-4102-a1fa-ade4dc97b736` SUCCEEDED on exact `5c61266…` with 9/9 checks, 24 outputs, zero absent output and cleanup complete; [Remote review R3](evidence/README.md#historical-records) returned 0 BLOCKING / 0 ACTIONABLE NON-BLOCKING / 0 EXCESSIVE on exact Gate R4 candidate `5c61266…` and closed the fixture/public-seam technical scope. RC-12 real Runtime graph, 10,000-edge rendering, exact cancellation latency, Native revoke/regrant, screen-reader/IME/DPI, A19 and Product/main/release remain NOT_MEASURED/NOT_RUN.

[Native observation](evidence/README.md#historical-records) covers the fixture `fixture:graph/Task-184` version 3/digest header, projection revision 2, Nodes (9)/Edges (8), search `outcome` filtered to one node/edge, Tab+arrow focus in both lists, one mounted listitem per virtualized list, an authorized empty graph, and disconnected withholding with no cached rows. A compact 10,000-node fixture rendered Nodes (10000)/Edges (0), filtered `N09999` to one row in 271 ms, and a superseding `N9` query ended with zero rows and no stale result. Captures are retained under [evidence/flow1/captures](evidence/README.md#historical-records). Exact 10,000-node cancellation latency, 10,000 edges, `Ctrl+3` synthetic modifier delivery, full screen-reader/IME/DPI/High Contrast certification, native graph replacement and authorization revoke/regrant remain `NOT_MEASURED`; real Runtime graph RC-12 and A19 remain `NOT_RUN`.

## 13. MP-REC3 local restoration qualification

[The REC-01–REC-17 admission](evidence/README.md#historical-records) owns the affected contract. `app-core` unit tests cover schema round-trip and atomic replacement, corrupt/unknown-version/unknown-field full-default fallback, closed serialized keys, Windows failed-replace preservation, repeated-save stability, bounded opened-handle acquisition (exact MAX / MAX+1), bounded serialization, and position+dimension clamping across removed, reordered, negative-origin, small-display and empty-display cases. [Native observation](evidence/README.md#historical-records) proves normal save/restart restoration of Flow, Task-184 and Viewed B; corrupt-file defaults with a static notice; off-screen geometry clamped to the visible display; and explicit unavailable notices for missing Viewed and Task identities. Captures are under [evidence/rec3/captures](evidence/README.md#historical-records).

[Remote review R1](evidence/README.md#historical-records) returned 2 BLOCKING (actual-I/O bound and oversized-window dimensions); both were closed by Gate R2. [Remote review R2](evidence/README.md#historical-records) then found clone-before-bounds in save and missing build identity; save now validates the borrowed schema before cloning, [build identity](evidence/README.md#historical-records) binds source `6ccef79` to executable `2f4420…`, and fresh r4 save/restore/oversized-file/oversized-window runs were recorded. [Independent Gate R3](evidence/README.md#historical-records) `c5223e39-116d-444e-819a-bee6a6b5ea36` SUCCEEDED on exact `638aa5d…`; [Remote review R3](evidence/README.md#historical-records) then required the complete borrowed document (including fixture IDs) to be bounded before cloning and the quit path to save the locked value. Both are implemented; [Independent Gate R4](evidence/README.md#historical-records) `612a60c0-a35a-4452-a821-f800b4abd184` SUCCEEDED on exact `bc81b33…` with 9/9 checks, 39 outputs, zero absent output and cleanup complete; [Remote review R4](evidence/README.md#historical-records) returned 0 BLOCKING / 0 ACTIONABLE NON-BLOCKING / 0 EXCESSIVE on the Gate R4 candidate and closed the Desktop-local verified slice. Runtime Task survival (MP-RT8/RC-16), DD-23 topology/mixed-DPI, crash-before-quit, live revoke/regrant and native Compare restore remain `NOT_MEASURED`/`RUNTIME_BLOCKED`.

Real monitor detach/reorder/removal, mixed-DPI/maximized behavior (DD-23), crash-before-quit saving the latest change, live revoke/regrant during restoration, Compare-pair restoration, and Runtime Task survival (MP-RT8/RC-16) remain `NOT_MEASURED` or `RUNTIME_BLOCKED`; no canonical, credential, media-locator or command data is stored.

## 18. MP-R0A-S2A process lifecycle driver

[S2A admission](evidence/README.md#historical-records) bounds this dev-only launcher to approved Desktop/helper artifacts, pinned non-reparse paths, a dedicated single-use profile and launcher facts; visual probes remain `NOT_MEASURED`. Profile and LocalAppData come from `SHGetKnownFolderPath`, not environment variables. The per-user `LocalAppData/cos-desktop-r0` registry exclusively consumes both execution ID and profile volume/file ID; clearing or renaming a profile does not erase its claim. Existing user Desktop profiles are never modified. Every path retains read-only sharing with DELETE denied; the helper enters a kill-on-close JobObject before `go-v2`.

Publication exclusively writes, flushes, syncs and closes all three artifacts plus `completion.json`, validates their complete semantics, then creates an empty directory named `committed-<SHA256 of completion.json>` as its sole visibility commit point. No final JSON marker can survive its own failed fsync: `commit.json` is obsolete and is never an alternate success path. Consumer acceptance requires the matching empty commit directory, pinned bounded reads, exact endpoint IDs, reasoned `NOT_MEASURED` for all non-launch endpoints and ledger/result disposition agreement. A crash before commit is incomplete; after commit all prerequisite file operations have succeeded. This is not a power-loss durability guarantee or a defense against an administrator deleting the per-user registry.

[R9 verification](evidence/README.md#historical-records) records 48 Python tests (25 S1 + 23 S2A, SKIP 0), two helper vector tests, actual approved normal/timeout runs and real driver-kill containment. The fault tests cover create/write/flush/fsync/close errors and interruption on both sides of the directory commit point. Raw run ZIPs retain empty commit directories, which Git alone cannot store. Native observations distinguish the killed target driver/helper/Desktop from a surviving unrelated test Desktop; both test runs are subsequently cleaned up. Screenshots prove native startup only, not instrumented frame/Present/displayed endpoints. [R10 independent PASS 0/0/0](evidence/README.md#historical-records) supports the [S2A-only technical completion](evidence/README.md#historical-records); the reviewer inspected retained Native records and Gate hashes but could not independently unpack ZIPs in its environment. Earlier pending/HOLD fields remain historical snapshots. S2B/S3, R0B–C and measurements remain unexecuted; R0D/numeric SLO are not ratified.

## 17. MP-R0A-S1 measurement contract foundation

[R0A-S1 admission](evidence/README.md#historical-records) bounds this slice to the versioned manifest contract (rendering §10.1), a bounded loader, the §11.2 result contract, structural validation and an offline CLI in `apps/desktop/devtools/r0` (Python standard library only). [Validation](evidence/README.md#historical-records) records eleven unittest cases and CLI outcomes: valid manifests and results validate; missing fields, wrong types and unsupported versions fail; oversize, duplicate-key, malformed and non-finite JSON fail closed before publication; unmeasured endpoints and metrics stay `NOT_MEASURED` with explicit reasons and null percentiles; `NOT_IMPLEMENTED` cannot become observed; failed/censored/excluded counts stay separate from observed; result identity must match the manifest; error output reflects only stable codes and JSON paths, never input text or paths. The tool launches nothing, fetches nothing, executes no input value and performs no measurement; R0A-S2/S3 drivers, R0B baselines, R0C attribution, R0D ratification and any numeric claim remain NOT_RUN.

## 24. MP-IMG2-W13 4K full-frame output verification

`test_w13_full_frame.py` proves both inputs' 16,777,216 pixels equal the fixed
formula, pins the expected-row construction, replays each retained full-frame
output from its stored bytes through the bounded streaming reader (4096 rows,
67,108,864 inflated bytes, one canonical RGBA digest) and rejects truncated or
trailing-byte outputs. `test_w13_negative.py` requires the retained real control,
proves the unmodified control passes source-bound verification first, and mutates
40 semantic classes: fixtures/source identity, budget and admission fields,
decoder identity, unexpected ROI or partial-decode claims, refusal publication,
stage order and holds, PNG CRC/truncation/trailing bytes, output digests, worker
reaping/cancellation, cleanup, collection identity and bounds, false memory or
scratch claims, record size and repeat disagreement. A repository without the
pinned Git objects fails at setUpClass instead of reporting unearned rejections.

Unchanged W1–W12 regression, full quality, nine fresh committed Refine Gates,
disposal integrity and pushed-SHA Tern review remain separate exits.

## 23. MP-IMG2-W12 matched-raster JPEG/PNG differential

`test_w12_png_fixtures.py` covers the streamed grayscale producer and its
independent reader: fixed bytes/digest, exclusive create, chunk/CRC/IHDR/IDAT/
IEND structure, wrong dimensions/depth/colour type, bad CRC, missing IEND,
trailing bytes, excess inflation, missing deflate tail, unknown filter, a single
tampered pixel and ROI bounds. `test_w12_matched.py` proves every one of the
16,777,216 input pixels of both fixtures equals the fixed DC/block-coordinate
raster and that the oracle is applied only to bounded decoder ROI output, never
to the 4096-square source. `test_w12_negative.py` requires the two retained real
Native records and mutates 39 semantic classes — fixtures, source identity,
budgets, ROI charges, decoder identity, admission, stage order/holds, retained
pixels with coherent producer hashes, refusal publication, cancellation,
cleanup, sampling identity/completeness/bounds, false partial/peak/codec claims
and repeat disagreement. Mutations are verified against the real repository, so
a forged source commit or blob digest is re-checkable rather than merely
unproven; a missing control fails instead of skipping.

The matrix runs six fixed scenarios as six serial lifetimes and repeats them in
a fresh work root. Deterministic relations (fixtures, decoder/format
observation, admission, ROI geometry, output bytes, oracle results and
classification) must agree across repeats; PIDs, clocks and whole-process
counters are intentionally retained as unequal observations. Portable replay
cannot grant Git source closure, and a source change after the pin makes the
record non-closable. Unchanged W1–W11 regression, full quality, eight fresh
committed Refine Gates, disposal integrity and pushed-SHA Tern review stay
separate exits.

## 22. MP-IMG2-W11 JPEG full-frame admission and ROI boundary

`test_w11_jpeg_fixtures.py` validates fixed 4K/16K DC-only encoded identities,
bounded entropy/padding, real coordinate-dependent ROI pixels, exclusive output
creation and PNG framing/inflate limits. No full-frame RGBA is generated.
`test_w11_jpeg_monitor.py` exercises the real pending-to-final filesystem
transition after enumeration: an owned publication is conservatively charged,
while disappearing immutable or foreign files remain errors.
`test_w11_jpeg_negative.py` uses the mandatory retained real Native control and
mutates fixture/source identity, geometry/budgets, ROI/full-frame charges,
stage order/clock/holds, actual pixels with coherent producer hashes, refusal
publication, cancellation/cleanup, process-window identity/bounds and claimed
partial/peak results. Coherent stdout/trace and observer copies cannot turn
contradictory observations into verified output. Missing control fails.

The matrix covers five fixed scenarios / nine serial workers, including six
all-pixel-verified ROI outputs and three pre-OnLoad refusals. Three stage
windows for each 4K scenario come from separate process lifetimes, not a
same-process memory delta. Repetition must preserve deterministic fixture,
probe, pixel and classification relations; PIDs, clocks and real counters
are not forced equal. Portable replay cannot grant Git source closure.
Unchanged W1–W10 regression, full repository quality, five fresh committed
Refine Gates, before/after disposal integrity and pushed-SHA Tern review remain
separate exits. GUI startup, true partial decode, 16K full-decode memory,
internal scratch/reservations/concurrent peak, cooperative cancellation and
product adoption are not established by this experiment.

## 21. MP-IMG2-W10 JPEG metadata interpretation

`test_w10_metadata_fixtures.py` and `test_w10_metadata_negative.py` cover W10.
The independent parser checks bounded JPEG segment/entropy/EOI completion,
EXIF offsets/types/counts/duplicates, orientations 1/3/6/8, ICC header and tag-table
bounds/alignment/overlap, and invalid/truncated EXIF/ICC. Twelve deterministic
sources drive 29 real Windows decoder legs; the retained PNG bytes allow portable
reconstruction of every RGBA output and independent orientation index rotations.
The 40 negative classes exercise metadata observations, transform requests and
geometry, PNG bytes/RGBA digests, recomputed pixel comparisons, input/probe identity,
case completeness, malformed-success classification, bounds and reaped children.
Mutations preserve coherent captured JSON so the actual invariant must reject the
forgery. A missing/invalid retained control fails rather than skipping the suite.
Structural replay does not claim pinned-source closure. Real Windows repetitions,
the unchanged W1–W9 regressions and the new independent Gates remain distinct from
portable verification. Decoder differences are preserved; ICC colour correctness,
absolute JPEG correctness, product policy and full Decode Exit remain unverified.

## 20. MP-IMG2-W9 progressive JPEG

`test_w9_progressive_fixtures.py` and `test_w9_progressive_negative.py` cover the
W9 slice. Fixture tests prove the five-source set, byte-identical regeneration and
the encoded-size bound; marker tests require SOF2, at least a DC and an AC scan,
valid constraints, no trailing bytes and EOI, prove a high AC band is legal while
a DC scan with `Se≠0` or `Ss>Se` is not, and prove both malformed streams fail the
independent parser. The negative suite mutates the retained record in 51 classes —
SOF2/scan/selector/spectral/approximation/order forgeries, truncated entropy,
missing EOI, segment and digest changes, geometry/precision/sampling, encoded-as-
decoded, admission, candidate/decoder/case identity, worker/probe/terminal
outcome, publication, output bytes/SHA, comparison statistics, differential-to-
absolute promotion, malformed promotion/leg removal/missing outcome, coherent
trace tails and digest mismatch, incomplete capture, swapped process identity, an
old source commit and a forged stop classification — and requires `verify()` to
reject each one. Structural replay keeps `source_identity_verified=false` and
`closure_eligible=false`.

## 19. MP-IMG2-W8 JPEG raster evidence

`test_w8_fixtures.py` and `test_w8_negative.py` cover the W8 slice with the
standard library. Fixture tests prove the twelve-source set is fixed, every
runnable source regenerates byte-identically, encoded sizes stay under 16 MiB,
malformed derivations differ from their source and fail the independent parser,
markers expose variant/precision/geometry/sampling/tables for each encoding, the
DC reference reproduces the declared luminance levels exactly, and the reference
module contains no decoder, subprocess or image-library import. A synthetic PNG
pair proves the canonical comparison reports exact, per-pixel, per-channel,
maximum, mean and first-difference values. The negative suite mutates the
retained record in 51 classes — geometry, precision, sampling, markers, digests,
decoder identity and probe identity, requirement understatement, encoded-as-
decoded bytes, forged stages/outcomes/output, unpublished success, absolute
digests, differential-to-absolute promotion, zeroed mismatch statistics,
normalization and ICC/orientation claims, malformed promotion, unsupported
reclassification, coherent trace tails, incomplete captures, swapped process
identity, an old source commit and timeout-as-cancellation — and requires the
real `verify()` to reject each one. Structural replay must keep
`source_identity_verified=false` and `closure_eligible=false`.

## 18. MP-IMG2-W7 large-dimension admission

`test_w7_fixtures.py`, `test_w7_admission.py` and `test_w7_negative.py` cover the
W7 slice with the standard library only. Fixture regeneration is byte-identical
for the bounded cases, every encoded input stays under 64 MiB, the pattern-derived
RGBA digest equals the digest parsed independently from the encoded bytes for all
three codecs, and the Bgr565 mask expansion is checked directly. Admission tests
prove that the full-frame requirement — not the 1×1 ROI output — decides
refusal, that encoded size is never used as the decoded budget, and that the
materialized cases sit exactly inside the experimental limits. Oracle tests
reject a corrupted CRC, an incomplete inflate stream and a wrong byte-lane
addition. The negative suite mutates the retained real record in 32 classes (the 24 required plus probe-result and corrupt-worker consistency)
(declared width, codec, bpp, encoded-as-decoded bytes, promoted 8K/16K
admissions, ROI reduction, tampered budget values, OnLoad in a refusal,
published output or success after refusal, terminal-stage disagreement, fixture
digest, old source commit, decoder identity, altered oracle digest, decoded
trace tail, missing output, timeout-as-cancellation, private bytes as scratch,
refusal as 16K capability, preview promotion, incomplete capture as complete and
bound overruns) and requires the real `verify()` to reject each one, while a
structural-only replay must keep `source_identity_verified=false` and
`closure_eligible=false`.

The W7 Native matrix itself is evidence, not a test: 180 IMG2 tests, the W1–W6
replays, the WIC admission selftest and the workspace/feature/R0 quality gates
are the executed checks. `NOT_MEASURED` counters and damaged-container decodes
stay unverified; no skipped R0 case is promoted.

## 16. MP-IMG2 decoder-agnostic decode preflight

`crates/media/tests/decode_admission.rs` exercises the public preflight only: exact decoded-byte computation (8/16-bit RGBA, 1-bit rounding), inclusive budget boundaries with one-byte-over rejection, `max_dimension`/`max_pixels` rejection, invalid bit-depth/channel declarations, invalid limits (zero dimension/pixels/bytes, bit depth outside 1–16) and cancellation. No decoder, codec capability, profile, region, scratch allocation or GPU object is exercised; those remain IMG2 experiment/adoption or IMG3 work. R0 measurement authorization and decoder selection are unchanged.

[Two-Gate verification](evidence/README.md#historical-records) binds `eb4327a` to quality Gate `52bc394d…` (8/8) and build Gate `89d6aead…` (3/3), both SUCCEEDED with identical 13 artifact digests; the inherited `sec3-output-N` ids are noted as cosmetic in the aggregate.

## 15. MP-SEC3 adversarial media admission

[The A–H admission](evidence/README.md#historical-records) owns the SEC3 slice and its boundaries. `crates/media/tests/security_contract.rs` exercises observable admission through the public cache: zero or overflowing dimensions and an area above `LocalLimits::max_pixels` are rejected after reading only the bounded 33-byte header, with `Usage::default()` and no cache entry; the exact ceiling is admitted and one pixel above is not; a policy/decoder identity naming a type cannot bypass header rejection or create verification; trailing opaque text containing a command, drive path or URL stays uninterpreted and is never resolved (the fixture is unstructured tail bytes, not a parsed `tEXt` chunk); identifiers reject traversal, drive, slash and URL shapes; oversize declared lengths are rejected before any read (a panicking reader proves it), short sources fail closed and a non-IHDR first chunk is refused; cancellation during the header read publishes nothing; an absent expected digest is Unverified and a mismatch is bounded; error text contains only category names. The existing IMG1 suite now uses a complete 36-byte header fixture and still proves capacity, eviction, live-consumer reservation release and provenance/digest separation. No decode, transfer, authorization or GPU behavior is simulated.

[Two-Gate verification](evidence/README.md#historical-records) binds `89e2bcb` to quality Gate `b212273b…` (8/8) and build Gate `714139ad…` (3/3), both SUCCEEDED with identical 12 artifact digests. The earlier local quality r1 Clippy failure is retained in `quality-r1.txt`.

## 14. MP-RT8 local lifecycle qualification

[L01–L13 admission](evidence/README.md#historical-records) separates real OS-lock boundary tests, public observation/REC3 regressions and the required Windows executable matrix. The new ownership tests cover concurrent acquisition with exactly one winner, refusal while a lock is held, reacquisition despite a persistent lock file, Windows file/directory replacement refusal and unavailable-namespace failure without damaging its prior bytes. No fixture or unit result proves process-termination recovery or native activation.

Exit-watcher regression coverage proves completion before a deadline, zero-budget refusal, late completion remaining unconfirmed, and deterministic OS timer-start failure without global thread exhaustion. Existing owner/REC3 regressions remain the oracle for command-fence closure, no implicit Stop/retry/settlement and atomic-save protection. The [268-test quality run](evidence/README.md#historical-records) and 14 dependency tests passed without warnings; the initial Python-launcher failure and invalid-linker-argument warnings are preserved as earlier attempts.

[Independent verification](evidence/README.md#historical-records) uses two sequential bounded Gates on the same full candidate and candidate digest: quality (8 checks) followed by build (3 checks), with all nine distinct required checks covered and all 21 declared artifact digests identical. Both Gates succeeded, cleaned up and passed native integrity verification. This does not relabel the original full Gate's 600-second timeout as success, does not weaken any standard check, and does not establish Native or Runtime acceptance.

[R1 source review](evidence/README.md#historical-records) found save-order/GPUI-deadline conflict, delayed timer-start extension and false no-host confirmation. The Caller authorized bounded background saving with explicit incomplete reporting; [design review](evidence/README.md#historical-records) is PASS 0/0/0, not product/quality/Gate/Native PASS. Corrective `quality-r4.txt` passes 272 Rust tests and 14 dependency-checker tests with zero warnings. New coverage distinguishes actual successful preferences saving while observation remains pending, atomic-save failure from observation confirmation, late successful writing from deadline-confirmed saving, and delayed timer-start remaining-budget/wake behavior without scheduling-sensitive sleeps. Existing owner/atomic-writer regressions are reused. Observation and save are polled independently with deadlines from one callback start; no-driver/not-observed outcomes remain distinct. New candidate verification is recorded separately from the preceding 268-test/21-output Gates: [aggregate](evidence/README.md#historical-records) binds `ddd8d65202ba526e88e04e5f98bf0ee23094709b` to quality Gate `51cbcd02-2bbf-4adf-900e-cd37c43d6d40` (8/8) and build Gate `6951ff1d-8da0-49bf-b80d-1dca18659665` (3/3), both SUCCEEDED with identical 21 artifact digests and no absent output. Preserved failures: a rustc `STATUS_ACCESS_VIOLATION` while compiling `vswhom` (the same environment and a fresh target then passed 272 tests, so it is recorded as an environment crash, not a source failure) and a prepared `head` check still naming the previous candidate. `quality-r3.txt` preserves the resolved Clippy failure; `quality-r5.txt` is the detached-save build.

[Native observation r2](evidence/README.md#historical-records) binds source `32a3af27c034f734e674778d45e39e799e8c2999` to the built executable and records, under live task-scoped control, ten dedicated-process runs in an isolated `%LOCALAPPDATA%` subtree: ACTIVE-fixture launch; second launch exit 3 in 18 ms with static refusal while the incumbent stayed operable (candidate B Viewed/Active, Flow graph rendered); simultaneous acquisition with exactly one window and one exit-3 loser; normal close exit 0 reporting `observation shutdown confirmed` and `preferences save confirmed` in about 133/118 ms with zero Stop or command trace lines; restart restoring Task selection, Viewed B and the Flow surface from the saved document; `taskkill /F` of the exact bound PID only (exit 1) followed by a relaunch that reacquired ownership with `desktop.instance` retained; zero observation budget reporting NOT confirmed while the save was confirmed; and a preferences path replaced by a directory reporting `preferences save failed` without destroying the existing entry. A dedicated trace-sink run recorded only grant, live/validated consumer state and closed with zero command transactions, so closing an ACTIVE fixture emitted no Stop or resend ([trace](evidence/README.md#historical-records)). Screenshots and AX captures are retained beside the record. The post-detach executable (`sha256:5ea49984…`) is separately exercised on source `ddd8d65` in [native r3](evidence/README.md#historical-records): ACTIVE launch, collision exit 3, normal close exit 0 with `observation shutdown confirmed` and `preferences save confirmed` in 129 ms, and a written preferences document. The earlier [HOLD observation](evidence/README.md#historical-records) remains historical. Crash-before-quit latest-change restoration, real Runtime Task survival (RC-16), DD-23 mixed-DPI, Compare-pair restore and session-logoff shutdown remain NOT_RUN or RUNTIME_BLOCKED; A/B Desktop-local technical completion is recorded in [technical-completion.json](evidence/README.md#historical-records) with Tern R2 PASS 0/0/0; it closes only the measured slice and is not Product acceptance.

## 25. MP-REL1 internal Portable staging/preflight

Run `python -B scripts/tests/test_stage_portable.py` with the qualified interpreter
from [BUILD_AND_RUN §4.11](BUILD_AND_RUN.md#411-fixed-interpreter-and-environment-rules-2026-10-24).
The public CLI cases cover exact package inventory/digests, missing or modified
binary/LICENSE/NOTICE, extra synthetic-secret content, unsafe manifest paths,
forged signed/release or negotiated-wire claims, unsigned-distribution refusal,
Windows junctions and hard-linked payloads. A separate-process case varies Python
hash seeds for mixed-case DLL imports; identical content must keep verifying.
The symbolic-link case reports SKIP when Windows denies link creation, never PASS.
Synthetic PE fixtures exercise preflight structure, not executable startup.

Review round R1 required three further properties, each covered by a deterministic
case: hashing and PE inspection must describe the same stable bytes as the admitted
path, so a payload hard-linked away or a manifest replaced by identical bytes
mid-verification is refused; a file added after inventory, or an in-place write
during hashing, is refused; and a failed post-rename verification withdraws only
the directory this run created, never a foreign replacement. Case
`test_stage_removes_nothing_so_a_replacement_can_never_be_deleted` pins the stronger
rule that the tool performs no directory deletion at all, so a failing stage retains
its staging directory and any bystander path is untouched.
`test_retained_staging_directory_is_named_in_the_printed_error` proves the retained
path is inside the message text rather than a `add_note` addition that `str(exc)`
would hide. A CLI-level failing `stage` would need a full locked Cargo build inside
the suite, so that end-to-end case stays in the real staging smoke instead. Inputs are bounded
before parsing (`MAX_PE_BYTES` 128 MiB, `MAX_MANIFEST_BYTES` 1 MiB): the exact
limit is admitted and one byte over is refused, and the 32 MB-class Native
executable stays admitted. The `stage` transaction cases isolate the filesystem
behavior with patched build inputs; they are not a substitute for the real
staging run.

In addition to the unchanged repository quality route and an independent pinned
Gate, exercise `stage` on the real clean candidate, verify its manifest and hashes
with an independent oracle, and run the staged executable in explicit fixture and
normal/unavailable modes from an independent working directory/profile. Keep the
package's internal-only status, source/binary identities, observed exit, and
negative cases separate from signing, authentication, clean-user, dependency/license
closure, installer/upgrade/uninstall and release evidence. The commands are in
[BUILD_AND_RUN §4.34](BUILD_AND_RUN.md#434-mp-rel1-internal-portable-staging-2026-10-30);
[REL1 evidence](evidence-published/rel1-packaging-latest/session.json) records only
the exercised scope. No new CI lane or test framework is introduced.

## 26. MP-REL3 bounded redacted support report

Run `cargo test --locked -p app-core --test support_report`. The six cases pin the
observable guarantees a support reader depends on: a Runtime/protocol version is
never filled from the Desktop package, a disconnected publication never invents a
projection revision, the diagnostic gap and per-kind counts survive summarization,
rendering stays inside the existing per-line and per-report byte bounds with an
oversized field, a synthetic token / signed locator / private path never renders,
and an unavailable Desktop states no measurement. No exporter exists under DD-22,
so there is no artifact to scan end to end and no Native surface changed; Native
stays `NOT_RUN` for this task.
