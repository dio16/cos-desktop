# ADR-0010 — UX-01 closure for the dock behaviour (MP-DOCK2 gate)

- **Status:** Accepted (owner-ratified through the standing blanket acceptance in [ADR-0009](ADR-0009-CALLER-BLANKET-ACCEPTANCE.md); the owner has separately ratified the design-gate programme twice).
- **Decision scope:** the dock behaviours that [DC-MP-DOCK2](../architecture/DESKTOP_DESIGN_CONTRACTS.md) needs and that the documents genuinely leave **silent**. It reopens nothing already decided: the linked-transform model, per-pane clamping, link/unlink, "Viewed is local, Formal Selected is Runtime-owned", "two candidates select Compare", "compare is not canonical", the REC3 persistence schema and its identity rule, and the unavailable principle all stay as written.
- **Why a new record:** [ADR-0008](ADR-0008-OWNER-RATIFICATION.md) §5 approved UX-01 for the behaviours *already documented* in rendering §4.1 and DC-MP-CMP1/DC-MP-IMG5, and said explicitly that any behaviour those documents do not specify **stays open and blocks only that behaviour**. Several dock behaviours are in that gap, so they are determined here one behaviour at a time rather than being silently inferred from an approval label.
- **Method:** a read-only analysis enumerated DC-MP-DOCK2's required behaviours, quoted what the canon already fixes for each, and listed only the residual ambiguities with their options, consequences and the smallest default that unblocks implementation. Each determination below is the conservative option: it adds no Runtime dependency, cannot change a Runtime-owned projection, and where the current implementation already behaves this way it preserves it.

## 1. Determinations (each individually rejectable)

| # | Behaviour | Determination |
|---|---|---|
| D-1 | Initial viewed candidate when no persisted reference exists | **Explicit empty.** No candidate is shown until the user selects one ("choose a candidate" affordance). Never auto-select the first row and never auto-show a Runtime-provided "current" candidate; that would be an inference the contract forbids. |
| D-2 | Zero or one candidate | **Zero:** empty dock and empty work surface. **One:** viewing is available, Compare is unavailable **with a stated reason** (two candidates are required), not silently hidden. |
| D-3 | Compare pair creation and a third selection | The **first** explicit selection sets side A only once Compare is not already active, the **second** sets B and switches the surface to Compare; afterwards a plain selection changes **Viewed only**, and a side can change only through an explicit "change A" / "change B" action. Never silently replace a side from a third plain click. |
| D-4 | Viewed identity disappearing from the projection | **Keep the reference** and show an explicit unavailable state on the surface until the user acts. No automatic clearing and no automatic replacement. |
| D-5 | One compare side disappearing | **Keep Compare open** with that side explicitly unavailable; the user resolves it by changing that side or closing Compare. Never auto-close and never auto-substitute. |
| D-6 | What "the user acts" means | Changing the affected side (change A / change B) or closing Compare. If the same identity reappears in the projection while the reference is still held, it simply becomes available again — that is not a substitution. |
| D-7 | "Close and restore the previous view" | Returning to the **artifact surface with the Viewed candidate** and that candidate's own view framing. The compare session's per-pane transforms are discarded; no other history is replayed. |
| D-8 | Persistence scope | **REC3 stays authoritative**: a single viewed reference plus an all-or-nothing pair, with the documented identity rule. No per-task memory map in the MVP; changing the schema would be a REC3 amendment, not a dock decision. |
| D-9 | In-session task switch | The window carries **one** reference set; after a task switch the dock follows D-1 rather than restoring a per-task memory. |
| D-10 | Timeline content and edge cases | Only authoritative iteration records that carry candidate rows are shown, in their authoritative order. No synthesised gap nodes, no local "current iteration" inference; the formal-outcome display belongs to the inspector ([DC-MP-INS1](../architecture/DESKTOP_DESIGN_CONTRACTS.md)), not the timeline. |
| D-11 | Meaning of "history" in this contract | The **per-task iteration timeline** only. An application-level history surface remains [DC-MP-NAV3](../architecture/DESKTOP_DESIGN_CONTRACTS.md) and DD-07, which is Runtime-contract-gated and is not decided here. |
| D-12 | Focus versus selection | Focus is **independent** of Viewed and Compare. When a focused element disappears, focus moves to the nearest valid neighbour and **selection is unchanged**; keyboard bindings themselves are out of scope. |
| D-13 | Formal marker interaction | **Display only.** Clicking it does nothing and it can never issue a canonical command. |

## 2. Explicitly not determined here

- **Fit/1:1 default** and **swap semantics** remain open at DC-MP-CMP1/DC-MP-IMG5. They do not block the dock implementation, so determining them here would be a decision without a consumer.
- **Per-side link state on replacement** is a compare-model question (CMP1), not a dock question; the dock only exposes the change-side action.
- **Application-level history** stays Runtime-blocked (DD-07, RC-05/06/12), as does every Runtime-owned projection: Formal Selected, Best-so-far, Iteration, Formal Outcome, real history.

## 3. Consequences

- **Unblocked:** MP-DOCK2's dock implementation — the prerequisites (MP-DOCK1 technically complete) and this determination now cover the behaviours DC-MP-DOCK2 requires; implementation remains fixture/local, needs no Runtime and no new dependency.
- **Unchanged:** the contract's non-goals (no local inference of "current iteration", no canonical selection edits) and its acceptance checks (a silently swapped compare candidate, a lost viewed identity on removal and locally derived history order are still failures).
- **Still blocked, and this record does not pretend otherwise:** Runtime-owned data and real history, real media/thumbnails, compare display defaults (CMP1/IMG5), UX validation evidence (MP-UX1/UX2 prototypes), Refine Gate and product acceptance.
- **Rollback:** revert this ADR and the D-1…D-13 paragraphs added to DC-MP-DOCK2. The implementation keeps its current behaviour only where that behaviour already coincides with a determination; otherwise the dock returns to `DESIGN_GATE_REQUIRED`.
