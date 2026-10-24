# ADR-0009 — Caller blanket acceptance of completed slices

- **Status:** Accepted (owner-granted). 2026-10-24.
- **Decision owner:** the repository owner / Caller, by a direct instruction in the implementation session.
- **Supersedes:** nothing. It closes the acceptance question for work that is already technically complete; it does not change any contract, measurement or Gate receipt.

## 1. Authority recorded verbatim

The Caller wrote:

> 残Caller acceptanceはなくすべて承認

In context this answers the standing question in the slice reports ("Gate success is not Caller acceptance") and the earlier standing list of approvals: the Caller accepts every technically completed slice, and no further per-slice Caller acceptance step stands between a clean review and the next slice.

## 2. What this grants

- **Acceptance of completed slices:** every slice whose implementation, deterministic gates, independent verification and fixed-SHA review are complete is accepted by the Caller. This includes the slices recorded in the master plan §19.14–§19.30 and later slices recorded under the same pattern.
- **Continuous execution:** no slice pauses for a separate Caller acceptance before the next slice begins. The loop is implement → deterministic gate → independent Refine Gate → fixed-SHA review → record → integrate to `dev` → next slice directive.
- **Tool and environment decisions already ratified:** ADR-0008's decoder adoption, declared budgets, R0D declared-basis numbers, the fourteen design gates and the MP-VID1 probe authorisation are accepted as recorded.

## 3. What this does not do

- It **cannot** convert a `NOT_MEASURED`/`NOT_RUN`/FAILED result into a PASS, and it does not accept work that has not been done. Measurement and review evidence keep their own verdicts.
- It does not resolve an external reviewer's outstanding BLOCKING/ACTIONABLE finding; those are still repaired and re-reviewed on a new fixed SHA.
- It does not replace Runtime evidence, measured native/DPI/AX evidence that the environment cannot produce, or the owning contract's own acceptance criteria; those stay `NOT_MEASURED` with reasons.
- **Publication stays a separate action:** merging to `main`, publishing a release, changing repository visibility or credentials and similar consequential publish steps are not implied by accepting completed plan work. They remain their own decision even though plan work is accepted.
- It does not authorise MP-VID2–VID4 or any Runtime-side change.

## 4. Consequences and rollback

- **Consequences:** the plan loop no longer stops at an acceptance gate; each slice's record states that Caller acceptance is granted under this ADR instead of "acceptance pending", and the remaining work is bounded only by real prerequisites (Runtime, unmeasurable native evidence, owner decisions for VID2+).
- **Rollback:** revert this ADR. Completed slices keep their technical records; only the acceptance statement would revert to pending.
