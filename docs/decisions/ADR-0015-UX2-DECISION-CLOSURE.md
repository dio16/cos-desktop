# ADR-0015 — UX-01 / DD-03 decision closure for Compare and viewing (MP-UX2)

- **Status:** **Accepted — owner-ratified 2026-10-31.** The Product/UX owner was asked the eight residual questions left open by [ADR-0010](ADR-0010-UX01-DOCK-CLOSURE.md) and [ADR-0008](ADR-0008-OWNER-RATIFICATION.md) §5, and selected the recommended value for each. The verbatim selections are recorded in §0 and in the [decision evidence](../development/evidence-published/ux2-decision-latest/owner-ratification-20261031.json). Nothing here is inferred from the Caller's blanket implementation permission.
- **Decision scope:** only the questions [DC-MP-UX2](../architecture/DESKTOP_DESIGN_CONTRACTS.md#dc-mp-ux2) froze and the [register](../development/evidence-published/ux2-decision-latest/decision-register.json) left open: the linked-transform model and default (Q5), fit/1:1 switching (Q6), cursor-anchor mapping (Q7, derived from Q5), unlink restore (Q8), swap (Q9), link state on pair replacement (Q10), pane keyboard/focus order (Q12), unavailable/limited-preview wording (Q13) and reuse visibility (Q14).
- **Out of scope:** Q1–Q4 and Q11 keep their ADR-0010 values unchanged. Viewed stays Desktop-local, Formal Selected stays Runtime-owned, Compare stays non-canonical, and no Runtime semantic, command or projection changes. The frozen DC-MP-UX2 fallback still forbids inventing a default: it is now satisfied by ratified values rather than left open, and Compare implementation itself remains MP-CMP1 work.

## 0. Ratification record

- **Owner:** Product/UX owner (the Caller).
- **Date:** 2026-10-31.
- **Method:** the eight open questions were presented with their options, consequences and a recommended value grounded in rendering §4.1 and ADR-0010; the owner selected one per question in a direct decision request. Each selection below is the owner's statement, not a derived default.
- **Statement:** the owner selected, for every question, the recommended option listed below (「その他3点はあなたの提案状態の実装で承認」 was the earlier scope approval; these eight selections are the separate UX-01 ruling).

| Question | Ratified value |
|---|---|
| Q5 linked model | Relative-to-fit is the default linked model: one shared normalized anchor `(u, v)` and one multiplier `z` over each pane's contain-fit scale. Physical 1:1 is a separate explicit mode, never mixed into the same numeric zoom. |
| Q6 fit/1:1 | Default is Fit. An explicit per-pair control switches to physical 1:1; switching keeps the shared anchor and recomputes only the multiplier. |
| Q7 anchor mapping | Derived from Q5: pane `i` maps the shared anchor through its own `fit_i = min(view_width_i / image_width_i, view_height_i / image_height_i)` and display scale `fit_i × z`; each pane clamps independently and a clamped anchor is never fed back into the link. |
| Q8 unlink | Unlink keeps each pane at its current display; the panes diverge from then on. No hidden restore and no jump. |
| Q9 swap | Swap exchanges the candidates only; each pane keeps its own transform and re-clamps against the new image. |
| Q10 pair replacement | Replacing one side keeps the link and resets only the replaced pane to Fit; the other pane keeps its current display. |
| Q12 keyboard/focus | Tab order is pane A → pane B → compare actions → candidate list. Focus stays on the same pane after Swap and returns to the candidate list when a side disappears. |
| Q13 unavailable copy | Per-state explicit wording: unavailable with its reason, bounded preview labelled as low-resolution and not full detail, and clipped regions stated. A bounded preview is never described as full detail or 1:1. |
| Q14 reuse visibility | Same-key decode/upload reuse is shown only in diagnostics; no badge in Compare. Sharing benefit remains `NOT_MEASURED` until measured. |

## 1. Consequences

- Compare implementation (MP-CMP1) may now proceed against fixed values; until it does, Compare stays `DESIGN_READY_NOT_IMPLEMENTED` with answered defaults instead of invented ones.
- MP-IMG5 keeps its bounded-preview/unavailable honesty rule; Q13 fixes the wording principle, not new media capability.
- MP-IMG2/IMG3/IMG4 media scope is unchanged: a bounded preview is still not a full-detail/1:1 claim.
- Measurement stays open: unequal dimensions/orientations/aspects/DPI, cursor-anchored zoom under clamping, repeated link/unlink, and reuse fairness remain R5 evidence (`NOT_MEASURED`), not accepted behaviour.
- No ADR-0010 value was modified; Q1–Q4 and Q11 remain exactly as ratified.

## 2. Evidence and checks

- Register: 14 questions, **E = 0**, every question carries its class, ratified value or measurement state, and source.
- Canon: Product §16 carries the ratified compare values and links here.
- Contract: DC-MP-UX2 records the closure and narrows its fallback accordingly.
- No product source changed for this decision; it is documentation-only, so no Refine Gate applies (ADR-0014 Y04), and no UX value is claimed as implemented or measured.
