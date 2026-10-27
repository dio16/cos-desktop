# BUILD_AND_RUN — COS Desktop

- **Status:** Environment observed; workspace/build/startup commands in **Observed** bootstrap implementation. Real Runtime connectivity remains Deferred.
- **Date:** 2026-09-21; current-status banner 2026-10-08
- **Applies to:** `<desktop-root>` (COS Desktop). The runtime repository `<runtime-root>` is inspection-only: it is not built, launched, or addressed by any command in this document.

## Current status (2026-10-08) — read this before the historical sections

- **[Observed]** The workspace, `Cargo.lock`, pinned toolchain `1.98.1` and GPUI pin `395def7a…` exist; §1 below records the pre-workspace 2026-09-21 environment and §3 the planned shape — both are **historical**, not today's state.
- **[Observed]** The commands in §4 are current (`cos-desktop` selector, `--locked` gates, explicit `--fixture`). Full gate results and startup evidence: [BOOTSTRAP_REPORT.md](BOOTSTRAP_REPORT.md) §5.
- **Real Runtime connectivity, media pipeline, packaging/update:** still **Deferred**; the Runtime path always starts disconnected (`apps/desktop/src/main.rs`).
- **Implementation planning:** [IMPLEMENTATION_MASTER_PLAN.md](IMPLEMENTATION_MASTER_PLAN.md).

## Status vocabulary

| Marker | Meaning |
|---|---|
| **[Observed]** | Verified on this workstation by command output or file read on 2026-09-21; the evidence is quoted. |
| **[Unverified]** | From official upstream sources at the pinned revision, not yet proven locally. |
| **[bootstrap-after]** | Command/procedure that becomes executable only after the bootstrap steps it depends on (toolchain install, workspace creation, lockfile). Not run yet. |
| **[Deferred]** | Out of scope for bootstrap; trigger stated. |
| **[Blocked]** | Requires an external change first; blocker stated. |

## 1. Observed environment (2026-09-21)

Commands were executed read-only from a normal user shell. Nothing was installed, changed, or launched.

| Item | Observation | Evidence |
|---|---|---|
| Rust compiler | `rustc 1.97.1 (8bab26f4f 2026-07-14)` | `rustc --version` |
| Cargo | `cargo 1.97.1 (c980f4866 2026-06-30)` | `cargo --version` |
| rustup | `rustup 1.29.0 (28d1352db 2026-03-05)` | `rustup --version` |
| Default host | `x86_64-pc-windows-msvc` | `rustup show` |
| Toolchains | `stable-x86_64-pc-windows-msvc` (active, default), `1.97.1-x86_64-pc-windows-msvc` | `rustup show` (no `1.98.1`; no nightly) |
| Installed targets | `x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu` | `rustup target list --installed` |
| Components | cargo, clippy, rust-analyzer, rust-src, rust-std (both targets), rustc, rustfmt | `rustup component list --installed` |
| Git | `2.51.0` | `git --version`, `where git` |
| Visual Studio | VS 2019 Build Tools; MSVC toolset `14.29.30133` | directory checks |
| vswhere | Present; `-latest -products * -format json` returns `[]` | command output; anomaly recorded in ADR-0001 D3 |
| Windows SDK | `10.0.19041.0` (Include, Lib, bin); `rc.exe` at `bin\10.0.19041.0\x64\rc.exe` | directory checks |
| SDK import libs | `d3d11.lib`, `dxgi.lib`, `dwrite.lib`, `dcomp.lib`, `d3dcompiler.lib`, `d2d1.lib` under SDK `Lib\10.0.19041.0\um\x64` | directory checks |
| Spectre-mitigated libs | Not present under the MSVC toolset's `lib\spectre\x64` | directory check |
| LLVM / clang | Not installed; `clang` not on PATH | directory/PATH checks |
| CMake | Not found on PATH; no VS-bundled CMake at the documented location | PATH/directory checks |
| GPU | NVIDIA RTX 2080 SUPER, driver `595.97` | `nvidia-smi` |
| Vulkan loader | `vulkan-1.dll` present; pinned GPUI Windows backend uses Direct3D 11 (see ADR-0001 D3) | file check |
| Disk | `69.7 GB` free on the build volume at observation time | `Get-PSDrive` |
| sccache | Present under `<user-home>/.cargo/bin/sccache.exe` | `where sccache` |
| Global cargo config | `<user-home>/.cargo/config.toml` sets `build.rustc-wrapper = sccache.exe`, `net.git-fetch-with-cli = true`, and Windows `rustflags = ["-C", "link-arg=-fuse-ld=lld"]` | file read; risks in §5 |
| Environment variables | `RUSTFLAGS`, `CARGO_HOME`, `RUSTUP_HOME` unset | `echo` |
| Long paths | `core.longpaths` unset (global and system) | `git config --get` |
| Desktop repo contents | Documentation only; no `Cargo.toml`, `rust-toolchain.toml`, `.cargo/`, or sources | directory listing |

Context, read-only: the runtime Rust workspace (`<runtime-root>/orchestrator/rust/Cargo.toml`) is `resolver = "3"`, `edition = "2024"`, `rust-version = "1.97.1"` **[Observed]**. That is runtime-owned; it is recorded here only because it explains why `1.97.1` is the natural fallback experiment in ADR-0001 D2, not because Desktop shares code with it (it must not).

## 2. Requirement matrix for the bootstrap

| Requirement | Source | Local state | Action |
|---|---|---|---|
| Rust stable `1.98.1` | Upstream `rust-toolchain.toml` at the pinned SHA | **Missing** (only 1.97.1) | Installation is a later operator action; see [GETTING_STARTED](GETTING_STARTED.md) phase 2 |
| MSVC C++ build tools | Upstream GPUI Windows path + local evidence | Present (VS 2019 Build Tools, MSVC 14.29) | none now; linkage discovery unproven until first build |
| Windows SDK | Upstream Zed Windows guide asks ≥ `10.0.20348.0`; crate-level evidence only needs D3D11/DirectWrite/etc. | `10.0.19041.0` only | try first build **[bootstrap-after]**; if linking fails, record and install a newer SDK |
| `rc.exe` for `embed-resource` (`windows-manifest`) | `gpui_platform` → `gpui/windows-manifest` at pinned SHA | Present at SDK `bin\10.0.19041.0\x64`; discovery via vswhere is suspect | try first build; if resource compilation fails, see §6 troubleshooting |
| D3D11-class GPU driver | `gpui_windows` Direct3D 11 backend | Present (RTX 2080 SUPER, 595.97) | none |
| Git | dependency fetch | Present (2.51.0) | none |
| Network access to `github.com/zed-industries/zed` | git dependency pin | Not exercised | first `cargo fetch` **[bootstrap-after]**; long-path support may matter |
| CMake, LLVM | Zed-guide items tied to `wasmtime`/macOS `bindgen` | Absent | **[Proposed]** not required for this dependency set; re-evaluate if dependencies change |

## 3. Planned workspace and pins

The physical shape is fixed by [repository architecture](../architecture/REPOSITORY_ARCHITECTURE.md); nothing below exists yet. The dependency pins are decided in [ADR-0001](../decisions/ADR-0001-GPUI.md). The member list is the eventual minimum workspace, not permission to create empty crates before the native-window gate.

```toml
# Cargo.toml — shape only; not created yet [bootstrap-after]
[workspace]
resolver = "3"
members = ["apps/desktop", "crates/desktop-gpui", "crates/ui-model", "crates/app-core", "crates/cos-client", "crates/desktop-protocol"]

[workspace.package]
edition = "2024"
# rust-version stays unset until the bootstrap build proves the floor (ADR-0001 D2)

[workspace.dependencies]
gpui = { git = "https://github.com/zed-industries/zed", rev = "395def7a067b32ed598fcb3f048969ada9b54942" }
gpui_platform = { git = "https://github.com/zed-industries/zed", rev = "395def7a067b32ed598fcb3f048969ada9b54942" }
```

```toml
# rust-toolchain.toml — planned [bootstrap-after]
[toolchain]
channel = "1.98.1"
profile = "minimal"
components = ["rustfmt", "clippy", "rust-analyzer", "rust-src"]
targets = ["x86_64-pc-windows-msvc"]
```

Pin rules (binding): `rev` is a 40-hex commit, never a branch/tag/wildcard; `Cargo.lock` is committed; all build/test invocations use `--locked`; Zed crates always move together. The `rev` above is a candidate snapshot from 2026-09-21 — the committed lockfile, not this document, is the source of truth after bootstrap.

## 4. Build and run **[Observed]**

All commands run from the repository root. Binary package selector is `cos-desktop`; see the correction below.

```powershell
# Resolution and lockfile when manifests legitimately change (only without --locked, then commit)
cargo generate-lockfile

# Verify the pin resolved to the recorded revision
Select-String -Path Cargo.lock -Pattern 'zed-industries/zed'

# Metadata gate (fast, no compilation) — also input to the dependency-rules check
cargo metadata --locked --no-deps --format-version 1
python -B scripts/check_dependency_rules.py --self-test
python -B scripts/check_dependency_rules.py

# Debug build and local fixture/default smoke
cargo build --locked --workspace
cargo run --locked -p cos-desktop -- --fixture
cargo run --locked -p cos-desktop

# Release build when needed
cargo build --locked --release -p cos-desktop
```

Observed startup evidence is recorded in [BOOTSTRAP_REPORT.md](BOOTSTRAP_REPORT.md), not inferred from build success. The earlier draft used the planned package selector `-p desktop`; that name does not exist.
Notes:

- Step 1 is the only legitimate place `--locked` cannot be used. Afterwards the lockfile is committed and any drift is a diff.
- First dependency resolution/build size and cache benefit are unmeasured. Observed free space was 69.7 GB; adequacy is not proven. Record actual disk use and cache configuration during bootstrap.
- `desktop` is the planned binary package name; it becomes fixed only when the binary crate is created (`-p` selectors above use that planned name).
- Minimal-window validation comes first; later bootstrap grows the fixture shell. Real Runtime connectivity remains Deferred. Never address `<runtime-root>`, launch internal Runtime processes or read internal configuration. [ADR-0005](../decisions/ADR-0005-LOCAL-TRANSPORT.md) defines a future endpoint candidate, not a transport implemented in bootstrap.
- No CI configuration is created during documentation preparation. Windows CI and necessary developer scripts belong to stage 8 of the subsequent [bootstrap plan](BOOTSTRAP_PLAN.md).

### 4.1 Fixture-only delayed delivery and evidence trace

The MP-PUB1 B-03 fixture provider accepts `after_ms:<milliseconds>` with
`1..=86400000` milliseconds. The delay starts at each grant; only a transport
poll at or after the monotonic deadline may deliver it. The mock facade reports
an owner wake hint without changing `DesktopClient` or the generic transport.
Queued delayed frames keep a bounded follow-up hint; an idle fixture has none.

Delayed deltas must omit `transaction_id`, `cursor` and `predecessor`; the
existing scripted accumulator assigns fresh identities and the predecessor
chain. `after_ms_once:<milliseconds>` is reserved for exactly one
`read_invalidated` action and executes at most once in the entire transcript.
Replacement, reset, disconnect, invalidation and epoch changes cancel old
reservations; normal delayed steps re-arm on a new grant. A fixture has at most
1024 steps. These are fixture mechanics, not authenticated Runtime behavior.

For native evidence, explicitly set `COS_FIXTURE_TRACE` to a **new, nonexistent**
JSONL file and launch `cos-desktop --fixture --fixture-file PATH`. The fixture
opens the trace at its first background poll, never overwrites an existing file,
and records execution/PID/session/step identity, monotonic sequence and frame
delivery identities. It reserves the whole frame batch and flushes `delivered`
before any frame becomes consumable. Trace storage is capped at 8 MiB.
`command` records carry the exact `command_id` and payload kind of every client
dispatch the provider received, so a native run's Send count and identity come
from the provider record instead of being inferred from the rendered surface.

`cancelled` records name their producer-side cause (`replaced`, `disconnect`,
`reset_stream`, `authorization_changed`, `scope_changed`, `epoch_changed`,
`capacity`, or `closed`). Scope/authentication replacement initiated by the
client reaches the provider through `reset_stream`; this is a cancellation
path, not a claim that the producer observed the caller's higher-level intent.
The producer's `grant`/`withheld` events do not prove consumer authorization.
Separate `consumer_state` records observe the actual client's
`PublicationAccess` and connection after a pump, only when state/publication
changes. Native evidence must use these consumer observations together with
the rendered surface, never treat a produced grant as successful validation.

`FIXTURE_TRACE_NOT_PROVEN` in diagnostics, incomplete/missing/duplicate trace
records, overflow or a mismatched delivered frame count invalidates the run as
evidence. Trace I/O failure does not stop ordinary client delivery. Before/after
captures need stable pre/post trace reads and matching screenshot/AX contents in
the same process; two different runs do not prove an in-window repaint.
Unset `COS_FIXTURE_TRACE` after the fixture run. Real Runtime A19 remains
`NOT_RUN` until separately authorized and exercised.

### 4.2 MP-PUB2 incremental display verification

The shell's background subscriber now owns a per-consumer `ui-model::DerivationCache`.
It reuses immutable display rows by family, scoped identity and entity revision;
an unchanged owner-local snapshot marker skips family traversal. Generation,
epoch, cancellation and withheld authorization invalidate reuse. Installation
and rendering still require the exact current complete publication tag.
Candidate element/focus identity is the full scoped `CandidateRef`, not its
position; horizontal list updates restore the visible identity and pixel offset.

The existing binary flags `--fixture --fixture-file` were exercised with a stimulus representing 16 initial candidates, a task-only update, a front insertion and updated labels. Historical Native, pixel-comparison and correlated-trace records support the reported changed-path selection/focus/scroll preservation, but are private and not independently verifiable from this snapshot; see the [historical-records disclosure](evidence/README.md#historical-records). The recorded run used a new `COS_FIXTURE_TRACE` file and stable pre/post trace reads in one process. The normal binary reportedly exited 0 with `observation shutdown confirmed`; task-scoped foreground control was released. Broader keyboard/AX/DPI certification is not claimed.

The throwaway public presentation workload is historically described as exercising 10,005 authored Fixture rows and 100 updates against the compiled libraries, including a changed candidate amid task-only updates. It was neither a large-wire transport acceptance nor an R0/PUB4 representation experiment or ratified performance claim. The probe and binary were reported removed; supporting record is private (see [disclosure](evidence/README.md#historical-records)).

In a minimized Refine environment, the checker cannot discover bare `cargo`
through PATH. The existing MP-PUB1 route is reused: invoke the pinned Cargo
executable by absolute path for `metadata --locked --no-deps --format-version 1`,
write that unmodified JSON outside the candidate, then invoke the existing Python
checker with `--metadata` and `--root` set to the independent checkout.
Historical route-probe and R1 records are private; see the [historical-records disclosure](evidence/README.md#historical-records). The route was reported to pass without PATH or product/checker changes; R1 remained failed. This is launcher preparation, not a relaxed dependency rule or a new quality gate.


### 4.3 MP-NAV4 independent chrome and Stop

The shell renders six labelled read-only axes: Task Status, Inner Phase,
Iteration, task-scoped Artifact Availability, Connection/Freshness and Formal
Outcome. Missing projections remain Unknown/Unavailable. A Stop receipt never
rewrites Task Status or supplies a missing formal outcome.

`Request Stop` uses the existing app-core owner and admits one window-local
intent at a time. The intent is scoped to the task and observation generation it
was created for: switching task or generation retires it (releasing only a
reference the window actually acquired) and permits a new explicit Stop, while a
late reply from the retired intent cannot authorize or confirm the current one.
`Check receipt` is one single-flight explicit authorized read of the displayed
command identity, never a resend.

Its outcome is kept only as window-local attempt history (`found`, `not retained`,
`capacity`, `denied`, `unavailable`, `no longer current`) shown as a separate
auxiliary note beside the current authorized command state, so its own safe
diagnostic publication cannot erase it. A found or not-retained value is usable
only while that read's exact authorized publication is still current; the history
is never receipt truth, never exposes receipt content, and is retired with the
intent. Both controls use pointer activation and the existing Tab/Shift+Tab,
Enter/Space focus/activation path; no global shortcut was added.
Capability/session unavailability, ordinary admission capacity and receipt
lookup/reference capacity have separate safe reasons. A known command identity
survives unavailable/unknown outcomes; final window shutdown does not settle it.

Historical public-seam smoke and Native tier B records are private; see the [historical-records disclosure](evidence/README.md#historical-records). The recorded status is **NOT_RUN/HOLD** because provider live confirmation timed out (`active: false`); no NAV4 window, pointer, keyboard, screenshot or accessibility PASS is claimed. The unchanged startup commands above remain the manual entry points after confirmation. Real Runtime reflection (tier C) and Product acceptance (tier D) remain conditional under [DC-MP-NAV4](../architecture/DESKTOP_DESIGN_CONTRACTS.md#dc-mp-nav4).


### 4.4 MP-IMG1 local encoded input / native picker feasibility

The `media` crate accepts an explicit local file/reader capability on a background executor. It separates Generated/Fixture provenance, independent expected digests and computed SHA-256; no expected digest remains Unverified. Cache limits include live consumers after eviction. Representation keys never grant external acquisition or authorization.

```powershell
cargo build --locked -p cos-desktop --example media_picker
cargo run --locked -p cos-desktop --example media_picker -- --fixture-root <evidence-root>/img1-fixtures
# Optional transient exclusive lock on an owned direct Fixture file; no ACL/security change:
cargo run --locked -p cos-desktop --example media_picker -- --fixture-root <evidence-root>/img1-fixtures --lock-file locked.png
```

Supply a dedicated local Fixture directory containing only non-sensitive test inputs. The separate example accepts only its direct regular files and bounds encoded payload to 1 MiB. Its native picker has explicit cancel/unavailable states and never launches Runtime. PNG signature recognition is encoded-container feasibility only: no image validation, decode, rendering, production import action or supported-format decision. Historical Native approval, test-admission and consumer-smoke records are private; see the [historical-records disclosure](evidence/README.md#historical-records). Building the example alone is not a Native PASS.

### 4.5 MP-SEC2 diagnostic and trace safety

The debug-only `COS_FIXTURE_TRACE` route is a stateful, bounded redaction path: a 256-byte token buffer, no full-input copy/token vector, a saturating per-area quota (dock 4,096 events; rail 8,192 events) and a complete-line cap of 512 bytes including prefix, area, fields and newline. Sensitive context survives whitespace/control separators; split assignments, Bearer values, signed locators, quoted/absolute path values, detached signature continuations and multi-token path continuations cannot pass, while opaque namespaced identities and ordinary numeric fields remain visible. A quoted multi-word path retains continuation state through its closing quote; an unquoted path fails closed until a structured non-path field. An overlong component never clears active continuation state: a real closing quote is detected without retaining the token, an unquoted path keeps withholding later fragments, a long token beginning with an absolute path enters withholding, a long key=value/private-path prefix is classified with the same rules as its short form, an overlong separated secret fails closed, a sensitive key split from its separator still withholds the following value, and an overlong URL whose query/fragment appears after the retained prefix withholds the following token because a detached signature boundary cannot be established. Once a separated secret or late signed-URL boundary is unknowable, the rest of the line is withheld except an explicit Bearer marker; a complete same-token value does not hide following safe fields, and no later token length or classification can deactivate that withholding. The rail trace records query presence only, never raw search text. The trace is disabled unless the variable is set in a debug build; the same variable names the fixture JSONL trace. Product publication errors are closed `PublicationError` categories: server and local rejection free text is dropped, and the command map key equals the receipt `command_id`.

Startup and exit output is static; a synthetic private path or endpoint credential is not echoed. This is an in-process redaction/boundedness guarantee, not DD-22 retention/export policy and not evidence for Windows Error Reporting crash-report scope. Historical admission and capture records are private; see the [disclosure](evidence/README.md#historical-records).

### 4.6 MP-FLOW1 graph inspection

Launch `cargo run --locked -p cos-desktop -- --fixture`, press `Ctrl+3` or activate the accessible `Flow` button, then Tab to `Search graph nodes and edges`, the `Graph nodes` list, or the `Graph edges` list. The surface shows the exact `GraphRevision` (`graph_id` + version + digest) separately from the projection revision, uses virtualized rows with stable NodeId/EdgeId focus, and filters only visible inspection entries. There is no canvas fallback.

An authorized fixture with `initial_state.graphs = []` shows `No graph in the current authorized observation.`; disconnected startup shows `Graph data withheld — no current authorized observation.` in both lists. Historical Native observations are private; see the [historical-records disclosure](evidence/README.md#historical-records). Real Runtime graph (RC-12) remains blocked.

### 4.7 MP-REC3 local preferences and restart

Preferences are stored in `%LOCALAPPDATA%\cos-desktop\preferences.json`. Launch the fixture, select a surface and Viewed candidate, then close normally; relaunch and verify the surface, Task, Viewed/Compare identities and clamped geometry restore. A corrupt/unknown-version/oversized file restores defaults with a bounded stderr notice and no partial merge. Remembered identities absent from the current projection produce an inspector notice rather than substitution. The file contains only the closed DC-MP-REC3 schema; Runtime Task survival, crash-before-quit and DD-23 topology/DPI certification are not claimed. Historical Native observation records are private; see the [disclosure](evidence/README.md#historical-records).

### 4.8 MP-RT8 Desktop-local lifecycle

Both explicit Fixture and disconnected startup use the same per-user `%LOCALAPPDATA%\cos-desktop\desktop.instance` OS lock before initializing a window or observation owner. An existing owner makes the second process exit 3 with a static refusal; inability to establish ownership exits 2. Do not delete the lock file to recover: file existence is not ownership, and open handles/OS process termination govern release. Second-launch activation/argument forwarding is not implemented.

Quit synchronously closes observation/command admission and requests owner shutdown, while attempting the existing closed-schema atomic preferences save once off the UI thread. Observation and preferences are checked independently against absolute deadlines from the same callback start, each at most 100 ms (half the pinned GPUI future-wait allowance). `COS_DESKTOP_EXIT_BUDGET_MS` only shortens the observation deadline; zero is unconfirmed. Static diagnostics distinguish observation confirmed/unconfirmed/not-applicable/not-observed and preferences saved/failed/unconfirmed/not-observed. The historical Caller-authorized bounded-save decision prioritizes finite waiting over guaranteed latest-settings persistence; it cannot cancel a blocked OS write or promise a 200 ms limit on synchronous callback/OS I/O execution. Its record and the quality run are private; see the [disclosure](evidence/README.md#historical-records). Earlier failed/warning-bearing attempts are historical, not final PASS.

### 4.9 MP-R0A-S2B development-only foreground observation

The S2B observer is compiled only with the explicit `r0-s2b-instrumentation` feature, and that build refuses Runtime/non-fixture startup before constructing the GPUI application. Ordinary builds keep the existing behavior with no collector, worker, queue, probe or export.

```text
cargo check --locked -p cos-desktop --features r0-s2b-instrumentation
cargo clippy --locked --workspace --all-targets --features cos-desktop/r0-s2b-instrumentation -- -D warnings
cargo test --locked --workspace --features cos-desktop/r0-s2b-instrumentation
cargo build --locked -p cos-desktop --features r0-s2b-instrumentation
python -B -m unittest discover -s apps/desktop/devtools/r0/tests -p "test_*.py"
python -B apps/desktop/devtools/r0/s2b_capture.py --manifest MANIFEST --executable EXE --fixture FIXTURE --output-dir EMPTY_DIR --helper target/debug/examples/r0_owner.exe --profile EMPTY_PROFILE --timeout-ms 15000
python -B apps/desktop/devtools/r0/s2b_validate.py bundle --manifest MANIFEST --output-dir DIR --timeout-ms 15000
```

The feature maps to `gpui/profiler`, which adds the optional `hdrhistogram 7.6.0` package (MIT/Apache-2.0) to the lockfile for its profiling histograms. That dependency is present only because the feature is explicit; the default build does not enable the profiler journal.

`s2b_capture.py` calls the unchanged S2A `driver.launch()` in the same process and then independently validates its three original artifacts. The opt-in observer writes one fixed-name capture under the run's isolated `TEMP`, syncs and closes it, then creates its own empty completion directory; the wrapper copies that capture and verified marker, writes a sanitized `s2b-observation.json`, and creates the bundle's completion directory. The S2A result never gains S2B claims. Consumers require every completion marker and exact digests, and reject mismatched identity, wrong clock domains, closed/reused window lifetimes, invented draw/device/displayed identity, hidden queue or retention loss, and non-finite/duplicate/unknown/oversized data.

Capture is bounded to 30 s, 2,048 queued local events, 256 retained records, 1 MiB of public output, a 20 ms drain interval, a 500 ms final drain and a 1 s shutdown wait. A producer-reported `lost` count covers only offered/finalized journal positions; unoffered input and an unfinalized tail remain `UNKNOWN`. A failed or interrupted process or missing completion marker is incomplete evidence, not a successful sample. Observation never calls `on_next_frame`, requests an animation frame, or manufactures repaint demand.

### 4.10 Storage-bounded development workflow (2026-10-23)

This operational policy changes build storage, not Product/DC semantics or the paused MP implementation's authorization. Quality and Native requirements remain owned by [TESTING](TESTING.md); independent-check retention is governed by the [workflow boundary](../integration/REFINE_TASKERN_EXECUTION_BOUNDARY.md#build-storage-and-terminal-retention).

**Observed before this change:** recursive logical file sizes on this workstation, excluding filesystem allocation/compression effects. Nested rows below are not additive.

| Location / category | GiB | Finding |
|---|---:|---|
| Workspace `target` | 22.287 | Rebuildable output, not source or durable verification evidence. |
| `target/debug/incremental` (inside target) | 7.104 | Incremental state retained by the previous default development profile. |
| All `.pdb` files (inside target) | 6.396 | Debug symbols across current and obsolete build variants. |
| Six external Cargo target trees under `C:/tmp` | 7.598 | Separate clippy/tests/default/feature outputs remain after Gate completion. |
| Five private `.rustup` copies under Gate state | 3.562 | Toolchain duplication, distinct from target output and immutable receipts. |
| Local sccache | 7.750 | Installed 0.17.0 reports a 10 GiB maximum; not an unbounded cache. |
| User Cargo registry + Git sources | 1.641 | Download/source cache; not the main storage consumer. |

The complete `C:/tmp` tree was 12.750 GiB, including Gate state and other material; it is **not** a blanket deletion candidate. Volume free space was 102.54 GiB at observation. The initial workflow-only step deleted no existing artifacts and claimed no reclaimed space; the subsequent Caller-authorized cleanup is recorded below.

**Edit loop versus completion**

```powershell
# Routine feedback: format, metadata, dependency checks and type-check all targets.
# No application/test linking, test execution or Native claim.
pwsh scripts/quality.ps1 -CheckOnly

# Smaller feedback scope when only one crate changes:
cargo check --locked -p cos-client --all-targets

# Completion checkpoint: unchanged full gates, then actual Native smoke from TESTING §6.
pwsh scripts/quality.ps1

# Full local debugger type/variable information only when needed; same workspace target.
cargo --config profile.dev.debug=2 build --locked -p cos-desktop
```

`cargo check` skips final code generation/linking and cannot replace tests, link checks or Native startup. Run affected behavior checks while implementing; run the required full route before completion. Do not rerun the full build/Gate for every edit or status inspection, and never transfer old results to a changed candidate.

The workspace development profile now uses `debug = 1` (limited symbols) and `incremental = false`; tests inherit it. Debug assertions, overflow checks and optimization defaults are retained; release/benchmark profiles are unchanged. Disabling incremental compilation trades some changed-crate rebuild speed for bounded retained state and compatibility with sccache's Rust cache. Environment/config overrides take precedence and must be recorded: existing r9 Gates explicitly used debug 0 and incremental 0 and are not retroactively changed by these defaults.

**Targets and cleanup**

- Ordinary development uses the workspace's one default `target`; do not create a target per edit, agent, review round or feature toggle. Cargo already fingerprints features, compiler options and profiles. Record an intentional `CARGO_TARGET_DIR` override; do not silently redirect an independent Gate into the local target.
- Independent Refine checks retain fresh external targets as required by the current Skill/producer contract. Strict one-project-one-target does **not** apply across those checks. Budget for them, serialize storage-heavy verification, then complete the terminal retention procedure in the workflow boundary before starting another attempt.
- Build output is disposable only after preserving anything still required for Native execution, debugger use or source-to-binary evidence. A `target` can contain the only executable/helper needed for a paused task. Exact hashes/manifests, logs, Native bundles and Gate records remain durable evidence.
- Profile changes do not remove old fingerprints, PDBs or incremental state. A one-time `cargo clean --target-dir <confirmed-exact-workspace-target>` can reclaim them after the owner authorizes that exact target and any needed binaries are preserved. It forces a rebuild; never clean before/after every local command, delete by `target*` wildcard, or clean while a compiler/application uses the tree.
- Never prune private Gate directories by age/name alone. Current receipts may say `cleanup: complete` while caller-owned external targets remain; that field is not a disk-GC result. Existing private toolchain copies require the separate state-retention/integrity assessment described in the workflow boundary.
- A successful Gate's source checkout is not permanent evidence storage. Plan and finish the [Gate-only workspace disposal procedure](../integration/REFINE_TASKERN_EXECUTION_BOUNDARY.md#successful-gate-workspace-disposal): retain the full pinned commit under an authorized durable Git reference plus private reports/logs/artifacts, remove the exact approved `work/repo` only after the terminal/process/evidence checks, then verify stored integrity again. Do not retain another full copy of source and historical evidence for every Gate. Refine 0.3.1 does not remove this workspace automatically; failed/recovery workspaces and delegated execution/Refinement checkouts are excluded.

**Caches**

CI currently has no target or Cargo-home cache action and keeps that simpler policy; incremental compilation is explicitly disabled. No new cache service/setup dependency is required to reduce this workstation's disk use. If CI caching is introduced later, cache only dependency downloads (`registry/index`, `registry/cache`, and `git/db` for the pinned GPUI Git dependency) and an explicitly size-bounded sccache, never `target` or the entire Cargo home. Do not duplicate compressed/downloaded and extracted sources, cache credentials/configuration, or infer a cache benefit without measurement. sccache cannot eliminate final linking or all build output. Local caches/toolchains are reused rather than cloned per attempt where the approved isolation policy permits; no global tool configuration was changed.

**Observed verification of this change (2026-10-23):** the new `-CheckOnly` entry point completed successfully, then the no-switch quality entry point completed fmt, metadata, dependency checker/self-test, workspace clippy/tests/build with the new default profile. Verification used the existing real Python interpreter ahead of the Windows launcher, offline cached dependencies and the previously documented child-only empty `CARGO_ENCODED_RUSTFLAGS`; no global configuration was changed. With live task-scoped approval, the resulting default executable displayed both the explicit Fixture surface and the disconnected/data-withheld surface in separate temporary local profiles. Screenshots/AX confirmed both; normal window close returned exit 0 for both, with no remaining owned window. Temporary verification scripts/profiles were removed afterward. This is local build/startup evidence, not remote CI, a new Refine Gate, full Native certification, R0 measurement or resumed MP acceptance.

**Subsequent authorized cleanup (2026-10-23):** the machine-readable cleanup record is [retained privately](evidence/README.md#historical-records); it records exact removed paths, retained executable/PDB hashes and native Gate integrity reports before/after. Logical removed file sizes total 34.203 GiB: local build intermediates/obsolete variants 23.164 GiB, external check/build intermediates 7.477 GiB, and five private toolchain copies 3.562 GiB. Measured volume free space increased by **31.785 GiB**, from 100.246 to 132.031 GiB; logical file sums and actual volume-free deltas are distinct measurements.

The current local application, picker and owner helper plus their PDBs, and the latest real Gate default/feature executable/PDB pairs remain at their original paths (10 files), byte-identical. Existing historical evidence and immutable authority/artifact/log files were protected by 22,032 file hashes; all six affected Gates passed native `status`/`inspect`/`verify` checks before deletion and identical integrity reports afterward. No build/Native/Gate check was replayed, no new Gate was dispatched and no target/toolchain/binary copy was created for cleanup. Private process HOME/TEMP are not terminal-bound evidence and were not recursively treated as such; Windows virtual/dangling cache junctions caused the first planning-only scan to stop before deletion. Only the five explicitly verified private `.rustup` subtrees were removed. The historical Native helper's old byte identity was already absent before cleanup; the record distinguishes it from the retained current helper rather than claiming equivalence.

The next Cargo invocation will regenerate missing intermediates using the new storage-bounded profile and existing source/compiler caches. Do not rebuild merely to recreate the deleted cache: the already-exercised preserved executable and actual Gate evidence remain available. The disposable cleanup script and planning scaffold were removed after the durable result was written.

References: [Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html), [check limitations](https://doc.rust-lang.org/cargo/commands/cargo-check.html), [build-cache layout](https://doc.rust-lang.org/cargo/reference/build-cache.html), [configuration precedence](https://doc.rust-lang.org/cargo/reference/config.html), [Cargo-home caching](https://doc.rust-lang.org/cargo/guide/cargo-home.html), [sccache Rust limitations](https://github.com/mozilla/sccache/blob/main/docs/Rust.md).

## 5. Environment risks and how they will be handled

| Risk | Evidence | Handling |
|---|---|---|
| Global cargo config injects `-C link-arg=-fuse-ld=lld` for Windows targets | Observed configuration; lld/clang-cl not found on PATH | Record effective compiler/linker arguments before diagnosing. Do not assume a repository array override removes inherited flags: Cargo arrays can merge. Any later scoped override must be checked against effective invocation, and global changes require user authorization. No workaround was applied here. |
| `sccache` wrapper is global | Same file; `sccache.exe` exists **[Observed]** | Keep it; note it in build evidence so cache artifacts are not mistaken for fresh outputs. If it ever interferes with a build, the same repo-scoped override pattern applies. |
| Environment flags can affect config flags | RUSTFLAGS currently unset | Keep effective build flags explicit and recorded. Choose a scoped workaround only after observing a failure and checking Cargo precedence; do not prescribe an unverified config override. |
| vswhere reports no registered product while VS files exist | Observed | In a later authorized build session, initialize vcvars64.bat within the same cmd process that runs Cargo; invoking a batch file as a child of PowerShell does not import its environment into that parent. Verify actual linker/resource compiler discovery. |
| SDK `10.0.19041.0` vs upstream’s recommended ≥ `10.0.20348.0` | Upstream guide **[Unverified]**; local SDK **[Observed]** | Attempt the build; on link errors tied to newer APIs, install a newer SDK. Record the conclusion either way. |
| Long-path support uncertain | core.longpaths unset; no effective checkout test performed | If a later dependency checkout fails on path length, evaluate a scoped solution first. Git/OS setting changes require appropriate permission and recorded evidence; do not infer effective OS policy from an unset Git value. |
| No Spectre-mitigated libs | **[Observed]** | Not required by crate-level evidence for this dependency set; revisit only if a build demands them. |
| No LLVM/CMake | **[Observed]** | Not required by this dependency set (**[Proposed]**, see §2); revisit if `bindgen`/`wasmtime` enter the graph. |

### 4.11 Fixed interpreter and environment rules (2026-10-24)

These rules are fixed for every later session; they exist because the default `python` on this host was a broken app-execution alias that failed every script invocation with `No runtime installed that matches 3`.

| Rule | What was done | How it is verified |
|---|---|---|
| `python` must be the uv CPython 3.13.14 | The user PATH starts with `C:\Users\diobrando\AppData\Roaming\uv\python\cpython-3.13.14-windows-x86_64-none`, and the WindowsApps `python.exe`/`python3.exe`/`pythonw.exe` alias stubs were renamed to `*.dsh-disabled-20261024` so they can no longer shadow it | `python --version`, `python -c "import sys; print(sys.executable)"` and `python -B scripts/check_dependency_rules.py` all resolve the uv path and exit 0 |
| Shells that inherit an old PATH still get the uv interpreter | `%LOCALAPPDATA%\Microsoft\WindowsApps\python.cmd` forwards to the uv interpreter for cmd/PowerShell, and `~/bin/python` does the same for bash (Git Bash ignores `.cmd`) | plain `python --version` in bash, PowerShell and a registry-composed (new-logon) environment prints `Python 3.13.14` |
| Out-of-tree verification builds never share the repository target directory | stale artifacts from deleted Gate worktrees embed `CARGO_MANIFEST_DIR` and break fixtures (observed as a `cos-client` NotFound until `cargo clean -p cos-client`) | every export/scratch build sets its own `CARGO_TARGET_DIR` |
| Parallel streams are gated on a fixed commit, not on a live worktree | one gate ran while another stream edited sources; it was detected by mtime overlap, the evidence was discarded and re-taken from a pristine export | gate briefs require a clean tree, a fixed SHA and a scope check |
| Fresh dedicated target builds must disable sccache | with the inherited `rustc-wrapper=sccache.exe`, a build in a brand-new `CARGO_TARGET_DIR` panicked inside rustc (`rustc_mir_dataflow` index out of bounds while compiling `syn`) | set `RUSTC_WRAPPER=` for that build; the repository's warm target directory is unaffected, and the same export then builds and tests cleanly |
| PowerShell must resolve a real `python.exe`/`python.cmd`; an extensionless `python` is **not** proof | `~/bin/python` is an extensionless bash shim that exists because Git Bash ignores `.cmd`. A PowerShell command that resolves to it has no file association, so Windows raises the "choose an app to open this file" dialog in an interactive session and, when it cannot, runs a silent no-op: no output, `$?` True, `$Error.Count` 0 and `$LASTEXITCODE` left at its previous value. `scripts/quality.ps1` therefore never resolves a bare `python` (see §4.14) | `pwsh -NoProfile -File scripts/quality.ps1 -CheckOnly` prints the proven `python:` and `cargo:` resolution lines before any gate runs; with an extensionless shim first on PATH the gates still execute their real output |

Rollback for the shim rule: none is required for the repository — the shim is user-environment only. To remove the hazard entirely, delete `~/bin/python` and give bash the interpreter through `~/.bashrc` (`alias python='/c/Users/<user>/AppData/Roaming/uv/python/cpython-3.13.14-windows-x86_64-none/python.exe'`), which bypasses Windows file associations.

Rollback: restore the alias names (`python.exe.dsh-disabled-20261024` -> `python.exe`, likewise `python3`/`pythonw`), delete `python.cmd` from the WindowsApps directory and `~/bin/python`, and remove the uv entry from the user PATH. No repository file participates in this environment setup.

### 4.12 MP-R0B/C measurement readiness (2026-10-24)

**Preparation only:** no startup/idle/present baseline, stall or cadence measurement is executed here, no numeric SLO or threshold is set, and nothing here authorizes R0D ratification or a change to the manifest/result contracts. Fixed baseline `a93ac794a4ae574cbda5282a9198dcab5252ec38` (**OBSERVED** by `git rev-parse HEAD` at drafting time).

Labels: **OBSERVED** = verified on this baseline; **PROPOSED** = plan plus precondition, not executed; **NOT_MEASURED** = no valid measurement exists (reason given). `PROPOSED`/`NOT_MEASURED` are the [§11.2](../architecture/RENDERING_PERFORMANCE_VISUAL_ARCHITECTURE.md#112-machine-readable-result-contract) result terms; the [Status vocabulary](#status-vocabulary) above lacks them (**source conflict**, reported below).

**Blocking precondition — R0A first:** [DC-MP-R0B](../architecture/DESKTOP_DESIGN_CONTRACTS.md#dc-mp-r0b) and [DC-MP-R0C](../architecture/DESKTOP_DESIGN_CONTRACTS.md#dc-mp-r0c) both list MP-R0A as hard prereq; [DC-MP-R0A](../architecture/DESKTOP_DESIGN_CONTRACTS.md#dc-mp-r0a) exit requires a reproducible manifest/driver, retained hashes and the §11.2 result contract. **OBSERVED:** R0A is IN_PROGRESS — S1 is complete and S2A is a LIMITED TECHNICAL COMPLETION (see the plan's MP-R0A card), S2B/S3 tooling exists, S2B Native N01–N06 are `NOT_RUN` on this host (approved A0/A1 executables and a matching helper digest are absent), and no R0B/R0C result record exists. R0B/C formal measurement completion must not be declared before those R0A preconditions hold; missing probes block only their own claim.

**R0B — baseline capture, noise and endpoints (all need the locked Release shell and the R0A driver):**
- startup / idle / present baseline — **PROPOSED**; the [§10.1](../architecture/RENDERING_PERFORMANCE_VISUAL_ARCHITECTURE.md#101-reproducible-benchmark-contract-r0) startup milestones stay separate; present/displayed are **NOT_MEASURED** until an independent native present capture exists.
- noise floor, instrumentation on/off overhead, measurement floor — **PROPOSED**; repeated baseline-vs-baseline and feature-OFF/ON pairs, spread/confidence method, §11.2 `instrumentation`.
- warm/cold policy, repetitions, run order — **PROPOSED**; declare warm conditions and repeats, balanced or recorded randomized paired order; true cold reset is **NOT_MEASURED** unless a declared reset actually runs (§10.1).
- exclusion policy, clock and correlation — **PROPOSED**; excluded/failed/censored samples retained with reasons and counts, no silent tail deletion; clock resolution and cross-process/tool mapping stay **NOT_MEASURED** (S2B `common_clock` crosswalk); no cross-domain subtraction.
- stall-detector exploration settings — **PROPOSED**; manifest-declared exploratory settings plus raw gaps ([§10.2](../architecture/RENDERING_PERFORMANCE_VISUAL_ARCHITECTURE.md#102-additional-calibration-decisions)); their thresholds stay **UNRATIFIED**.

**R0C — Windows scheduling and stall attribution (same preconditions, plus a timestamped visible-change demand trace):**
- native stress matrix (continuous keyboard, held-key/repeat, pointer, scroll, popup, entity-update bursts, and keyboard+projection / scroll+projection combinations) — **PROPOSED**.
- controlled posted/foreground-task saturation with declared traffic and cadence; requested vs achieved producer cadence — **PROPOSED**; a missed producer schedule is not a faster renderer (§10.1).
- demand-to-present correlation by local frame serial (input → model mutation → invalidate → frame request/start → scene → submit → present), with coalescing and superseded-change accounting — **PROPOSED**; internal counters are not screen completion ([§11.1](../architecture/RENDERING_PERFORMANCE_VISUAL_ARCHITECTURE.md#111-windows-scheduling-endpoints-and-starvation-attribution-r0)).
- stall and restoration classification — **PROPOSED**; gap since the last new Present under continuous visible demand; idle/occluded/minimized/suppressed spans classified separately; a stall censored at observation end stays in accounting; restoration only through the declared bounded queue/cancel/restoration policy (DC-MP-R0C, §11.1).

**Shared items:** execution matrix = one aggregate per scenario × variant × machine × cache state × endpoint combination (§11.2); native endpoints = the §11.1 table — when an endpoint does not exist record `NOT_MEASURED`/`NOT_IMPLEMENTED` with provider/reason and null, never zero, and never relabel an internal counter as that endpoint (§11.1/§11.2); Release shell identity = executable SHA-256, repository SHA and dirty-state/content hash, GPUI pin, build profile/features/flags and toolchain (§10.1 Identity) — **OBSERVED** only for the 2026-10-08 shell ([TERN §10.1](TERN_BINARY_RENDERING_ANALYSIS.md#101-release-cos-build-identity)); a Release shell and fixture/build/environment hashes bound to baseline `a93ac79`, plus the warm/cold definition, are **NOT_MEASURED** in R0 terms; missing-endpoint reasons are carried in each run's `endpoints` map.

**Model/counter-based vs natively measured:** in-process request/start/scene/submit counts, local frame serial, queue depth, mutation/derivation timing and derived durations are COS observer counters (model/counter-based); process-launch/S2A lifecycle facts and, only when the sampler is explicitly enabled, `working_set` as a **sampled lower bound** are the available native observations. Independent present/displayed timing, input-to-photon, clock calibration/resolution and tool mapping are **NOT_MEASURED** (S2A endpoint table; S2B crosswalk `common_clock`/`unsupported`).

**Source conflicts (reported, not resolved here):** (a) §7.6 states all R0 tasks are `NOT_AUTHORIZED` until an explicit authorization, while the R0B card says measurement-authorized and R0C says `NOT_AUTHORIZED`; §19.14 grants R0B/C work — R0C's status and the preamble are unreconciled. (b) The R0A card still carries the 2026-10-23 "S2B HOLD" text while §19.14 releases the hold and §19.15–§19.17 record S2B/S3 work. (c) DC-MP-R0B puts a "present baseline" in scope while §11.1 present/displayed endpoints are currently unavailable — such a baseline cannot be called measured until an endpoint exists. (d) The [Status vocabulary](#status-vocabulary) above does not define `PROPOSED`/`NOT_MEASURED`, although §11.2 uses them, so this section declares its own label meanings.

Related: [TESTING.md](TESTING.md) §17–§18; [IMPLEMENTATION_MASTER_PLAN.md](IMPLEMENTATION_MASTER_PLAN.md) §7.6 and §19.15–§19.17; R0A evidence: [S1 completion](evidence/r0a-s1/technical-completion.json), [S2A endpoint availability](evidence/r0a-s2a/endpoint-availability.json), [S2B crosswalk](evidence/r0a-s2b/endpoint-source-crosswalk.json).

### 4.13 MP-R0A completion inventory and Native feasibility (2026-10-24)

Bounded inventory only: no measurement, no executable launch, no plan/DC change and no reconstruction of old evidence. Baseline `cf8f8de3a7f500a25a3d4318786d4eaa5b197a5a` (**OBSERVED** by `git rev-parse HEAD`). MP-R0A remains **IN_PROGRESS** (plan:340); [DC-MP-R0A](../architecture/DESKTOP_DESIGN_CONTRACTS.md#dc-mp-r0a) exit is "a replayer can reproduce aggregates from retained definitions/hashes; missing probes block their own claim only". Labels: **OBSERVED** = executed/recorded; **NOT_RUN** = not executed on this host; **NOT_MEASURED** = no valid measurement; **PROPOSED** = plan only. Tracked records: §19.14–§19.18 and the MP-R0A card of [IMPLEMENTATION_MASTER_PLAN.md](IMPLEMENTATION_MASTER_PLAN.md) (`plan:NNN` = that file's line); paths under `docs/development/evidence/**` are local and git-ignored (only [evidence/README.md](evidence/README.md) is tracked), so they are retained references, not public proof.

**Completion conditions (7):**

| # | Condition | Status | Artifact or command that would satisfy it | Record |
|---|---|---|---|---|
| 1 | S1 contract foundation | **OBSERVED** (A/B complete) | `apps/desktop/devtools/r0/contracts.py`; `python -B apps/desktop/devtools/r0/validate.py manifest FILE` (also `result`, `pair`); suite `python -B -m unittest discover -s apps/desktop/devtools/r0/tests -p "test_*.py"` | plan:343; local `evidence/r0a-s1/technical-completion.json` |
| 2 | S2A process driver | **OBSERVED** (limited completion) | `apps/desktop/devtools/r0/driver.py`; `python -B apps/desktop/devtools/r0/driver.py --manifest M --executable E --fixture F --output-dir D --helper H --profile P` | plan:344-345; local `evidence/r0a-s2a/technical-completion.json`, `endpoint-availability.json` |
| 3 | S2B observer and Native capture | **OBSERVED** (feature-gated observer; R5 repairs closed, Tern PASS 0/0/0) / **NOT_RUN** (Native capture N01–N06) | `s2b_capture.py` and `s2b_validate.py` plus the approved A0/A1 executables, matching helper and session in the Native table below | plan:1195-1197, 1202; local `native-verify-2026-10-24/SUMMARY.md` |
| 4 | S3 ledger/aggregation/import | **OBSERVED** (limited completion; Tern PASS 0/0/0) | `s3_ledger.py`, `s3_import.py`; `python -B apps/desktop/devtools/r0/s3_import.py import --result R --ledger L --bundle B --bundle-tag n02d`; `python -B apps/desktop/devtools/r0/s3_ledger.py ledger verify --ledger L`; `python -B apps/desktop/devtools/r0/s3_ledger.py summary --ledger L` | plan:1221, 1225 |
| 5 | Reproducible manifest/driver and scenario families (DC exit) | **OBSERVED** for replay mechanics; **NOT_MEASURED** for an observed aggregate | `python -B apps/desktop/devtools/r0/s2b_validate.py bundle --bundle-zip <abs> --bundle-tag n02d --timeout-ms 60000` reproduces `n02d` stdout byte-identically; the recorded run is NOT_MEASURED, so the reproduced aggregate is honestly empty | plan:1221; local `SUMMARY.md:35-44` |
| 6 | Retained hashes (§10.1 Identity, §11.2 evidence) | **OBSERVED** for bundle/build-record/manifest/A0/A1 digests (N07 16 PASS / 2 NOT_MEASURED); **NOT_MEASURED** for `source_identity_sha256` and historical ancestry | bundle `sha256:3a5bc6da…`, build record `sha256:dc4f2db1…`, `identity-n07.json`; no retained file carries the recorded `sha256:0c19d36e…` | plan:1195-1196, 1222; local `SUMMARY.md:91-101` |
| 7 | §11.2 result contract | **OBSERVED** as contract+validator (endpoints/metrics/resources/frames/instrumentation); **NOT_MEASURED** as an end-to-end observed run | `apps/desktop/devtools/r0/contracts.py:525-543` required run keys and closed statuses; `validate.py result` and `pair`; no OBSERVED run instance exists | `contracts.py`; [TESTING](TESTING.md) §17; plan:343 |

**Native admission cases (definitions: `native-verify-2026-10-24/00-recovered-scenarios.md`; status from the 2026-10-24 verification):**

| Case | Required executables / fixtures / session | 2026-10-24 status | Prerequisite to run here |
|---|---|---|---|
| N01 | approved A0 `sha256:fe4787b5…` (`apps/desktop/devtools/r0/approved-executables.json:29-35`), helper `sha256:fa04da89…`, `manifests/s2b-default-a0.json`, fixture, interactive session | **NOT_RUN** (`SUMMARY.md:27`) | A0 bytes, approved helper bytes and a session must exist |
| N02 | approved A1 `sha256:a86f3758…` (`approved-executables.json:52-59`), helper, `manifests/s2b-feature-a1.json`, fixture, session | **NOT_RUN** (`SUMMARY.md:28`) | A1 bytes, approved helper bytes and a session must exist |
| N03 | an N02 run plus provider-granted native operation (AX, screenshot, input) | **NOT_RUN** (`SUMMARY.md:29`) | N02 prerequisites plus a per-session provider grant |
| N04 | feature binary, helper, session; early close/cancel | **NOT_RUN** Native; replay PASS (`SUMMARY.md:30`) | same binaries/session; retained `n04b` replay already passes |
| N05 | feature binary, helper, session; pressure beyond 256 retained | **NOT_RUN** Native; replay PASS (`SUMMARY.md:31`) | same binaries/session; retained `n02d` replay shows retained 256 / dropped 131 |
| N06 | A0 and A1 settled runs for enabled/disabled cost | **NOT_RUN** / **NOT_MEASURED** (`SUMMARY.md:32`) | both binaries; independent CPU-time/frame-count attribution stays NOT_MEASURED |
| N07 | recorded bundle, build-record, source-identity, manifests, `n07_identity.py` | **16 PASS / 2 NOT_MEASURED** (`SUMMARY.md:33`) | no new artifact; the two gaps are historical ancestry and `source_identity_sha256` |

**Met:** S1, S2A (limited), S3 (limited), the S2B observer/R5 repairs, the §11.2 contract/validator and replay mechanics. **Blocked by an absent artifact or session:** S2B Native capture/N01–N06 entirely, a measured (non-empty) aggregate, and the two N07 identity items. None of these rows is MP-R0A-wide completion; no threshold, SLO, R0D ratification or product acceptance follows.

**Blocked-by list:** (1) approved A0 executable `sha256:fe4787b5…` absent; (2) approved A1 executable `sha256:a86f3758…` absent; (3) approved helper `sha256:fa04da89…`/670,720 B absent — the host helper is 616,960 B and the two recorded current digests disagree (`126bb3ed…` plan:1222 vs `ac50eab7…` `SUMMARY.md:27`); (4) no interactive session, and N03 additionally needs a per-session provider grant; (5) no OBSERVED run exists that could replay as a measured aggregate; (6) private bytes cannot be validated from the public checkout (`evidence/README.md`). Rebuilding an executable so it resembles a historical approved identity is prohibited (plan:1222); these are prerequisites, not a recovery plan.

**What R0B/C still needs after R0A (§4.12; all PROPOSED):** the R0A preconditions above, an endpoint availability table with present/displayed resolved or explicitly NOT_MEASURED, and a locked Release shell identity bound to the baseline. R0B then runs startup/idle/present baselines, noise floor, instrumentation overhead and measurement floor, warm/cold policy, repetitions/order and exclusion; R0C then runs the native stress matrix with declared saturation traffic, requested-versus-achieved cadence and demand-to-present/stall classification. Stop points: R0B/C formal completion cannot precede R0A; R0D requires both R0B and R0C; no threshold is set before R0D, and the "present baseline" cannot be called measured without an independent present endpoint.

**Conflicts (reported, not resolved):** (a) the R0A card still states "S2B/S3 … pending" and keeps the 2026-10-23 "S2B HOLD … explicit resume is required" (plan:340, 343, 346), while §19.14 releases the hold and §19.18 records the S3 limited completion with Tern PASS 0/0/0. (b) [evidence/README.md:13](evidence/README.md) still says the last recorded MP-R0A-S2B disposition is **HOLD**. (c) The helper-digest disagreement in the blocked-by list cannot be resolved from the records. (d) The card's "N06 PARTIAL" (plan:346) and the 2026-10-24 N06 **NOT_RUN**/**NOT_MEASURED** describe different dates and must not be merged. (e) The recorded `source_identity_sha256` has no matching retained file and the historical commits are absent from this history, while the plan cites them as identities. (f) S2A's card says the docs-only completion record "awaits final independent confirmation" (plan:345) although plan:344 already records R10 PASS 0/0/0.

### 4.14 MP-IMG2 GDI+ comparison probe and fail-closed gate entry point (2026-10-24)

**Records:** [plan §19.25](IMPLEMENTATION_MASTER_PLAN.md#1925-gdi-decode-candidate-and-fail-closed-quality-gates-2026-10-24) owns the disposition; [DC-MP-IMG2](../architecture/DESKTOP_DESIGN_CONTRACTS.md#dc-mp-img2) remains the behavioural contract. This section only records how to run the pieces.

- **Third decode candidate:** `apps/desktop/examples/decode_compare/gdiplus_probe.ps1` is an operator-run managed `System.Drawing` (GDI+) leg for the existing exploratory harness. Identity probe: `pwsh -NoProfile -File apps/desktop/examples/decode_compare/gdiplus_probe.ps1 -Version`. Measurement invocation is the harness one (`-InputPath`, `-OutputPath`, `-MaxPixels`); exit codes are 0 decoded / 2 unsupported (**output extension only**) / 4 oversize / 1 error, and the probe never emits `unsupported` or `corrupt` for an *input* because GDI+ makes no such distinction. Run it through the harness, for example `cargo run --quiet --locked -p cos-desktop --example decode_compare -- --cases <file> --candidate gdiplus=pwsh:<abs probe> --candidate wic=pwsh:<abs wic probe> --candidate ffmpeg=<abs ffmpeg> --max-pixels 20000000 --timeout-ms 60000 --out <record>`; the harness unchanged is schema 9.
- **Two GDI+ limits to keep in view when reading a record:** an oversize refusal happens *after* the first full-frame allocation (`-MaxPixels` bounds the second copy and the encode, so a 65-byte 20000×20000 claim still allocates ~1.2 GiB first), and a corrupt zlib payload is not rejected by GDI+ at all — only the harness's independent pixel oracle (`pixels:` on the case) exposes it, so a structure-only case can read `structure_verified` for a broken file. Do not cite `scratch_max_bytes` as decoder scratch: it is the peak of the two capture files. `working_set_max_bytes` is an opt-in `sampled_lower_bound`, never peak RSS.
- **Quality entry point now fails closed:** `pwsh scripts/quality.ps1` resolves **and proves** `python` (`python.exe` → `python.cmd` → `python.bat` → `py.exe` → `py.cmd` → `py.bat`, each answering a bounded `-c 'print(...)'` probe) and a real `cargo.exe` (a `cargo.cmd`/`cargo.bat` shell launcher is refused, because such a wrapper can answer `--version` and still no-op every subcommand; resolution also requires `cargo fmt --version`). It prints both proven paths, uses those absolute paths, resets `$LASTEXITCODE` per gate, and then requires each gate to prove execution: `metadata` must return a JSON document with `packages`, `clippy`/`check` and `build` must print `Checking`/`Compiling`/`Finished`, `tests` must print `test result:`, and the two dependency-rules gates must print `Ran N tests` / `allowlist edges verified for N workspace member(s)`. A gate that leaves no status or no signature fails the run; `fmt` is silent on success by design and is proven through `cargo fmt --version` at resolution time. The probes run with the repository root as working directory, so the proven `cargo` is the toolchain `rust-toolchain.toml` pins for the gates rather than whatever the caller's cwd would select (observed difference: `1.97.1` from `C:\tmp` vs the pinned `1.98.1`). An unprovable interpreter/tool aborts the run with an explicit message instead of skipping a gate. Cost: one bounded probe per candidate (20 s cap each) — up to six Python candidates plus one cargo, so the worst case is roughly two minutes on a host where every earlier candidate is a hang; a host whose only Python or cargo is an extensionless launcher now fails closed.
- **Verification commands used for that fix:** `pwsh -NoProfile -File scripts/quality.ps1 -CheckOnly`, `pwsh -NoProfile -File scripts/quality.ps1`, and negative runs with a process-local PATH pointing at a broken shim (expect exit 1 with the explicit message). The native startup smoke remains a separate manual gate (§4.10, TESTING §6).

### 4.15 N06 attribution probe (2026-10-24)

Current reading for the N06 admission case; the §4.13 table stays as the 2026-10-24 inventory and is not rewritten. [Plan §19.26](IMPLEMENTATION_MASTER_PLAN.md#1926-n06-attribution-probe-and-the-measured-a0a1-comparison-2026-10-24) owns the disposition and every number; [DC-MP-R0A](../architecture/DESKTOP_DESIGN_CONTRACTS.md#dc-mp-r0a) owns the exit.

```powershell
# one attribution record for a live window's process
python -B apps/desktop/devtools/r0/n06_attribution.py attribute --pid <pid> --seconds 8 --interval-ms 20 `
    --out <record.json> --executable-sha256 sha256:<approved executable digest>
# compare two records (descriptive, no verdict is emitted)
python -B apps/desktop/devtools/r0/n06_attribution.py compare --a0 <a0.json> --a1 <a1.json> --out <comparison.json> `
    --qualification "<one bounded qualification string>"
# the R0 suite
python -B -m unittest discover -s apps/desktop/devtools/r0/tests -p "test_*.py"
```

- **Endpoints (frame side):** observed sampled window-content differences (`sampled_window_content_changes`: how often the sampled capture differed, with an unknown relationship to presented frames — `PrintWindow` asks the owning application to render, so this is **not** a frame count and **not** a compositor-frame lower bound), process CPU (`GetProcessTimes`, `process_over_window`), PDH GPU-engine utilisation (pid-attributed, ~1 Hz), and a desktop-wide DWM counter set that is `per_app:false` and refused on this host (`hr=0x88980090` → `DWM_TIMING_UNAVAILABLE`). The N06 independent-frame-count criterion stays `NOT_MEASURED`. `frames.app_displayed_frames` is always `NOT_MEASURED / NO_PER_WINDOW_DISPLAY_COUNTER`; `app_reported_stand_in` is `false` everywhere.
- **Identity:** a caller-supplied executable digest is recorded as `ASSERTED`, never as a probe measurement; `OBSERVED` survives only as a legacy tolerance for the pre-review retained records so they still replay.
- **Caps and seams:** seconds 1–120, interval 10–1000 ms, samples ≤10000, window ≤4 M px (CLI default; the accepted ceiling is 16 M), probe stdout read cap 1 MiB (a read cap, not a write quota), probe timeout seconds+30, record ≤4 MiB, absolute probe/interpreter paths, and a documented `--probe` seam so the real CLI is exercised against a fake probe.
- **Reading a record:** the validator enforces the closed shape, so a value without an allowed status, a status outside the vocabulary, a `NOT_MEASURED` carrying a non-zero value, `compositor_desktop_wide.per_app: true`, or an `app_displayed_frames` status other than `NOT_MEASURED` is refused as `COMPARE_INPUT`. A `DERIVED` ratio is descriptive over the declared window and becomes `NOT_MEASURED / CPU_BELOW_CLOCK_RESOLUTION` when the CPU numerator is zero. No cost conclusion follows from the recorded runs: the driven CPU delta reverses sign across the two repetitions (+78.125 ms, then −203.125 ms) and the driven sampled-change denominator swings 3 → 16, so the paired deltas are not stable and the comparison is descriptive only.

### 4.16 Output policy and canonical pixel transport for the decode comparison (2026-10-24)

[Plan §19.27](IMPLEMENTATION_MASTER_PLAN.md#1927-candidate-to-candidate-pixel-verification-for-png-bmp-and-jpeg-2026-10-24) owns the disposition. Two output policies exist and the default is the historical one:

- `--output-policy native` (**default**): each executable leg writes `{stem}.{candidate}.{case-suffix}`, exactly as before. A leg that writes a container the in-repo oracle cannot read stays `presence_only` (no expectation) or `structure_verified` (dimensions only) and is `pixel_unavailable`/`not_png` only when a raster was declared.
- `--output-policy canonical-png`: every executable leg is asked for `{stem}.{candidate}.png` (the script transport already writes `.<candidate>.wic.png`). `pixels_verified` then means "the SHA-256 of this leg's decoded 8-bit RGBA raster equals the operator's independently computed expectation, transported losslessly", which is what makes a declared raster verify every leg. It still does **not** describe the case container a candidate would have written natively, nor colour management, >8-bit or non-straight alpha.
- A failed normalisation is a closed refusal in canonical mode: a complete non-PNG container is `error` + `output_container_not_canonical` regardless of whether the case declares `pixels:`, only `expected:WxH`, or nothing, and contributes no capability row.
- The record carries `output_policy`, so the mode that produced a record is always identifiable. `pixel_digest` is recorded for every complete decode and `pixel_reason` explains every absent digest (`no_pixel_expectation`, `no_output`, `output_incomplete`/`output_unparsable`, `output_container_not_canonical`, or an oracle token such as `not_png`); no row carries a null digest with a null reason.
- Record-size admission includes the capability and resource summaries as well as
  observation rows. If those summaries make a prefix too large, collection shrinks
  the prefix and recomputes its summaries until the returned report and serialized
  evidence contain the same observations. The writer still refuses any inconsistent
  or oversized report; this does not turn a missing decode into a capability.

### 4.17 Refine Gate operations on this workstation (2026-10-24)

Refine 0.3.1 is now installed from source (`https://github.com/dio16/refine`, built with the repository's pinned toolchain) at `C:/Users/<user>/.dsh/tools/refine/bin/refine.exe`; the matching Agent Skill is installed at `~/.agents/skills/refine`. Independent Gates are therefore runnable again instead of `NOT_RUN`.

- **Operator config** (absolute, outside the candidate): `schema_version`, `git_executable`, an `executables` allowlist (cargo, python), `local_repository_roots`, empty `https_origins/ambient_env/secret_sources`, `limits` (`timeout_ms` ≤ 600000, the implementation cap) and a `state_dir` outside the repository. One working copy lives at `C:/tmp/refine/operator-config.json`.
- **Two Gates per candidate, matching the historical practice:** a *quality* Gate (`fmt`, `metadata`, dependency self-test, dependency rules, `clippy -D warnings`, `cargo test --locked --workspace`) and a *build* Gate (`cargo build --locked --workspace`). Each check gets its own fresh absolute `CARGO_TARGET_DIR` outside the checkout plus the explicit MSVC/SDK environment; the whole Gate has a 600 s ceiling, so splitting quality from build keeps each inside it.
- **Commands:** `refine gate preflight --config <cfg> --request <req> --json`, then `refine gate --config <cfg> --request <req> --state-dir <state> --json`; keep the stderr `REFINE_GATE_ACCEPTED` line and the gate ID, and read the report rather than the exit code (preflight exits 0 for `fail`/`unknown`). An actual Gate normally reports aggregate `unknown` at preflight because runtime compatibility, output delivery and caller authorization cannot be established statically.
- **Reserved environment lesson (measured):** Refine starts every check with an EMPTY PATH and does not accept PATH as request environment. The dependency checker therefore failed with `WinError 2` until it honoured an explicit `CARGO` program (see the resolution rule and its regression tests in `scripts/check_dependency_rules.py`, recorded in [plan §19.29](IMPLEMENTATION_MASTER_PLAN.md#1929-refine-installed-and-independent-gates-restored-2026-10-24)); any future check that shells out must take an absolute program path or an explicit variable, never PATH search.
- **First two accepted Gates:** build `77740749-4ada-43ef-999f-1ae3b3db87a4` on `5316c5e` (SUCCEEDED, 1/1) and a first quality attempt `bc223df1-b646-4908-9bd1-d47fa1b7d04f` on the same commit that FAILED at `dep-rules` (the PATH cause above) and stays recorded as history, never relabelled. After the checker fix, quality `26ff48f8-9451-4bb1-9830-096a0bfc934c` on `358843b` is SUCCEEDED with 6/6 checks.
- **Boundary:** a Gate receipt is independent verification of a pinned commit, not product or Caller acceptance, and not merge permission. A FAILED Gate is repaired by a new candidate commit plus an explicitly authorized new Gate; the old request is never mutated or silently redispatched.

### 4.18 MP-TYP1-E native typography acceptance runbook (human-driven)

Native pixel acceptance cannot be produced by an unattended session: it needs the user to foreground the window and to change the Windows display scale. The measurement path itself is automated and side-effect free, so the human work is only those two actions.

**Setup per scale.** Set Settings → System → Display → Scale to the step under test, then start the observation:

```text
python -B apps/desktop/devtools/typography/host_observe.py --out <evidence-dir>/host --wait-for-foreground 120
```

The observation launches **its own** fixture process, so the window to foreground is the one that appears when this command runs — not a copy started separately (`--exe` selects a different binary; `--powershell`, `--probe` and `--out` also exist, and `--help` on the pinned revision is the authoritative flag list). While the bounded wait runs, bring **that** window to the foreground and leave it visible; the harness never raises, focuses, moves, reorders windows or asks for foreground permission, it only reads which window is foreground and re-checks the handle, pid and pinned title each poll. A capture is accepted only when the image comes from the bound target window, the handle still matches and the sampled display agreement holds.

Do the steps in this order: **100 %, then 125 %, 150 %, 175 %, 200 %**, and repeat once with the window dragged to a monitor of a different scale.

**What each step must establish, and where it is recorded.** Body text size/line height and the Japanese/Latin mixed runs with no missing glyphs or clipping; the structural hairline actually drawn with a **DIP width of `1 / scale_factor` that resolves to exactly one physical pixel** at every scale (1.000 DIP at 100 %, 0.800 at 125 %, 0.667 at 150 %, 0.571 at 175 %, 0.500 at 200 %, each one physical pixel); the focus outline at 2 DIP and independent of the hairline; which face the run resolves to; High Contrast focus rendering; and text/layout frame cost. A step whose screenshot is refused (`WINDOW_OCCLUDED`, an occluded target) or cannot be judged stays `NOT_MEASURED`; a refused capture leaves no file and no digest by design.

**Boundary.** Until evidence exists for the steps above, MP-TYP1 stays **not complete**: the DPI matrix, the AX geometry (Task Rail 240, Inspector 320) and the label/mixed-script presence are the only OBSERVED typography results, and they are not pixel claims. A real rendering defect found by this runbook opens a new repair slice; the runbook itself changes no product code.

### 4.19 MP-R0C-S1 failed-result attribution (2026-10-26)

`r0c_driver.py` saves a failed workload through the same `finalize_result` path as
a successful workload. Missing and corrupt probe records keep their existing
precedence; otherwise the independently collected verification failures determine
the persisted primary reason before an ordinary binding loss. A binding-only
loss is `PROBE_BINDING_LOST`; a simultaneous PID, generation, launcher, title,
trace or classification mismatch retains its own higher-priority reason.

The result distinguishes `delivered_actions` (the accepted count) from
`probe_self_reported_delivered` and the original action detail (probe reports,
not proof of application processing). `probe_failures` retains all failed checks.
`delivery_attribution=CONFIRMED` requires no probe verification failure;
`UNATTRIBUTED` records that full verification did not succeed. Identity or trace
failure yields zero accepted deliveries even if action detail says `delivered`.
A legitimate binding-only partial run can retain its independently checked
pre-loss posting count while remaining `UNATTRIBUTED` and `FAILED`; it is never
a complete measurement. The public `verify` command refuses every non-`COMPLETE`
result, and successful results still require the original identity, correspondence,
cleanup and action-count checks.

N06's `attribute` CLI likewise validates a caller-supplied executable digest
before resolving PowerShell or the probe file: malformed identity returns
`ARGS_IDENTITY` even when those prerequisites are missing. A well-formed identity
with missing prerequisites still returns `PROBE_MISSING`; no probe is started.

Run the deterministic contract and driver regressions from the repository root:

```text
python -B -m unittest discover -s apps/desktop/devtools/r0/tests -p "test_r0c_*.py"
python -B -m unittest discover -s apps/desktop/devtools/r0/tests -p "test_*.py"
```

The [latest qualification records](evidence-published/r0c-s1-latest/behavior.json)
separate deterministic save/reload evidence from Native delivery and list every
skip. Eleven historical N07 assertions remain `NOT_RUN` because their original
baseline binaries are unavailable; a rebuild is not restoration. No Present,
displayed-frame, scanout, input-to-photon or GPU measurement follows from these
tests. Qualification and review status belong to the
[master plan](IMPLEMENTATION_MASTER_PLAN.md#1952-mp-r0c-s1-r15-source-qualification-2026-10-26).

### 4.20 MP-R0A-S3-N1 published Native journal replay (2026-10-26)

On Windows, replay the six tracked synthetic-input captures without launching the
application or requiring the missing historical N07 executables:

```text
python -B apps/desktop/devtools/r0/s3_native_replay.py --input-dir docs/development/evidence-published/r0c-2026-10-25
python -B -m unittest discover -s apps/desktop/devtools/r0/tests -p "test_s3_native_replay.py"
```

The reader checks all 37 `SHA256SUMS` entries, strict S2B capture contracts,
available observation/launcher/legacy S1 result identities, recorded evidence
references and the README's per-scenario counts/maxima. Historical CRLF evidence
references are accepted only by explicitly recomputing that documented byte form;
published checksum mismatches are never normalized away. The final README is
bound by `SHA256SUMS`, not its obsolete self-row in the historical correspondence
table. Original evidence remains unchanged.

Stdout is one canonical JSON envelope with a report digest. Its default and
maximum size is 1 MiB; `--max-output-bytes` may reduce it to at least 1,024 bytes.
Oversized output yields a closed error, not a partial report. The reader creates
no output directory, temporary file or child process. Redirect stdout explicitly
if a caller needs a file.

Each metric lists the retained source record sequence IDs, integer nanoseconds,
sample count, population and exclusions. Draw/submission durations, adjacent
start-to-start intervals and signed end-to-start gaps are separate. Gaps are
partitioned by window/lifetime; retained loss/power fences exclude cross-boundary
pairs. No scope-initial interval is invented. Invalidation totals are separated
by draw/submission kind and are not distinct-change totals or queue depth.

The six-record [replay](evidence-published/r0a-s3-native-latest/replay.json) and
[independent arithmetic oracle](evidence-published/r0a-s3-native-latest/independent-oracle.json)
retain the README denominator discrepancy: 48 navigation draws give 47 adjacent
gaps, whose maximum is 474,866,700 ns (474.867 ms rounded). The roughly 500 ms
driver action cadence is context, not a mapped native clock or visible-stall
measurement. Scroll has zero retained draws; its draw performance is
`NOT_MEASURED`, not a measured zero duration.

This is **published-record consistency and retained-journal aggregation**, not
full `s2b_capture.verify_bundle` replay. Original feature manifest, build/source
identity files, S2A result/completion/commit markers, executable/helper/fixture
bytes and the original launcher timeout declaration are absent from this input
set. Recorded source/executable identities agree where available, but independent
source/binary verification remains unavailable. No draw count is imported into
S3's frame counters. Present/displayed/GPU/input-to-photon, complete event
populations, visible stalls, queue/cancellation/restoration and full R0A/R0C
acceptance are not established.

### 4.21 MP-IMG2-W1 WIC decode admission before materialization (2026-10-26)

Budget mode is opt-in. Supplying any of the four new limits makes the harness
resolve the complete bundle from `media::DecodeBudget::ratified_default()` and
forward all five values to `wic_probe.ps1`; an unusable bundle is refused without
clamping. Omitting them keeps the byte-identical legacy argv, so existing
comparison records keep their meaning.

```text
cargo run --locked -p cos-desktop --example decode_compare -- \
  --cases <dir> --out <record.json> --candidate wic=pwsh:<abs>/wic_probe.ps1 \
  --output-policy canonical-png \
  --max-dimension 512 --max-bit-depth 8 \
  --max-decoded-bytes 1048576 --max-scratch-bytes 2097152
```

The probe checks the declared width/height, the closed pixel-format/depth
mapping, `max_dimension`, `max_pixels`, the per-row source decoded bytes
(`height * ceil(width * bits-per-pixel / 8)`), the BGRA32 conversion destination
plus the declared scratch allowance against the scratch ceiling, and the bundle's
own validity — with exact integer arithmetic — **before**
`BitmapDecoder.Create(..., CacheOption::OnLoad)`. Row-by-row sizing matters for
packed formats: a 1-bit 1×16 image requires 16 bytes, not the two a whole-image
bit total implies. WIC's own stride alignment and internal buffers remain
`NOT_MEASURED`. `Bgr565` reports its widest channel depth (6), not 5.

Presence is the operator's explicit binding: `-MaxDimension 0` (or any explicit
zero limit) is `invalid_limits`, never "flag absent"; a declared scratch
allowance without the four limits is also refused. A refusal exits 4 (`oversize`)
with a distinct reason code, or 1/2/3 for invalid limits, unsupported format and
invalid metadata; it writes no output. `admission_status`/`materialization` in
the probe's own JSON show the stage, and `matrix.json` records real cases, their
expected outcomes and a verification status.

The pure arithmetic and format table live in `wic_admission.ps1` and are
regression-tested without WIC or a file write:

```text
pwsh -NoProfile -File apps/desktop/tests/wic_admission_selftest.ps1
```

`run_matrix.py` now refuses an existing work root instead of deleting anything it
did not create, and exits non-zero when any recorded outcome contradicts its
expectation. Its cancellation check rejects a missing record, an empty or partial
observation list, duplicated case ids and a non-zero process exit instead of
treating `all([])` as success; `--selftest` exercises those negatives.

The BGRA32 conversion charge to `max_scratch_bytes` is a W1 interpretation, not
measured decoder behaviour. WIC-internal temporaries remain `not_measured`; the
probe never substitutes zero or an estimate, and no claim is made that a
container parse allocates nothing. `scratch_max_bytes` in the harness record is
still only the two capture files' peak, and `working_set_max_bytes` remains a
sampled lower bound. Real WIC tolerance is recorded, not wished away: a
truncated IDAT and a bad Adler checksum were accepted and decoded by this WIC,
a file cut inside IDAT was `unsupported`, and a zero-width IHDR was rejected as
`BADHEADER` before the probe's metadata check.

JPEG stays `structure_verified` only (no absolute raster claim). Region/
sequential/tiled capability, colour management, orientation, concurrent resident
memory, decoder scratch peak and in-decoder cancellation remain unmeasured. This
is not the full DC-MP-IMG2 Decode gate and not a memory-safety proof.

### 4.22 MP-IMG2-W2 bounded WIC resource and cancellation observations (2026-10-26)

This is a Windows **CLI WIC consumer**, not a GPUI preview, texture upload or
Native-window acceptance. [DC-MP-IMG2](../architecture/DESKTOP_DESIGN_CONTRACTS.md#dc-mp-img2)
owns its observation boundary; the complete inputs and observations are in
[`img2-w2-latest`](evidence-published/img2-w2-latest/tern-directive.txt).

```powershell
cargo build --locked -p cos-desktop --example decode_compare
python -B apps/desktop/devtools/img2/w2_resource_matrix.py run `
  --work-root <absolute-fresh-owned-work-root> --out <absolute-absent-report.json> `
  --pwsh <absolute-pwsh.exe> --harness <absolute-decode_compare.exe>
python -B apps/desktop/devtools/img2/w2_resource_matrix.py verify `
  --report <report.json> --work-root <owned-work-root> --source-root <repository>
pwsh -NoProfile -File apps/desktop/devtools/img2/w2_probe_selftest.ps1
python -B -m unittest discover -s apps/desktop/devtools/img2/tests -p "test_*.py"
```

The last command structurally replays the retained real Native record and its
negative mutations; it does not start a decoder. Supplying a work root to
`verify` additionally rehashes actual fixture, consumer, trace and output bytes.
Without it, report-only replay cannot re-verify discarded files. Raw Native
source hashes and explicitly LF-normalized Git-replay hashes remain separate.
Owned absolute path prefixes in the embedded consumer record are redacted;
its original/raw and redacted canonical identities are recorded separately.

The runner uses the existing W1 fixture generator and the real comparison
consumer's pixel oracle. PNG/BMP solid rasters have independent RGBA expectations;
JPEG remains structure-only. Its narrow experiment overrides are resolved through
the existing harness/`media::DecodeBudget`, not copied ratified defaults. The
manifest bounds case count, three **decode workers** (plus at most one managed
OS-counter sampler), per-worker/global time, work/output/trace/capture bytes and
sample count. An existing work root or report is refused without overwriting it.

Opt-in probe parameters are `-ObservationRoot`, `-TraceMaxRecords`,
`-TraceMaxBytes`, and optionally the paired `-PauseAtStage`/`-PauseTimeoutMs`.
The observation root must be fresh, empty and non-reparse. Boundary holds are
limited to `before_admission`, `before_onload`, `after_onload` and
`before_publish`; they are explicitly labelled artificial instrumentation.
They never occur inside `OnLoad`. Ordinary probe stdout, version identity and
exit meanings remain unchanged. Instrumented output is encoded to an exclusive
pending file, then moved without replacement only after successful completion.
Trace-cap failure, refusal and cancelled work cannot publish a successful result.

Stage rows identify the worker PID/start lifetime, observer and QPC frequency.
Parent lifecycle/cancellation uses its own monotonic-nanosecond domain; no
cross-process clock subtraction is performed. Entered/returned brackets include
trace-entry flush and call overhead, not pure decoder cost or an R0 latency claim.
OS `WorkingSet64`/`PrivateMemorySize64` reads refresh and recheck lifetime.
Individual observed maxima are `sampled_lower_bound`; a three-process total is
only a **derived sum within one bounded, non-atomic collection window**, not an
instantaneous concurrent value or peak bound. Runtime/shared pages and explicit
retention holds are included. Separate fields keep logical encoded/raster/BGRA/
declared-scratch requirements distinct from actual reservations, internal scratch
and true concurrent allocation peaks, which remain unmeasured.

Controlled cancellation observes a held boundary, verifies the process lifetime,
terminates and waits for the direct child, then removes only owned output.
Stop evidence is a closed state machine. `Worker.stop()` records the child's state
at the request (`alive_at_request`/`exit_before_request`), whether a terminate was
actually issued (`kill_issued`), any kill error, the reap result, the code that was
observed and whether an operator boundary hold was established (`hold_established`,
`cause`). A child that had already exited keeps its real outcome and is never
classified cancelled. A returning `kill()` proves only that termination was
requested, so `stop_confirmed` additionally requires the last trace row to be the
declared hold stage carrying `artificial_instrumentation_hold`: a parked probe
cannot publish or finish on its own. Every other stop -- including a successful
kill during OnLoad work where no hold exists, and any kill/natural-exit race -- is
`unverified_race` with outcome `not_verified`. Only `stop_confirmed` yields
`cancelled`/`timeout` and discards that worker's pending output.
`w2_resource_verify.py` rejects a cancellation record without those fields, a zero
exit, a missing hold marker, a cause/flag disagreement, or an exited-before-request
row claimed as cancellation. `apps/desktop/devtools/img2/tests/test_w2_worker_stop.py`
covers exited-before-request with exit 0 and exit 1, a confirmed kill, the
kill/natural-exit race, idempotent requests and both output-cleanup paths against a
fake process; the verifier negatives add the corresponding fabricated shapes.
Timeout is a distinct outcome. Genuine bounded attempts after `onload_entered`
record the last observed stage but do **not** prove the request occurred inside
WIC; cooperative in-decoder cancellation stays `NOT_MEASURED`. Three held,
overlapping workers establish process lifetime/retained-footprint observations,
not simultaneous decoder-execution peaks. Full Decode, 16K, region/sequential/
tiled/colour/orientation, JPEG absolute rasters and Upload remain separate exits.

The retained records were produced by the repaired runner in two fresh owned work
roots; the earlier records are superseded, not rewritten into success. The
independent oracle now reads the tracked [`native-rasters.zip`](evidence-published/img2-w2-latest/native-rasters.zip)
archive instead of a scratch work root, so pixel verification no longer depends on
a temporary path:

```powershell
python -B apps/desktop/devtools/img2/w2_independent_oracle.py `
  --matrix <native-matrix.json> --repeat <native-repeat.json> `
  --rasters <repository>/docs/development/evidence-published/img2-w2-latest/native-rasters.zip `
  --out <absolute-absent-oracle.json>
```

Its first repaired run was rejected by the stricter verifier for missing stop
observations on workers that were never stopped; that rejected record is kept as a
fail-closed detection, and only the two accepted runs bind the current evidence.

### 4.23 MP-IMG2-W3 real Bgr565 admission and raster proof (2026-10-27)

W3 reuses the existing managed WIC probe, comparison consumer and `crates/media`
budget resolution. Its fixed small BMP fixtures declare `BI_BITFIELDS` with
`F800/07E0/001F` masks; actual WIC source format, not that declaration, determines
whether a row can be certified as `Bgr565`. The `Bgr555` control is not a W3 success.

```powershell
cargo build --locked -p cos-desktop --example decode_compare
python -B apps/desktop/devtools/img2/w3_native_matrix.py run `
  --work-root <absolute-absent-owned-directory> --out <absolute-absent-report.json> `
  --pwsh <absolute-qualified-pwsh.exe> `
  --harness <repository>/target/debug/examples/decode_compare.exe
python -B apps/desktop/devtools/img2/w3_native_matrix.py verify `
  --report docs/development/evidence-published/img2-w3-latest/real-native-matrix.json `
  --source-root <repository>
python -B -m unittest discover -s apps/desktop/devtools/img2/tests -v
```

The portable unittest command also runs from `apps/desktop/devtools/img2` with
`-s tests`; no decoder dependency is installed. Qualify `RUSTC`, `RUSTDOC` and
`RUSTFMT` as well as Cargo: an absolute
Cargo executable alone does not pin the compiler chosen from PATH. Preserve the
unchanged full quality route; record any compiler/linker environment override.
Finish build/link commands before launching their consumer executable on Windows.

The manifest bounds twenty-four instrumented worker cases, one decoder at a time,
45 seconds per worker, 300 seconds overall, 1 MiB encoded input, 16 MiB per output,
64 MiB work, 128 trace rows / 128 KiB, 64 KiB per capture and 8 MiB report.
The separate ordinary consumer replays five of the same positive input cases.
Operator timeout waits at least five seconds from worker start before terminating
and reaping its held child; this is not a WIC cooperative timeout. Fresh work and
report paths are required. Work is disposable; retained JSON includes original
small BMP bytes, actual separately instrumented PNG bytes, traces and identities.

Budget-enabled 16-bit bitfield BMP admission validates a fixed-size header before
WIC materialization. This closes the observed `planes=0` tolerance without changing
legacy non-budget transport. Valid mask declarations alone never certify a format.
BMP encoded stride is four-byte aligned, while logical source bytes are summed
per row; WIC internal stride/scratch are not inferred from either.

The independent oracle extracts each 5/6/5 channel from the original payload,
expands by bit replication and verifies all RGBA samples after independent PNG
CRC/inflate/filter reconstruction. A mismatch remains a failure with bounded
coordinate/value diagnostics, never a new expectation fitted to the output.
Internal allocation peaks/reservations, cooperative cancellation, untested
dimensions/codecs, 16K, ICC/orientation, region/tiled behavior and full Decode exit
remain explicitly unmeasured. W3 is a limited format proof, not product-wide media
completion or GPU upload evidence.

Malformed and unsupported refusals additionally require the observed
`process_observed → input_opened → refused → probe_exit` population. Removing only
the `refused` row while keeping the terminal reason is a rejected negative fixture,
not a complete run; header admission also requires `corrupt / invalid_metadata /
refused` and terminal fields agreeing with the probe result.

### 4.24 MP-IMG2-W4 rectangle output and partial-decode classification (2026-10-27)

W4 adds an opt-in `-Roi x,y,width,height` and `-MaxRoiOutputBytes` to the existing
managed WIC probe. Bounds are strict decimal, checked with `BigInteger`, never
clamped, and refused before `OnLoad`. The full-frame decoded/conversion/scratch
admission still runs first; a 1×1 rectangle cannot reduce it. The rectangle is
copied from the existing `OnLoad` Bgra32 conversion, so it proves rectangle
output only: `true_partial_decode`, `sequential_decode` and `tiled_decode` stay
`NOT_VERIFIED`, and `internal_decoder_scratch` stays `NOT_MEASURED`.

```powershell
cargo build --locked -p cos-desktop --example decode_compare
python -B apps/desktop/devtools/img2/w4_region_matrix.py run `
  --work-root <absolute-absent-owned-directory> --out <absolute-absent-report.json> `
  --pwsh <absolute-qualified-pwsh.exe>
python -B apps/desktop/devtools/img2/w4_region_matrix.py verify `
  --report docs/development/evidence-published/img2-w4-latest/real-native-matrix.json `
  --source-root <repository>
python -B -m unittest discover -s apps/desktop/devtools/img2/tests -p "test_w4*.py"
```

The manifest bounds twenty-four cases, one decoder at a time, 45 s per worker,
300 s overall, 1 MiB input, 16 MiB output, 64 MiB work, 128 trace rows/128 KiB,
64 KiB capture and 8 MiB report. Positive cases cover full, center, both 1×1
corners, 1-pixel-wide/high, odd and boundary rectangles over PNG RGB/RGBA,
24-bit BMP and Bgr565, plus a Bgr555 control that is never certified as Bgr565.
Negatives cover negative/zero/outside/overflowing rectangles, ROI output above
its own bound, a 64×64 source above the full-frame decoded bound with a 1×1 ROI,
malformed/unsupported source, cancellation and timeout. The PNG/BMP oracle
reconstructs the source raster independently and slices the exact expected ROI;
WIC output never supplies an expectation.

## 6. Troubleshooting (evidence-anchored)

| Symptom | Cause | Fix |
|---|---|---|
| Linker not found / `link.exe` resolution failures | rustup/`cc` discovery unable to see VS through the broken vswhere path | Launch a shell with `vcvars64.bat`, verify with `where link`, then rebuild. Alternatively point a repository `.cargo/config.toml` at the toolchain. |
| Resource-compiler error during `zed`-style manifest compilation (`invalid RC path`, “The system cannot find the path specified”) | `embed-resource` cannot locate `rc.exe`; upstream documents this exact class for the Zed binary | Point the resource-compiler lookup at the installed Windows SDK `bin\<sdk-version>\x64` directory using the documented environment override, or ensure the developer environment is initialized. Record the chosen fix. |
| `error: failed to get ... as a dependency` with path-too-long | Deep `cargo` git checkout paths | Enable Git long paths plus Windows `LongPathsEnabled` (restart), then retry. |
| Direct3D device failure at startup | Cause not yet diagnosed | Capture error, adapter and feature-level evidence first. Do not assume driver update is required; driver/environment changes need authorization. |
| Build uses an unexpected compiler | `rust-toolchain.toml` absent, or rustup auto-installed a channel | Verify with `rustc --version`; the pin must be explicit and recorded. |
| Cargo tries to mutate the lockfile during `--locked` build | Lockfile not committed/refreshed | Refresh once without `--locked`, review the diff, commit, then return to `--locked`. |

## 7. Out of scope for this document (Deferred)

- Installers, code signing, updater, packaging, and distribution checks.
- Real runtime connectivity, media pipeline, and any runtime-side changes (owned elsewhere; see `docs/architecture/DESKTOP_PROTOCOL.md` and the integration documents).
- CI configuration. The proposed pipeline lives in [TESTING.md](TESTING.md); no workflow files are created by this delivery.
- Performance measurement execution and results; canon §37–38 targets are design targets and are **not measured**. §4.12 records only R0B/R0C readiness, preconditions and labels.
