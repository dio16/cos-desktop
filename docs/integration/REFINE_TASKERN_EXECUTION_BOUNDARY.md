# Refine / Taskern execution boundary

**Status (2026-10-11):** Accepted Desktop **development-workflow policy**, grounded in Taskern 0.2 and Refine 0.3 public contracts. Not a COS product integration, Runtime change, implementation authorization, Caller acceptance or release approval. Product/UX, DC, DD, RC, R0 and video authority remain unchanged.

## Source identity and evidence

| Product | Observed local source (clean `main`) | Existing executable / observed version | Contract axes |
|---|---|---|---|
| Taskern | `d0f565d13d01a3d91ed97c308a04e42d6eae71e0` | `<taskern-root>/target/release/taskern.exe`; `0.2.0` | CLI 2 / interaction 1 / semantic 1 / binding 1 |
| Refine | `64474739046ba00a24d42c228f900971014025f8` | `<refine-root>/target/debug/refine.exe`; `0.3.0` | CLI 2 / native semantic 1 / binding 1; six Gate operations |

These are separate source and executable observations, **not** proof that either binary was built from the adjacent source, packaged-release qualification or current remote-tip verification. Exact paths, digests, invocations and limitations were historically recorded in private STEP 1 evidence; see the [historical-records disclosure](../development/evidence/README.md#historical-records). Binary provenance and the actual independent check environment must be qualified before an implementation Gate.

Public authorities: [Taskern contracts](https://github.com/dio16/taskern/blob/d0f565d13d01a3d91ed97c308a04e42d6eae71e0/docs/CONTRACTS.md), [reference](https://github.com/dio16/taskern/blob/d0f565d13d01a3d91ed97c308a04e42d6eae71e0/docs/REFERENCE.md), [integration boundary](https://github.com/dio16/taskern/blob/d0f565d13d01a3d91ed97c308a04e42d6eae71e0/docs/INTEGRATIONS.md); [Refine contracts §14](https://github.com/dio16/refine/blob/64474739046ba00a24d42c228f900971014025f8/docs/REFINE_CONTRACTS.md#14-independent-candidate-gate-extension) and [source Skill](https://github.com/dio16/refine/blob/64474739046ba00a24d42c228f900971014025f8/skills/refine/SKILL.md). Use the producer contracts matching the selected executable and its actual `describe` surface; this document does not maintain a second schema.

### Skill discovery and use

- Read the host-discovered `taskern`, `refine` and `tern-chatgpt-review` Skills before their operations. Use only the needed operation descriptions/references, not a transcription of the canon into prompts.
- Taskern: select `check`, `compile` or `revise` for the current planning input; submit a complete request through `taskern call --request FILE`. `next`/`feedback` use Caller observations. `adapt` requires an explicit Caller Binding and is not part of the default Gate route.
- Refine: select **VERIFY_EXISTING_CANDIDATE** for existing-agent development; read the matching source Skill's Gate workflow, then environment/verification/failure references as needed. Discover with `--version`, `describe capabilities` and `describe operation gate.run`; use an absolute executable when not on PATH.
- Tern: use the existing `tern-chatgpt-review` connection, verify the resolved conversation and reachable composer, and review the fixed candidate/diff/constraints/evidence in that same conversation.
- Discovery, reading, CLI activation and real execution are separate evidence claims. If a Skill is missing, stale or unusable, record the cause and stop the affected operation; do not invent a replacement workflow.

**Observed host mismatch:** OMP currently discovers an older managed Refine delegation router referencing an absent `target/release/refine.exe`. The producer's current source Skill was explicitly read and the existing debug binary exposed Gate 0.3. This is manual source-Skill loading/static discovery, not automatic discovery of the current router or a qualified Gate run. Resolve host discovery and binary provenance during the separately authorized environment step; no external repository or Skill installation was changed here.

### Historical evidence (not the default workflow)

The 2026-10-08 observation used Refine source `acdfd363f7a98f863637b77a4300ad9eff55e051` / product 0.2 and Taskern source `10fe4e8314ec0b3f3caf873da84db2fd96441004`. Its generic and COR1 `check → compile → adapt` probes admitted representations only; they did not establish product acceptance. Their source-qualified measurements and probe digests remain in [the historical finalization report](../development/ARCHITECTURE_PLAN_FINALIZATION_REPORT.md).

The later MP-COR1 E2E **was accepted** on candidate `a5b318d7d474e032393b6c17660b8d845e393b64`, evidence head `c39e6eaa2ee6b592e566f6c2ca8f3076fe7e3d5a`, through three delegated Refine 0.2 executions. Caller acceptance, quality results and reviews are private historical evidence; see the [disclosure](../development/evidence/README.md#historical-records). They are not GateReceipts. Do not migrate, rewrite or relabel them as Refine 0.3 Gates. COR1's three-way implementation split is not a requirement for later tasks.

## Actual boundary: Plan to candidate to Gate to acceptance

| Owner | Responsibility | Does not own |
|---|---|---|
| Planner / Taskern Skill | Agent drafts a bounded derived Plan/TaskPacket; Taskern deterministically admits, validates and compiles it, with explicit context and Budget | Implementation, execution, context persistence, evidence truth, permission or acceptance |
| Coding Agent | Explore, implement, test and repair in the same authorized session; choose HOW within the frozen DC | New product/Runtime semantics, scope expansion or approval of unresolved gates |
| Refine Gate | Prepare the pinned commit in an independent detached checkout; run ordered checks, capture outputs/private logs and immutable evidence | Coding-agent launch, agent context, automatic retry, product acceptance or Git integration |
| Caller | Authorize scope/candidate/checks/budget, preserve correlation/history, independently review and explicitly accept/reject; authorize integration separately | Treating machine success as Domain truth or bypassing existing owner decisions |
| COS Runtime / Domain | Canonical Task, graph/plan, artifact, evaluation/outcome and stop semantics | Reinterpreting Taskern/Refine IDs as COS projections |

### Data mapping

- A Taskern `Plan`/`TaskPacket` is derived from the current Markdown plan and its single DC contract. `CompiledPacket` binds the selected packet digest, supplied context and input-upper-bound accounting; it is not permission, execution state or delivery proof.
- `ContextRef`, `ContextInput` and explicit compile `Budget` follow the Taskern reference. Bind required slices to the source commit and exact bytes/digests; preserve complete acceptance and prerequisite evidence. A compiler token estimate is not a Gate time/output/artifact budget.
- The existing `adapt` + `AdapterBinding` path emits the delegated **`TaskRequest`**, with an executor, not a `GateRequest`. It rejects unsupported/human evaluation rather than silently removing it. Use it only when delegated execution is explicitly authorized; no Gate adapter or schema equivalence is implied.
- Caller constructs the native **`GateRequest`** separately: `schema_version`, `gate_label`, `repository`, `policy`, `verification`, `outputs`. No executor/objective/Taskern metadata is added. `repository.revision` is a full commit ID; required `expected_commit` equals it; `policy.expected_changes` is `none`. At least one declared check is `command` or `file`; `head`/`git_clean` alone are not a quality Gate.
- Checks reuse Refine's `command`, `file`, `git_clean`, `head` vocabulary and operator allowlists. The receipt binds observed repository/candidate facts, checks, outputs, cleanup and bounded private logs. `gate verify` checks stored integrity only; it does not rerun checks or turn FAILED into SUCCEEDED.

### COS development-task mapping

Caller records outside both wire contracts: MP/DC and adopted Plan revision, source/context/packet digests and delivery, accepted hard dependencies, authorization, allowed/forbidden scope, candidate commit, attempt and accepted Gate ID, binary/config/environment identity, raw result/integrity references, review, explicit disposition and chronological history. Gate labels are correlation, not deduplication. Keep the full human/native/product acceptance rubric; machine checks cannot stand in for it.

Caller may authorize a finite same-task correction loop up front, with scope, checks and attempt/time limits. Within that grant, contract-internal HOW fixes need no new DD approval. Each changed candidate still needs an explicitly pinned new commit and new Gate request; no silent redispatch. Exhausted bounds, changed scope/contract/authority or an ungranted consequential operation require the relevant owner. An uncertain accepted Gate is reconciled before another attempt.

### Operational loop and owners

1. **PLAN:** select one authorized task from the latest source/DC; Taskern `check → compile` with bounded required context. Eligibility is not permission.
2. **IMPLEMENT:** the existing Coding Agent implements and tests through its own tools under [TESTING](../development/TESTING.md). Refine does not launch another implementation agent.
3. **FREEZE:** prepare a Caller-authorized candidate commit and record branch/worktree/diff. Only **PINNED_COMMIT** is supported; dirty/untracked changes are excluded and **FROZEN_WORKTREE remains HOLD**.
4. **PREFLIGHT:** prepare explicit operator config, environment, checks, outputs and finite limits; invoke native `refine gate preflight`. Exit 0 means the report was produced, not PASS. Resolve each FAIL/UNKNOWN with evidence and Caller decisions; retain UNKNOWN when static assessment cannot establish runtime/SDK/authorization/output behavior.
5. **GATE / RESULT:** invoke `refine gate --config ABS --request FILE --state-dir ABS --json`. Preserve `REFINE_GATE_ACCEPTED` and `gate_id`; inspect per-check results, logs, outputs and cleanup, then `gate verify`. SUCCEEDED, integrity success and Caller acceptance are separate facts.
6. **REPAIR:** diagnose the actual failure in the same Coding Agent session, test, pin a new candidate and authorize a new Gate within the grant. Lost response: `gate status`/`gate inspect`; use `gate recover` only for the applicable dead-owner case, without check replay. Never edit immutable Gate state or rerun an uncertain request blindly.
7. **REVIEW / ACCEPT:** submit the exact SHA/diff/DC constraints and verification evidence to Tern; resolve valid findings in the same conversation to **0 BLOCKING / 0 ACTIONABLE NON-BLOCKING / 0 EXCESSIVE**, or HOLD. Caller separately judges full acceptance. Review success does not authorize acceptance, dev integration, push, main merge or the next user-gated step.

Use public CLI/Skill guidance directly; do not make per-task JS/Python orchestration, dummy executors, a second validator or a new framework the standard. An environment helper needs a concrete demonstrated gap and separate scope; `trusted_local` isolation is not hostile-code confinement or OS-enforced network denial.

### Build storage and terminal retention

**Operational policy (2026-10-23):** keep ordinary local development's single workspace target separate from Refine's fresh, external check targets. Do not share mutable targets across independent checks or weaken the matching producer/Skill isolation contract to save space. Profiles/cache policy and the measured storage diagnosis are in [BUILD_AND_RUN §4.10](../development/BUILD_AND_RUN.md#410-storage-bounded-development-workflow-2026-10-23). This policy does not authorize a Gate, resume paused implementation, modify historical requests or grant deletion of existing data.

Before an authorized attempt, identify each exact scratch target and its owner, check available space against equivalent observed builds and the requested check count, and include post-terminal scratch disposal in the Caller grant. Reuse the already-qualified installed toolchain through absolute executables; do not copy/install rustup per attempt or let a private HOME silently acquire another toolchain. Where a subcommand genuinely needs rustup, qualify and explicitly approve its existing toolchain root/environment under the matching contract, with no implicit install or global-configuration change. Keep private HOME/TEMP and approved source-cache access separate from build targets.

After an attempt, terminal status, process reaping/cleanup and stored evidence integrity are separate prerequisites. Reconcile a lost response or interrupted owner first. Retain candidate/request/config/toolchain identity, accepted Gate ID, receipt, `inspect`/`verify` results, declared artifacts, private logs and required Native/source-binary evidence. Only then remove the exact caller-owned scratch targets covered by the grant, after checking that no process uses them and no required executable remains solely there. Record removed/kept paths and the observed cleanup result outside immutable producer records before the next storage-heavy attempt. Never report that Refine itself removed external target directories merely because its receipt says `cleanup: complete`.

Never remove or rewrite Gate state, artifacts, logs, retained checkout/quarantine, or nonterminal/cleanup-incomplete material as generic cache cleanup. In particular, existing `state/gates/<id>/private/home/.rustup` copies are **not** automatically disposable just because they contain a toolchain: first establish the matching producer's integrity/recovery scope and obtain exact deletion authority. Prevent new copies through qualified executable/environment preparation rather than deleting private state blindly. Failed attempts remain failed and preserved; scratch disposal is not acceptance or a retry.


## COS Runtime RC-01–17 crosswalk

The Desktop request register itself labels these Runtime-owned proposed changes, and says it does not establish acceptance, scheduling, or implementation (`docs/integration/COS_RUNTIME_REQUIRED_CHANGES.md:3,11-31,248-264`). Its initial survey anchors date from 2026-09-21; they are not refreshed by this report. “Desktop-side” below means protocol/client/UI draft or fixture only, never a real Runtime guarantee. **Neither Taskern nor Refine implements any RC.** They cannot close any Runtime gap or authorize its work.

| RC | Capability named by Desktop request | Present on Taskern / Refine side | Actual gap / authority |
|---|---|---|---|
| 01 | Runtime-owned cosd protocol adapter | Neither | Unimplemented/unapproved Runtime server adapter; Desktop client/fixture is not a server. |
| 02 | Desktop principal and credential issuance | Neither | Caller/Runtime security authority; no product identity or credential API here. |
| 03 | Read authorization and audience-bound observation | Neither | Runtime authorization/cursor binding; no substitute from Refine trusted-local policy. |
| 04 | Runtime epoch/session identity | Neither | Runtime identity and restart invalidation; product IDs/Refine sessions are unrelated. |
| 05 | Atomic snapshot cut and durable ordered delta journal | Neither | Runtime persistence/stream guarantee; Taskern Plans and Refine state do not project COS. |
| 06 | Per-projection revisions | Neither | Runtime-owned revision semantics; packet plan revision is not projection revision. |
| 07 | Structured protocol errors and correlation IDs | Neither | Runtime adapter contract; Taskern/Refine error envelopes are separate product protocols. |
| 08 | Dedupe retention and ReceiptExpired semantics | Neither | Runtime command receipt retention; Refine IDs/Taskern IDs provide no dedupe guarantee. |
| 09 | Mutating-command `expected_revision` preconditions | Neither | Runtime command admission/concurrency behavior; Plan revision is not a COS command precondition. |
| 10 | Stop intent/pending projection and reflected stop outcome | Neither | **Unimplemented/unapproved.** Refine cancellation applies to its own foreground execution only; it is not COS stop, and neither product projects/refers a COS stop command. |
| 11 | Real COS capability/grant projection | Neither | Runtime grant authority; Refine `required_capabilities` are local execution policy, not COS grants. |
| 12 | Scoped COS graph/Plan inspection | Neither | Runtime graph read authority; Taskern Plan is caller-authored planning input, not COS graph projection. |
| 13 | Media descriptor identity, integrity/audience, authorized fetch | Neither | Runtime media authorization/data plane; Refine artifact refs are local execution evidence and not COS media. |
| 14 | Frozen COS phase vocabulary / preserve unknown phases | Neither | Runtime status mapping and protocol client concern; Refine execution lifecycle enums do not map to COS phases. |
| 15 | Protocol/schema negotiation and capabilities | Neither | Desktop client has client-side draft/protocol support; real negotiated Runtime version/capabilities remain unimplemented/unapproved (register says client-side only). Product describe commands do not negotiate COS. |
| 16 | Desktop launch/lifecycle service integration, not checkout | Neither | Runtime service and authorized launcher; Refine's private Git execution checkout has opposite scope and does not provide COS lifecycle. |
| 17 | Task-scoped resumable SSE binding (if selected) | Neither | Runtime transport/persistence prerequisite after RC-05; no Refine/Taskern stream or durable COS delta cursor. |

Existing Desktop protocol types and fixture behavior may represent some desired client-side messages, but do not certify any RC server behavior. RC-06/10/14 being marked “partial” in the request register refers only to limited fixture/Runtime-survey overlap, not a Taskern/Refine implementation or authorization. Runtime revalidation must use its own current survey, symbol anchors, contracts, and REOS process before any proposed RC is acted upon.

## Compatibility and approval limits

Taskern 0.2's adapter compatibility concerns its existing TaskRequest/criteria subset, not GateRequest, GateReceipts, binary provenance, platform qualification or COS Runtime interoperability. Refine 0.3 adds a distinct Gate family while preserving delegated execution/refinement records, CLI2, semantic v1 and binding v1. Gate results are not refinement-session execution inputs. Claims about current release availability require their own release evidence, not a source version or old report.

This update exercised Taskern historical Plan admission, Refine static discovery and Tern review only. It did not run an MP-COR2 packet/implementation/Gate, qualify Windows MSVC/SDK or current Windows CI, launch COS Runtime, update dependencies, mutate either external repository, install Skills or approve product/release gates. Those remain separate authorized steps.