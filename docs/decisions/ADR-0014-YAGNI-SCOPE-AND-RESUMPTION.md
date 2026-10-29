# ADR-0014 — YAGNI proposal adoption and scoped implementation resumption

- **Status:** Accepted (direct Caller decision), 2026-10-30.
- **Decision owner:** repository owner / Caller, exercising the Desktop product, architecture and execution decisions presented in the audit.
- **Decision baseline:** local `dev` and live `origin/dev` both `e50325f6baf7ff7af3e12b3b0d68c837e96205db`, clean worktree before this documentation change.
- **Supersedes:** the global implementation pause recorded in master-plan §19.67–§19.69 and the outstanding Caller-adoption/permission predicates of the audited proposals. Historical pauses, failed/HOLD results and reviews remain unchanged records. ADR-0008's configurable ceilings, ADR-0009's acceptance/evidence separation and ADR-0013's evidence retention remain in force.

## 1. Direct authority

The Caller wrote:

> すべての提案を承認し、コーラー承認待ちも承認する。そのうえで実装停止を解除

This adopts the reviewed [status audit](../development/IMPLEMENTATION_STATUS_AUDIT.md), Y01–Y12 and its execution principles; grants outstanding Caller permissions for that Desktop scope; and releases the global implementation pause **after the owning canonical documents and derived plan are reconciled**. The approval is not a measurement, producer implementation or claim of product acceptance. No repeated per-slice Caller-acceptance wait is required for technically completed, independently verified work covered by ADR-0009.

## 2. Adopted proposal register

| ID | Accepted decision | Execution boundary |
|---|---|---|
| Y01 | Retain scoped authorization, atomic canonical publication and unknown-command safeguards | No duplicate policy layer or weakened integrity |
| Y02 | Retain checked resource bounds, ownership, lifetime and independently useful regressions | Low frequency does not waive high-severity guarantees |
| Y03 | Consolidate the exact overlapping observer cleanup scenario and subset source-name scan | Later bounded test change retains persisted failure/privacy checks and explicit `ok=false`; this documentation change deletes no test |
| Y04 | Use the existing change/risk-appropriate docs-only verification route | Do not mechanically rebuild/re-Gate unchanged product bytes; changed-source evidence stays candidate-specific |
| Y05 | Adopt one reconciled 62-task census and stop re-requesting granted approvals | Historical fields are not new approval predicates; observed completion/verification stay independent |
| Y06 | Keep minimal durable latest evidence and exact candidate references without new persistent source/report mirrors | No historical pruning, receipt rewriting or loss of independently inspectable evidence |
| Y07 | Defer PUB4 production adoption and further representation variants | Keep production Arc baseline and existing exploratory evidence; re-entry needs a real workload deficit |
| Y08 | Defer optional adaptive scheduling and advanced graph/custom-rendering stages | Fixed safe budgets and simplest passing presentation first; retain caps and equivalent accessible information |
| Y09 | Stop speculative W14, duplicate workflow/framework additions and quota-driven test/experiment growth | Missing facts alone do not commission work; no automatic selection of rejected/deferred additions |
| Y10 | Adopt the `BOUNDED_IMAGE_DECODE` initial capability profile | Behavioral owner: [DC-MP-IMG2](../architecture/DESKTOP_DESIGN_CONTRACTS.md#dc-mp-img2); only actually admitted and safely bounded classes may proceed |
| Y11 | Adopt `IMAGE_FIRST_INITIAL_RELEASE` | Product owner: [Product §68.1](../architecture/COS_DESKTOP_PRODUCT_UX_ARCHITECTURE.md#681-image-first-initial-release); named deferrals do not remove the remaining real workflow/security/distribution requirements |
| Y12 | Authorize consumer-driven option/API consolidation and stop new unused knobs | Later code changes require reference analysis and caller migration; retain ratified budget configurability and one owner/default |

## 3. Canonical scope and permission boundaries

The Product Canon owns the initial-release inclusion/deferral list. The Design Contracts own admitted Decode, cancellation, resource, presentation and acceptance behavior; rendering refers to those contracts. This ADR records the **decision and authority**, not a duplicate capability specification or an admission-table substitute.

- The initial profile does not promise unconditional 16K, full-detail, region/tiled decode, cooperative in-decoder cancellation or exact hidden WIC allocation facts. It requires an actually verifiable conservative bound and an honest bounded preview/unavailable path outside supported classes. The ratified configurable numeric ceilings stay ceilings, not evidence that a class fits.
- W1–W13 limited completions, the historical full-target Decode HOLD, scoped native-startup PASS and Runtime-verification permission RECEIVED remain as recorded. Neither full-target completion nor a production admitted class is created by this decision. IMG3 needs the actual profile-specific predecessor evidence, not another permission request or an automatic PASS.
- All pending **Caller-side** acceptance/implementation permissions for the presented Desktop scope are RECEIVED. Unknown UX parameter details, missing Runtime operations/producer/issuer/descriptors, signed-distribution identity/certificate/channel inputs and physical evidence remain technical or external prerequisites. They are not a reason to ask for the same approval again, and missing values are not invented.
- Named initial-release deferrals preserve their full-target contracts. Other tasks marked DEFER in the census are queued behind their real prerequisites, not deleted from the image-first workflow. Deferring external OS deep-link registration does not defer internal History/reopen; deferring the automatic updater does not permit unsigned delivery or incompatible installation.
- Existing Desktop implementation/continuation authority may be exercised within the adopted scope and actual prerequisite-ready bounded work. Releasing the global pause does not resurrect speculative W14, deferred video/adaptive/representation work or a second planner/Gate framework. A permission record is not a dispatch of a new MP or Gate.

## 4. Unchanged restrictions and evidence

No Runtime-repository mutation, `main` integration, release publication, privileged environment/credential change, provider safety bypass, dependency or GPUI pin change follows. Runtime owns canonical semantics and server policy; no Desktop fallback simulates missing production success. Consequential operations retain their existing exact-target and point-of-risk approval requirements.

This reconciliation is documentation-only. No product source, test, script, lockfile, receipt or historical acceptance is changed; no Cargo build/test, Native/Runtime execution or Refine Gate is claimed. The preceding audit's OMP slow review remains bound to `084f62d8a9ac06afb75df8b6119b70ea6f0f753f`; it is not retrospectively transferred to these amended profiles. Documentation checks for this change are recorded separately in the plan.

## 5. Consequences and reversal

Update owning Product/Design/Rendering documents first, then the audit's current authority and plan task/readiness projections. Record permissions as received while retaining all unsatisfied non-permission conditions. The global state becomes **IMPLEMENTATION_AUTHORIZED — SCOPED RESUMPTION**; individual deferred or evidence-blocked tasks remain deferred or blocked for their named reasons. A future pause or scope revision requires its own explicit decision and does not rewrite this approval or historical evidence.
