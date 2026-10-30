# MP-UX1 participant validation protocol

The original developer reachability session did not execute this protocol. The later
[P01 record](participant-p01.json) contains actual answers to all eight prompts, five
comprehension failures and three partial results; it is not a successful UX validation.

## Setup

- Launch the shell with the explicit fixture mode:
  `cargo run --locked -p cos-desktop -- --fixture --fixture-file crates/cos-client/fixtures/task-184.json`.
- The explicit test-data label must remain visible; no real Runtime or product
  acceptance is implied.
- **UX08 exception:** launch a separate unconfigured window without `--fixture` (or with an
  empty `COS_DESKTOP_RUNTIME_ENDPOINT`); that window must state that no source is connected
  and data is withheld, with the single Desktop-local intervention row. This is the only scenario that uses the
  disconnected launch.
- One participant per session; do not coach beyond the neutral prompts.
- Record the participant id (opaque), the prompt, the observed actions, any hesitation,
  any misinterpretation and whether the participant found the answer unaided.
- A failure or misinterpretation is recorded as-is; it is not repaired during the session.
- The five-second check requires controlled first exposure and an observed timer.
  Record unmeasured timing as `NOT_MEASURED`, and keep any participant timing
  self-report separate. P01 had uncontrolled prior exposure and unknown timing.

## Neutral prompts and expected understanding

| Check | Neutral prompt | Expected understanding |
|---|---|---|
| UX01 | "What is this task doing right now?" | Task status, inner phase, iteration, connection |
| UX02 | "Where is the artifact you are working on?" | The artifact surface is the work area and names the candidate |
| UX03 | "How do candidates relate to iterations?" | Candidates belong to iterations in the loop dock |
| UX04 | "Which one did you choose, and is that the final selection?" | Viewed is local; Formal Selected/Outcome is Runtime-owned and currently unknown |
| UX05 | "Show me why this candidate was evaluated this way, and what supports it." | Evaluation values then Evidence summary, without leaving the inspector |
| UX06 | "Compare two candidates." | Reach Compare and understand the two-side selection requirement |
| UX07 | "When would you look at the Flow, and when not?" | Flow is an on-demand inspection surface |
| UX08 | "A problem occurred. What happened and what can you do?" | One Desktop-local condition, its cause, and the safe local action |

## Recording

For each check record: `PASS`, `PARTIAL` or `FAIL`; the observed action; the
misinterpretation; and the evidence (screenshot or note reference). Do not convert a
developer reachability pass into a participant PASS. Keep the original `scenarios.json`
developer record separate from per-participant records, and preserve every explicit failure.

## Boundary

Participant results cannot be fabricated by models or developers. A missing session stays
`NOT_RUN`; recorded failures do not satisfy the comprehension exit. The prototype is not a
shipping UI, and neither participant observations nor later repair smoke imply product acceptance.
