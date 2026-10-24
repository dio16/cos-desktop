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

Rollback: restore the alias names (`python.exe.dsh-disabled-20261024` -> `python.exe`, likewise `python3`/`pythonw`), delete `python.cmd` from the WindowsApps directory and `~/bin/python`, and remove the uv entry from the user PATH. No repository file participates in this environment setup.

### 4.11 Fixed interpreter and environment rules (2026-10-24)

These rules are fixed for every later session; they exist because the default `python` on this host was a broken app-execution alias that failed every script invocation with `No runtime installed that matches 3`.

| Rule | What was done | How it is verified |
|---|---|---|
| `python` must be the uv CPython 3.13.14 | The user PATH starts with `C:\Users\diobrando\AppData\Roaming\uv\python\cpython-3.13.14-windows-x86_64-none`, and the WindowsApps `python.exe`/`python3.exe`/`pythonw.exe` alias stubs were renamed to `*.dsh-disabled-20261024` so they can no longer shadow it | `python --version`, `python -c "import sys; print(sys.executable)"` and `python -B scripts/check_dependency_rules.py` all resolve the uv path and exit 0 |
| Shells that inherit an old PATH still get the uv interpreter | `%LOCALAPPDATA%\Microsoft\WindowsApps\python.cmd` forwards to the uv interpreter for cmd/PowerShell, and `~/bin/python` does the same for bash (Git Bash ignores `.cmd`) | plain `python --version` in bash, PowerShell and a registry-composed (new-logon) environment prints `Python 3.13.14` |
| Out-of-tree verification builds never share the repository target directory | stale artifacts from deleted Gate worktrees embed `CARGO_MANIFEST_DIR` and break fixtures (observed as a `cos-client` NotFound until `cargo clean -p cos-client`) | every export/scratch build sets its own `CARGO_TARGET_DIR` |
| Parallel streams are gated on a fixed commit, not on a live worktree | one gate ran while another stream edited sources; it was detected by mtime overlap, the evidence was discarded and re-taken from a pristine export | gate briefs require a clean tree, a fixed SHA and a scope check |

Rollback: restore the alias names (`python.exe.dsh-disabled-20261024` -> `python.exe`, likewise `python3`/`pythonw`), delete `python.cmd` from the WindowsApps directory and `~/bin/python`, and remove the uv entry from the user PATH. No repository file participates in this environment setup.

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
- Performance measurement; canon §37–38 targets are design targets and are **not measured**.
