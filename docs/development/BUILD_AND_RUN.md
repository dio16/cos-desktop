# BUILD_AND_RUN — COS Desktop

- **Status:** Environment observed; workspace/build/startup commands in **Observed** bootstrap implementation. Real Runtime connectivity remains Deferred.
- **Date:** 2026-09-21
- **Applies to:** `<desktop-root>` (COS Desktop). COS Runtime (`<runtime-root>`) is inspection-only: it is not built, launched, or addressed by any command in this document.

## Status vocabulary

| Marker | Meaning |
|---|---|
| **[Observed]** | Verified on the recorded workstation by command output or file read on 2026-09-21; the evidence is quoted. |
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
| Git | Installed under `<program-files>` | `git --version`, `where git` |
| Visual Studio | VS 2019 Build Tools and MSVC toolset `14.29.30133` | directory checks |
| vswhere | Installed under `<program-files>`; `-latest -products * -format json` returns `[]` | command output; anomaly recorded in ADR-0001 D3 |
| Windows SDK | Only `10.0.19041.0` (Include, Lib, bin). `rc.exe` is present under the SDK bin directory | directory checks |
| SDK import libs | `d3d11.lib`, `dxgi.lib`, `dwrite.lib`, `dcomp.lib`, `d3dcompiler.lib`, `d2d1.lib` under the SDK library directory | directory checks |
| Spectre-mitigated libs | Not present under the selected MSVC toolset | directory check |
| LLVM / clang | Not installed; `clang` not on PATH | directory/PATH checks |
| CMake | Not found on PATH; no VS-bundled CMake at the documented location | PATH/directory checks |
| GPU | NVIDIA RTX 2080 SUPER, driver `595.97` | `nvidia-smi` |
| Vulkan loader | Present; the pinned GPUI Windows backend uses Direct3D 11 (see ADR-0001 D3) | file check |
| Disk | System drive has 69.7 GB free | `Get-PSDrive` |
| sccache | Present under `<user-home>\.cargo\bin` | `where sccache` |
| Global cargo config | `<user-home>\.cargo\config.toml` sets `build.rustc-wrapper = sccache.exe`, `net.git-fetch-with-cli = true`, and for Windows targets `rustflags = ["-C", "link-arg=-fuse-ld=lld"]` | file read; risks in §5 |
| Environment variables | `RUSTFLAGS`, `CARGO_HOME`, `RUSTUP_HOME` unset | `echo` |
| Long paths | `core.longpaths` unset (global and system) | `git config --get` |
| Desktop repo contents | Documentation only; no `Cargo.toml`, `rust-toolchain.toml`, `.cargo/`, or sources | directory listing |

Context, read-only: the COS Runtime Rust workspace was recorded as `resolver = "3"`, `edition = "2024"`, `rust-version = "1.97.1"` **[Observed]**. That is Runtime-owned; it is recorded here only because it explains why `1.97.1` is the natural fallback experiment in ADR-0001 D2, not because Desktop shares code with it (it must not).

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

### 4.1 Storage-bounded development workflow

This stable-branch backport changes build storage and Agent operations, not the product source baseline. Full quality and Native requirements remain owned by [TESTING](TESTING.md); the [workflow boundary](../integration/REFINE_TASKERN_EXECUTION_BOUNDARY.md#build-storage-and-terminal-retention) owns Gate evidence and scratch retention.

```powershell
# Inner edit loop: format, metadata, dependency checks and type checking; no link/test claim.
pwsh scripts/quality.ps1 -CheckOnly

# Completion: unchanged full quality gates, followed by actual Native smoke from TESTING §1.
pwsh scripts/quality.ps1

# Full debugger type/variable information only when needed, in the same local target.
cargo --config profile.dev.debug=2 build --locked -p cos-desktop
```

`cargo check` skips final code generation/linking and cannot replace tests, link checks or Native startup. The no-switch quality command retains formatting, metadata, dependency checker/self-test, clippy, workspace tests and build. Use affected behavior checks during implementation and the required full route at completion.

The workspace development profile uses `debug = 1` and `incremental = false`; tests inherit it. Debug assertions, overflow checks and optimization defaults remain unchanged; release/benchmark profiles are unchanged. This trades some changed-crate rebuild speed for less retained debug/incremental storage. Record effective environment/config overrides rather than assuming the manifest always wins.

- Ordinary local development reuses one workspace `target`; do not create a target per edit, Agent, review round or feature toggle. Profile changes do not remove older artifacts. A one-time cleanup requires exact scope and preservation of any still-required executable; do not clean before/after every local command or while a process uses the tree.
- Independent Refine checks retain their required fresh external targets. Do not replace real builds with copied executables or `cargo check`, share mutable independent-check targets, or archive a whole build tree for evidence.
- For a successful authorized Gate, follow the [Gate-only workspace disposal procedure](../integration/REFINE_TASKERN_EXECUTION_BOUNDARY.md#successful-gate-workspace-disposal): preserve the fixed Git source and durable private evidence, verify terminal/process/evidence conditions, remove only the exact approved `work/repo`/scratch, then verify stored integrity again. Failed/recovery material and delegated execution/Refinement checkouts are excluded. Refine 0.3.1 does not remove the workspace automatically.
- Reuse the qualified toolchain rather than copying/installing rustup per Gate. A receipt's `cleanup: complete` does not mean external targets were deleted; record caller-side storage cleanup separately without rewriting immutable Gate records.
- CI has no target/Cargo-home cache action and explicitly disables incremental compilation. If caching is introduced later, limit it to dependency downloads (`registry/index`, `registry/cache`, and `git/db` for GPUI) and a size-bounded sccache, not `target`, credentials/configuration or duplicate compressed/extracted sources. sccache does not remove final linking or all build output.

Private evidence and predecessor history remain subject to the [publication boundary](evidence/README.md). This policy does not publish raw records, invoke a Gate, resume paused development or promote development product code to stable.

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

## 6. Troubleshooting (evidence-anchored)

| Symptom | Cause | Fix |
|---|---|---|
| Linker not found / `link.exe` resolution failures | rustup/`cc` discovery unable to see VS through the broken vswhere path | Launch a shell with `vcvars64.bat`, verify with `where link`, then rebuild. Alternatively point a repository `.cargo/config.toml` at the toolchain. |
| Resource-compiler error during `zed`-style manifest compilation (`invalid RC path`, “The system cannot find the path specified”) | `embed-resource` cannot locate `rc.exe`; upstream documents this exact class for the Zed binary | Point the resource-compiler lookup at the SDK directory `C:\Program Files (x86)\Windows Kits\10\bin\10.0.19041.0\x64` using the documented environment override, or ensure the developer environment is initialized. Record the chosen fix. |
| `error: failed to get ... as a dependency` with path-too-long | Deep `cargo` git checkout paths | Enable Git long paths plus Windows `LongPathsEnabled` (restart), then retry. |
| Direct3D device failure at startup | Cause not yet diagnosed | Capture error, adapter and feature-level evidence first. Do not assume driver update is required; driver/environment changes need authorization. |
| Build uses an unexpected compiler | `rust-toolchain.toml` absent, or rustup auto-installed a channel | Verify with `rustc --version`; the pin must be explicit and recorded. |
| Cargo tries to mutate the lockfile during `--locked` build | Lockfile not committed/refreshed | Refresh once without `--locked`, review the diff, commit, then return to `--locked`. |

## 7. Out of scope for this document (Deferred)

- Installers, code signing, updater, packaging, and distribution checks.
- Real runtime connectivity, media pipeline, and any runtime-side changes (owned elsewhere; see `docs/architecture/DESKTOP_PROTOCOL.md` and the integration documents).
- CI configuration. The proposed pipeline lives in [TESTING.md](TESTING.md); no workflow files are created by this delivery.
- Performance measurement; canon §37–38 targets are design targets and are **not measured**.
