# Local development topology

Status: **Accepted** workflow; bootstrap endpoints/modes below are **Observed**, production plumbing remains **Proposed**.

| `<desktop-root>` | Independent Desktop sources/docs, local build outputs, committed fixtures/scripts/CI |
| `dio16/cos-desktop` | Desktop repository identity |

No production code/config may contain a Runtime checkout path as a launch/data dependency. Runtime owns storage/worker/server lifecycle. Desktop discovers a protocol endpoint, not a repository. No hardcoded internal Python launch. No direct Runtime DB/artifact DB/config reads, even as a convenient fallback.

## Change classes

A. **Desktop-only:** GPUI/layout/media presentation/Command Palette/local preferences, window management/accessibility/diagnostics; change Desktop only.

B. **Runtime-only:** canonical projections, Task semantics, GraphRevision authority, Outcome semantics, persistence/server behavior; requires a separate explicit Runtime task and Runtime commit. Record requests in [required changes](COS_RUNTIME_REQUIRED_CHANGES.md).

C. **Protocol:** review in order (1) contract, (2) server, (3) client, (4) compatibility, (5) migration. State each owner/change/release coordinate. Do not combine Desktop bootstrap and Runtime changes in one commit/work scope. Consume one versioned contract, not copied schemas or local path dependencies. [Ownership](PROTOCOL_OWNERSHIP.md) governs transfer.

## Implemented settings and modes

- `COS_DESKTOP_RUNTIME_ENDPOINT`: read at startup only for Mode::Runtime; value never printed/logged; if absent or a real transport is attempted, the default startup is disconnected/not configured rather than fixture. Only fixture mode loads bundled/pointed fixture JSON through provider code in background.
- Explicit `--fixture [--fixture-file PATH]`: only deliberate fixture invocation; UI persistently displays FIXTURE/simulated data. No endpoint setting or installed state can select fixture; any malformed argument exits without launching. Binary package selector: `cargo run --locked -p cos-desktop -- --fixture`.
- `COS_DESKTOP_LOG`: reserved by documentation only; not implemented as observed logging configuration.

No production launcher exists yet. A future development-only launcher remains opt-in, separately authorized, and visibly distinct from production startup.


## Freshness and safe Git procedure for later bootstrap

Before touching Git, resolve Desktop's git root. No repository is a normal uninitialized state: branch, HEAD, status and remote are N/A. If another parent repository owns this path, stop rather than initialize/nest. If the exact Desktop root is already independent, preserve its history and remote. Use `<desktop-root>` as an illustrative placeholder only. Check actual path containment, not shared drive-letter coincidence; Runtime and Desktop must be separate non-containing paths.

Record branch/HEAD/worktree/remotes for both checkouts at start. Recheck Runtime at end using read-only Git with optional locks disabled; report observed differences without attributing concurrent changes to this task. No Runtime build/test/install is allowed under this documentation task. Do not use remote creation/push as an implicit validation step.

Later bootstrap requires local commit(s), including Cargo.lock, once that implementation task is explicitly authorized. GitHub creation/push remains separately gated. Do not require a report to contain its own hash: identify the code commit verified, then the final report-bearing commit in terminal/external delivery. If code changes after verification, reverify the changed behavior before declaring its commit verified.
