# ADR-0012 — Hold release and remaining authorisations

- **Status:** Accepted — owner-granted 2026-10-25.
- **Owner statement (verbatim):** 「全てのホールド解除。GPT実装・評価ループを使い、実装計画の全てを完了させて」
- **Reading:** every Caller-imposed hold in this repository is lifted, and the remaining implementation work of the master plan is to be driven to completion through the established loop (implement → deterministic gates → independent Refine Gate → fixed-SHA external review → repair → record → integrate). Nothing below invents a capability the environment does not have; it removes *permission* blockers and keeps *measurement* honesty.

## 1. What this releases

| Previously held | Now |
|---|---|
| **S2B journal** — Caller pause recorded in the MP-R0A card, with three open review findings | **Released.** Its three findings are ordinary repair work from here, and the journal's endpoints may be measured once the code passes the gates. |
| **MP-R0C** — card `NOT_AUTHORIZED` | **Authorised** to run under `DC-MP-R0C`, with the same bounded-record and closed-reason discipline as R0B. |
| **MP-R0B** — was `AUTHORIZED` but unmeasurable | Stays authorised; with the journal released, its CPU-side endpoints can be measured. |
| **MP-R0D** — `NOT_AUTHORIZED`, prerequisites unmet | **May proceed** as soon as R0B and R0C produce evidence; ratification remains the owner's decision and is not claimed by this ADR. |
| **MP-FLOW2 experiment authorisation** | **May be granted once R0D is ratified**, per the ordering the owner chose in plan §19.41; stages A–C remain the scope and stage F stays excluded. |
| **MP-INS3 native populated-strip acceptance** | No longer deferred by permission; it is an ordinary native-evidence step whenever a populated strip can be displayed. |
| **MP-RT8 remaining items, MP-SEC1, MP-ACC1/2, MP-INS2, MP-IMG6 and the other Runtime-bound cards** | Permission is not what blocks them: they need real Runtime evidence, real native evidence or privileged environment changes. They proceed **only to the boundary those prerequisites define**, and their runtime blocks stay recorded. |

## 2. What this does not change

- **No measurement becomes a PASS by decree.** `NOT_MEASURED` keeps its meaning: no present/displayed/scanout endpoint exists on this host, GPU execution has no timestamp query, true cold needs an authorised cache reset, and a single run is not a distribution. Where an endpoint does not exist, the record says so.
- **No Runtime-owned semantics move into Desktop.** Formal Selected, Best-of, Iteration, Formal Outcome, real history and the RC-* runtime guarantees remain Runtime-side; `Runtime`-blocked cards stay blocked on their Runtime contracts, not on permission.
- **Unchanged engineering boundaries:** the GPUI pin, `unsafe_code = "forbid"`, the dependency allowlist, no new dependency without its own justification, and the rule that a Gate receipt, an external review verdict and Caller acceptance are three different things.

## 3. Consequences and rollback

- **Consequences:** the queue becomes R0B (CPU-side endpoints now measurable) → R0C → R0D ratification → FLOW2 stages A–C, with the remaining plan cards taken in DAG order as their prerequisites clear. Each slice still lands only with its own gates and review; the release changes *what may be attempted*, never *what counts as done*.
- **Rollback:** revert this ADR. Holds return to their recorded states (§1's left column) and any work started under the release stops at its last reviewed commit.
