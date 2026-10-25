# ADR-0011 — DD-26 intervention priority policy (SPEC_APPROVED)

- **Status:** **SPEC_APPROVED — owner-ratified 2026-10-25.** The Product/UX owner was asked directly and answered **「推奨案で全項目を批准」** (approve every item with the recommended defaults). This record is the gate text; it does not authorise implementation.
- **Scope:** DD-26 as registered in [DESKTOP_DESIGN_CANON](../architecture/DESKTOP_DESIGN_CANON.md) — *which intervention surfaces first and the tie rules* — for [DC-MP-INS3](../architecture/DESKTOP_DESIGN_CONTRACTS.md). Everything DC-MP-INS3 already froze stays frozen: its scope, its v1 input set (typed, labelled Desktop-local conditions only, because the protocol carries no canonical issue collection), its non-goal (no issue invention from connection errors), its acceptance checks and its conditional native-notification wording.
- **Record correction:** [ADR-0008](ADR-0008-OWNER-RATIFICATION.md) §5's row *"DD-26 (inspector)"* referred to the inspector's own gated behaviour and is not the DD-26 registry entry, which is this intervention-priority policy and was still `DESIGN_DECISION_REQUIRED`. The registry is the single owner of DD descriptions, so the gate was treated as open until this record.
- **Separate step still required:** per the canon's own ladder, `SPEC_APPROVED` alone does not authorise implementation — MP-INS3 additionally needs `IMPLEMENTATION_AUTHORIZED`.

## 1. What the owner approved

| # | Decision |
|---|---|
| 1 | **Ranking** is the contract's interim rule, adopted as final: tier 1 Runtime-supplied severity/code (**empty in v1**, because no canonical issue collection exists — a local condition must never synthesise one), tier 2 task-status order **ACTIVE < ASSIGNED < COMPLETED < CANCELLED < unknown last**, tier 3 the typed stable identity. |
| 2 | **Tie rules:** items with the same tier-2 and tier-3 key are the same condition and are **deduplicated** (one row plus a remaining count); otherwise the typed identity's opaque ordering decides, so no non-deterministic tie exists. Display text is never used as a key. |
| 3 | **Empty rule:** the strip is empty when the authorised observation holds no active typed condition and when a condition's cause is resolved — a resolved condition must never persist. When observation is blocked, exactly one Desktop-local row states that observation is unavailable; connection state is its own axis and is never promoted into an issue. |
| 4 | **Provenance labelling:** every row carries a provenance prefix — `Runtime issue` only for genuinely Runtime-supplied items, rendered verbatim, and `Desktop-local` for local conditions using state words only. Local rows may never use Runtime severity/code vocabulary and may never claim that Runtime reports them. |
| 5 | **Actions:** no action is ever started automatically, by render, refresh or any side effect. Only local conditions may offer actions, and only read-only ones: View/Inspect (navigate to the surface or inspector), Details (existing read-only receipt display) and Reconnect (local re-subscription with no canonical mutation). Runtime-canonical facts offer a navigation-only affordance. |
| 6 | **Destructive operations:** v1 defines none, and the strip never offers Stop. Existing explicit Stop controls keep their semantics, and the strip only *reports* Stop presentation states — Stop requested, outcome unknown, accepted — never claiming completion and never changing task status, which comes only from the task snapshot. |
| 7 | **Native notifications: deferred.** v1 is in-app only and calls no OS notification API. Any later delivery must state known state only, must show a denied or unsupported permission honestly, and must not steal focus; triggers, content and opt-in remain open pending Product/UX. |
| 8 | **v1 input set:** connection freshness, Stop/settlement presentation, and unavailable/missing references. Evaluation-reference absence is included **as a missing-reference condition**; a merely unrequested evaluation is not. |
| 9 | **Exclusions:** Runtime issue/severity feeds, Runtime authorisation, evidence-detail authority (MP-INS2 territory) and any destructive automation are out of scope and unchanged by this record. |

## 2. Items settled here that the documents left silent

- Terminal statuses among themselves follow the ordered list above (the only order documented in this repository, from DC-MP-NAV1's search ordering).
- Grouping of same-kind conditions is one row plus a count, with the full list available on demand.
- Focus after a dialog is dismissed returns to the originating strip item; no new focus-management mechanism is introduced.
- The strip shows no formal-outcome availability row: that is an inspector responsibility, not an intervention.

## 3. Consequences

- **Unblocked once `IMPLEMENTATION_AUTHORIZED` exists:** DC-MP-INS3's fixture/local strip — deterministic ordering, provenance labels, dedupe, empty and resolved rules, read-only actions only.
- **Still blocked, and this record does not pretend otherwise:** Runtime-issued interventions (the Runtime contract gap stands), OS notification delivery (crosswalk row 8), any destructive automation, MP-INS2's Runtime authorisation, and every Runtime-owned projection.
- **Rollback:** revert this ADR and the DD-26 registry row. MP-INS3 returns to `DESIGN_GATE_REQUIRED` and DC-MP-INS3's interim ordering becomes the only rule again.
