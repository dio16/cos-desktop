# Documentation preparation report

Status: **Observed preparation**, not implementation. 2026-09-21. This report is distinct from a post-implementation bootstrap report and records only what was inspected, preserved, accepted or proposed.

## Repository state

Desktop initial state: **Observed** 2026-09-21T01:00:19.464Z. `<desktop-root>` was not a Git repository: `rev-parse`, HEAD, status and remote commands failed; that was normal for an uninitialized Desktop. It contained only the original root canonical Markdown. No Git initialization, remote change or history creation was performed.

Runtime initial state: **Observed** 2026-09-21T01:00:19.464Z. `<runtime-root>`: branch `dev`, HEAD `f94985bd4c5a3a0714153f9d068f24439d57a1f2`, clean porcelain including untracked files, origin `<runtime-repository-url>`. Runtime reads were limited to authorized inspection; nothing was written, launched, committed, installed or tested there.

Final containment check in this task: **Observed** with direct read-only Git commands. Desktop remained uninitialized, so branch, HEAD, status and remote stayed N/A. Runtime remained branch `dev`, HEAD `f94985bd4c5a3a0714153f9d068f24439d57a1f2`, clean worktree output and the same origin remote. Desktop held only Markdown; no `.git`, Cargo manifest/source, fixture, script/config, CI or license/binary artifacts. Canonical final digest: SHA-256 `42dcd4c1314f5f5bc34f3035e930933314ce2f9f209dc19674d34ac80398268e` (40,773 bytes). No independent canonical text remained at the root.

## Evidence scope

Canon: root Markdown moved unchanged into `docs/architecture/COS_DESKTOP_PRODUCT_UX_ARCHITECTURE.md`, verified byte-equal, with root duplicate removed. All semantic mapping came from its full contents, including Product goals/non-goals, UX principles, state/candidate distinctions, shell/surfaces, conceptual/logical architecture, ownership, media/performance/security/diagnostics/versioning and golden workflow/UX validation/architectural invariants.

Runtime: direct read-only inspection of Runtime application sources, frontend sources, runtime docs and workspace manifests, anchored by symbols/file paths at HEAD `f94985bd`. Existing functionality and proposed conversion were distinguished throughout.

Environment: read-only local compiler/build stack and pinned upstream GPUI file evidence; no build/startup, installation or environment change. Pinned upstream toolchain file was verified directly: `channel = "1.98.1"`, profile minimal, components rustfmt/clippy/rust-analyzer/rust-src. Local installation contains only `1.97.1`, so the exact toolchain remains unavailable until a separately authorized bootstrap operator installs it.

## Created/arranged documents

Markdown only; no duplicates were retained. New user-authored files were not overwritten.

- [README.md](../../README.md)
- [AGENTS.md](../../AGENTS.md)
- [Repository architecture](../architecture/REPOSITORY_ARCHITECTURE.md)
- [Direct dependency rules](../architecture/DEPENDENCY_RULES.md)
- [Protocol Draft](../architecture/DESKTOP_PROTOCOL.md)
- [Process model](../architecture/PROCESS_MODEL.md)
- [Bootstrap plan](BOOTSTRAP_PLAN.md)
- [Getting started](GETTING_STARTED.md)
- [Build/run](BUILD_AND_RUN.md)
- [Testing](TESTING.md)
- [Runtime survey](../integration/CURRENT_COS_RUNTIME_SURVEY.md)
- [Required Runtime changes](../integration/COS_RUNTIME_REQUIRED_CHANGES.md)
- [Protocol ownership](../integration/PROTOCOL_OWNERSHIP.md)
- [Local workflow](../integration/LOCAL_DEVELOPMENT_TOPOLOGY.md)
- [ADR-0001 GPUI](../decisions/ADR-0001-GPUI.md)
- [ADR-0002 Repository separation](../decisions/ADR-0002-REPOSITORY-SEPARATION.md)
- [ADR-0003 Process separation](../decisions/ADR-0003-PROCESS-SEPARATION.md)
- [ADR-0004 Protocol boundary](../decisions/ADR-0004-PROTOCOL-BOUNDARY.md)
- [ADR-0005 Local transport](../decisions/ADR-0005-LOCAL-TRANSPORT.md)
- [ADR-0006 Media boundary](../decisions/ADR-0006-MEDIA-BOUNDARY.md)

Agent-created guide files at the root were relocated under `docs/development/` without changing their technical content; requested locations are preserved there. No BOOTSTRAP_REPORT.md exists, as required.

## Preservation, resolved conflicts and accepted terms

Canonical preservation: `PRESERVED`. No canonical sentence was edited for bootstrap convenience. Physical interpretation is separate: its workspace diagram is logical, Desktop's `media` is presentation-only, cos-client addition is transport separation, and technical bootstrap is not product UX approval.

Conflict handling: identical content was moved rather than merged. An already-arranged target would have stopped the move for manual review; final checks found only the intended Markdown documents and no duplicate canon.

Accepted non-negotiables include independent repositories/lifecycles; Runtime-only Task/selection/graph/plan/artifact/outcome/stop authority; enforceable allowlist plus checker intent; atomic observation/receipt/version policy; visible explicit fixture boundary; transport/UI security and local state ownership separation; and minimal-workspace/gated workflow ordering.

## Runtime findings and requested changes only

Runtime survey: `COMPLETE` as source evidence, `PARTIAL` for claims requiring future Runtime implementation or runtime-authoritative decisions. Existing implemented boundaries include task/catalog/candidates/evaluations/history/details/stop/receipt/media routes, wire enums, identities/scopes, actor-bound command receipts and stop authorization/reconciliation. Current frontend uses polling; it does not provide Desktop observation guarantees.

Required Runtime register: Proposed RC-01–RC-17, with owner/proposed API/compatibility/migration/Desktop dependency/priority and bootstrap-versus-real split. Bootstrap needs no Runtime implementation because it uses in-process fixtures; real integration needs runtime-owned adapter/identity/authorization/epoch/observation/revision/error/retention/revision-guard/reflection/capability/graph/media/version/service/lifecycle/stream work. No Runtime change was implemented.

## Unvalidated items and decisions

Unvalidated local/technical items: pinned GPUI source/version and rendering path were verified upstream but not built/started; local pinned toolchain `1.98.1` is missing; SDK `10.0.19041.0` versus upstream `>=10.0.20348.0`; vswhere registration anomaly; inherited lld link argument; sccache/long-path/credential/service behaviors; all contract/client/fixture/server behavior; performance targets; and media pipeline/browser-only scope handling.

Runtime gaps: existing restrictions are current-lane constraints, not Desktop-canon amendments; database/backup/identity-rotation/DB-restore replay guarantees are unverified; absence claims are scoped as specified in the survey; Rust details are survey-level; legacy phase vocabulary and resume-evidence coupling need Runtime decisions; cross-section numbering remained stable in its intended locations.

Required user decisions before relevant actions: Desktop license/rights holder; GitHub visibility/repository creation/initial push; exact toolchain/SDK/Build Tools installation and environment fixes; future protocol canonical location; SSE versus alternative real streaming binding; production discovery/pairing and credential/server authentication; Runtime-owned API/lifecycle acceptance and prioritization. These were deliberately undecided here; no LICENSE or GitHub repository was created.

## Prohibited-operation status

Runtime modification: **NO**. No `<runtime-root>` file was created/updated and no generated/install/test process was executed there.

Code or build configuration: `NO`. No Rust file, Cargo manifest/lock, toolchain file, CI YAML, executable script or fixture/data file exists in Desktop.

Git initialization/commit/push or remote/branch/config change: `NO`. Git commands were read-only state checks initially; end-state `hub wait` behavior is requested, but it cannot create commits or remotes. Environment/package/toolchain/SDK installation: `NO`. No application/service/transport/media execution: `NO`.

Ready for bootstrap: `CONDITIONAL`. Documentation-only entry is complete, but implementation requires a new explicit instruction and the operator/user decisions above. Minimal-native-window/toolchain evidence must precede any workspace/protocol/shell expansion.

Required decisions:

- Approve Desktop license/rights holder and GitHub creation/visibility/push policy before distribution/repository actions.
- Authorize exact toolchain/environment changes and select the first-build SDK path.
- Confirm Runtime-owned integration priorities and final protocol canonical home.

## Final documentation status

DOCUMENTATION RESULT:
PASS

Canonical design:
PRESERVED

Runtime survey:
COMPLETE

`<runtime-root>` modified by this task:
NO

Code or build configuration created:
NO

Git initialized or committed:
NO

Ready for bootstrap:
CONDITIONAL

Required decisions:
- Desktop license/rights holder and GitHub creation/visibility/push authorization
- Exact toolchain/SDK/Build Tools installation and environment fixes
- Runtime-owned integration priorities and final protocol canonical home
