# MP-UX1 participant validation protocol (not executed)

This protocol is the future participant step for DC-MP-UX1. It was **not executed** in
this session; the recorded reachability results are developer observations and are not
participant evidence.

## Setup

- Launch the shell with the explicit fixture mode:
  `cargo run --locked -p cos-desktop -- --fixture --fixture-file crates/cos-client/fixtures/task-184.json`.
- The fixture label `FIXTURE / simulated data` must remain visible; no real Runtime or
  product acceptance is implied.
- One participant per session; do not coach beyond the neutral prompts.
- Record the participant id (opaque), the prompt, the observed actions, any hesitation,
  any misinterpretation and whether the participant found the answer unaided.
- A failure or misinterpretation is recorded as-is; it is not repaired during the session.

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
developer reachability pass into a participant PASS. After the sessions, update
`scenarios.json` per participant and keep every explicit failure.

## Boundary

Participant results cannot be fabricated by models or developers. Until participants run
this protocol, the participant result stays `NOT_RUN`, and the prototype is not a shipping
UI and not a product/UX acceptance.
