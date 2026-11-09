# MP-ACC2 product acceptance matrix (owner directive 2026-11-09)

Bounded, falsifiable acceptance report for DC-MP-ACC2. **This is not a self-granted
product acceptance.** On 2026-11-09 the Product owner (Caller) approved the push and
accepted all judgment/decision items; this report records, item by item, what is
measured, what is `NOT_MEASURED`/deferred, and which acceptance lines remain open.
The release decision stays the owner's.

## Authority

- Owner directive (2026-11-09): push approved; all owner-approval and judgment items
  accepted. Implementation authority was already granted for the Runtime-dependent
  waves; this report does not add new product scope.
- External reviewers were not used (Tern/GPT forbidden by the standing instruction);
  every candidate was gated by the internal verifier + reviewer and the deterministic
  workspace gates.
- Statuses below are copied from the master plan/audit at the recorded commits; no
  item is marked PASS without a measurement.

## Matrix

| Item | Status recorded | Evidence (latest) | Open / residual |
|---|---|---|---|
| **MP-ACC1** real-integrated acceptance | PARTIAL / measured | golden generation (`COMPLETED`/`SUCCESS_ACCEPT`, 0×409 in 1,986 serving 200s), native artifact display, STOP, missing-media, History/Flow/Compare, Evidence PARTIAL: [acc1-native-closure-latest](../acc1-native-closure-latest/README.md); earlier protocol/native runs: `acc1-latest`, `acc1-followup-20261109`, `acc1-native-latest`; Brief receipt: [brief-receipt-latest](../brief-receipt-latest/README.md) | one-recipe catalog (multi-recipe `NOT_RUN`); further generation `NOT_RUN` (ComfyUI was repaired afterwards, not re-run); Evidence groups not projected; Compare panes did not display an image; DD-25 Start/Next resolved as submit-only (Runtime auto-dispatch) — no separate Desktop step; native AX `PARTIAL` with F1 open |
| **MP-FLOW3** LOD/batching custom-element decision | DEFERRED (D/E/F) under ADR-0014; required accessible inspection kept | plan card; no ACC2 blocker claimed | optional advanced stages; no attributable need recorded |
| **MP-REL2** updater | DEFERRED_INITIAL_RELEASE per Product §68.1 | plan card | explicit scope exception; not an initial-release exit prerequisite |
| **MP-REC2** cache plateau/release proofs | IMPLEMENTED_WITH_EVIDENCE_HOLD | §19.296; observation/replay campaigns | physical residency/release acceptance HOLD where owning endpoints are unavailable |
| **MP-NAV5** Command Palette | IMPLEMENTED — LIMITED to the approved inventory | `1b97fd3` historical Gate/review; palette rows measured in the AX audit (11 rows, disabled reasons) | real-Runtime availability, AX2 matrix, crosswalk rows, product/UX acceptance |
| **MP-PL1** visual polish | DESIGN_GATE_REQUIRED (dependent on MP-TYP2/DD-04b typography) | plan card | typography adoption decision; no measured polish acceptance |
| **MP-AX2** usability/keyboard verification | LIMITED native verification (12-operation matrix 11 PASS / 1 PARTIAL on `84f842a3`) + 2026-11-10 audit PARTIAL | [native-ax-ime-dpi-latest](../native-ax-ime-dpi-latest/README.md): UIA trees, Tab 1–8, Escape dismissal/focus restoration, shortcuts, DPI 96/96, ASCII typing | **F1 ACTIONABLE blocks the AX acceptance line**: focus signals disagree, no focused element from Tab 9, no UIA event stream (accevent requires elevation). Screen-reader pronouncement (NVDA absent), Shift+Tab, Enter/Space, Japanese IME composition (ImmGetContext NULL), mixed DPI, High Contrast, painted artifact image `NOT_MEASURED`. F2–F4 NITs |
| **MP-PUB3** multi-window/generation/stale-result conformance | COMPLETED_ACCEPTED (historical Caller acceptance) | plan card / historical records | full product windows/Runtime remain separate |
| **Video (VID1–4)** | Deferred by Product §68.1 | plan card | required absent an explicit scope exception; the owner accepted the exception on 2026-11-09 |
| **Typography (DD-04b)** | Conditional (MP-PL1 dependency) | plan card | measured typography evidence required before polish acceptance |

## Gate evidence at the recording tip

- Desktop final code gate at `fd4444a`: fmt/metadata/deps/ledger green, workspace clippy
  0, tests 52 suites / 1,305 passed / 9 ignored, build green. The subsequent product
  changes are the Brief receipt terminality fix (`8bad92d`) and the test-only RT3/AX
  harnesses; the final integrated gate is re-run at the closing tip and recorded in the
  plan (§19.327 or the final close-out record).
- Runtime: RC-11 `943e6b21`, journal `b794b637`/`0378fa40`, Windows service `1fb8900`,
  quality `270fd58`/`e1bc3e6`, all gated by pinned verifier runs; Runtime targeted
  suites are green except the pre-existing storage/lineage failures that the quality
  lane fixed (`e1bc3e6`).
- Evidence hygiene: closure manifest/raw-log caveats and the AX `NOT_MEASURED` list are
  documented in the respective READMEs; no secret/token/handle is stored.

## Verdict

**OWNER-ACCEPTED (decision items) with open evidence lines.** The owner's blanket
acceptance covers the judgment/decision items and the deferrals listed above; it does
not manufacture measurements. ACC1 is measured to its declared boundary; AX acceptance
remains open on F1 until a real AT/event-stream run; FLOW3/REL2/video/typography are
deferred/conditional by their owning decisions; MP-REC2 remains HOLD where endpoints are
unavailable. No release promotion is claimed or performed.
