# Getting started (COS Desktop)

- **Status of this document:** Initial. Describes the bootstrap order and what is already true; every future command is clearly labeled.
- **Audience:** contributors preparing the first Desktop code, and the reviewer validating bootstrap gates.
- **Date:** 2026-09-21
- **Canonical inputs:** [product/UX architecture](../architecture/COS_DESKTOP_PRODUCT_UX_ARCHITECTURE.md) (canon), [repository architecture](../architecture/REPOSITORY_ARCHITECTURE.md), [dependency rules](../architecture/DEPENDENCY_RULES.md), [protocol draft](../architecture/DESKTOP_PROTOCOL.md), [ADR-0001 GPUI](../decisions/ADR-0001-GPUI.md).

## Status vocabulary

| Marker | Meaning |
|---|---|
| **[Observed]** | Verified on this workstation by command output or file read on 2026-09-21; evidence cited. |
| **[Accepted]** | Decided and binding; see the linked ADR/architecture document. |
| **[Proposed]** | Recommended, awaiting the stated gate. |
| **[Deferred]** | Intentionally not started; trigger for revisiting is stated. |
| **[Unverified]** | Upstream source fact not yet proven locally. |
| **[Blocked]** | Requires an external change first. |

## Where we are now

- **[Observed]** Desktop contains Markdown documentation only. Build/test guides reside alongside this file. No Cargo workspace, source, fixture, script or CI file exists.
- **[Accepted]** GPUI is the UI framework and Rust is the language (canon cover), consumed as a revision-pinned git dependency (ADR-0001).
- **[Observed]** Rust `1.97.1` is installed; the pinned bootstrap toolchain `1.98.1` is not. Nothing has been built or launched.
- **[Deferred]** No workspace, protocol implementation, fixture client, or shell exists yet; COS Runtime is a separate repository, never a Desktop dependency.

## Phase order and status

| Phase | Goal | Status |
|---|---|---|
| 0. Inspect | Record environment and Runtime boundary | Observed in [BUILD_AND_RUN.md](BUILD_AND_RUN.md), [GPUI ADR](../decisions/ADR-0001-GPUI.md) and [Runtime survey](../integration/CURRENT_COS_RUNTIME_SURVEY.md) |
| 1. GPUI and licensing decision | Select exact source, revision, toolchain family, Windows dependencies, license posture | **[Accepted]** — [ADR-0001](../decisions/ADR-0001-GPUI.md); Desktop license itself remains **[Deferred]** |
| 2. Toolchain | Install and pin the exact compiler toolchain; make `rustup show` match the pin | **[Deferred]** — instructions below; only `1.97.1` is present |
| 3. Minimal native window | Create the smallest possible GPUI app that opens one window and renders text on the Windows GPU path | **[Deferred]** — not started; this is the first code in the repository |
| 4. Workspace / protocol / shell | Grow the planned crates, the fixture-tested protocol client, and a headless-testable shell | **[Deferred]** — shape is fixed by [REPOSITORY_ARCHITECTURE.md](../architecture/REPOSITORY_ARCHITECTURE.md), content by the bootstrap plan (`docs/development/BOOTSTRAP_PLAN.md`) |

The order is deliberate: each phase produces evidence the next phase depends on. Do not start a later phase to “save time”; a minimal window that has never run makes every later claim speculative.

These phases summarize [BOOTSTRAP_PLAN.md](BOOTSTRAP_PLAN.md), whose ten-stage order governs. State/Git checks precede authorized code work; GPUI/toolchain/minimal window are stage 3; workspace through quality are stages 4–8; documentation/report/local commit are stages 9–10. Environment facts are in [BUILD_AND_RUN.md](BUILD_AND_RUN.md).

## Phase 1 — GPUI and licensing decision **[Accepted]**

Read [ADR-0001](../decisions/ADR-0001-GPUI.md) before writing any manifest. The short version:

- Source: `zed-industries/zed`, pinned `rev = "395def7a067b32ed598fcb3f048969ada9b54942"`, crates `gpui` + `gpui_platform` (Windows pulls `gpui_windows`).
- Toolchain: stable `1.98.1`.
- Windows path: Win32 windowing, DirectWrite text, Direct3D 11 rendering; import libraries must exist in the installed Windows SDK.
- Upstream crates are declared Apache-2.0; **Desktop’s own license is unresolved** and distribution is gated on a license decision plus a third-party notice inventory.

Two rules that follow and are easy to get wrong:

1. Never introduce a floating GPUI reference (`branch`, `tag`, `"*"`). The lockfile is part of the pin.
2. Do not copy Zed’s repository configuration wholesale. Its `.cargo/config.toml` flags (`tokio_unstable`, `windows_slim_errors`, `+crt-static`) and its Windows guide target the Zed editor build, not this dependency set. Add configuration only when a concrete failure demands it, and record why.

## Phase 2 — Toolchain **[Observed implementation]**

`rust-toolchain.toml` pins stable `1.98.1`, minimal profile, rustfmt/clippy/rust-analyzer/rust-src, Windows MSVC target. The earlier toolchain line that separates components with spaces is not valid for this rustup CLI; use comma-separated components. The repository reports `rustc 1.98.1 (48a229cea 2026-09-01)`, Cargo `1.98.1 (797e8a9bc 2026-08-05)`.

Acceptance evidence: `rustc --version` reports `1.98.1` with the repository file. The exact user-level install command and output are recorded in the bootstrap report and ADR-0001. The SDK question was resolved by experiment: SDK `10.0.19041.0` built and rendered the dependency set, so no upgrade was installed. No administrator installation or global Git change was made for the toolchain itself; the initial task installed the user toolchain before requesting the normally required environment-change approval, disclosed in ADR-0001/BOOTSTRAP_REPORT.

Do not run another build without the workspace/lockfile present; do not let rustup silently install a channel mid-build.


## Phase 3 — Minimal native window **[Observed before shell expansion]**

Goal: the smallest correct code that proves the pinned GPUI stack builds and runs on this machine — nothing more.

Planned shape (per [REPOSITORY_ARCHITECTURE.md](../architecture/REPOSITORY_ARCHITECTURE.md); create only what is needed for the window, no empty crates):

```text
Cargo.toml                 # workspace + pinned git deps (ADR-0001)
rust-toolchain.toml        # channel 1.98.1 (ADR-0001 D2)
.cargo/                    # only if a concrete build failure requires local config
apps/desktop/              # binary: entry point, window creation
crates/desktop-gpui/       # create only when it owns actual rendering behavior
Cargo.lock                 # committed
```

The first build generated `Cargo.lock`; every later build/test command uses `--locked`. Binary package is `cos-desktop`; selectors use `cargo build --locked -p cos-desktop` and `cargo run --locked -p cos-desktop -- --fixture`. No repository `.cargo/config.toml` workaround was required for this dependency set.

Acceptance evidence: the pinned revision appears in `Cargo.lock`; `cargo build --locked` succeeded (initial full build ≈3m07s); the native window rendered bootstrap text and a close-button invocation exited 0. The first process-supervised termination exited 1 and is not normal-close evidence. No Runtime path, launch, or import. The minimal experiment's code has since been replaced by the fixture shell. D3D feature level/device-loss handling remain unmeasured.

If later dependencies reintroduce prerequisite failures, record the exact error and authorized resolution in [BUILD_AND_RUN.md](BUILD_AND_RUN.md) and the ADR before expanding further.

## Phase 4 — Workspace / protocol / shell **[Observed bootstrap]**

Workspace now contains `desktop-protocol` → `cos-client` → `app-core` → `ui-model` → `desktop-gpui` plus `apps/desktop`, in dependency order. The fixture-tested Draft is implemented as Rust wire types, the sole machine-readable source; no server guarantees are invented. Navigation/layout is headless-testable in app-core/ui-model before GPUI wiring. Runtime integration remains **[Deferred]** until the runtime side provides the guarantees in `docs/integration/COS_RUNTIME_REQUIRED_CHANGES.md`.

## Boundaries and non-goals (do not violate while “getting started”)

- Desktop never owns Task status, evaluation results, selection, graph revision, execution plan, artifact identity, or stop completion (canon; repository architecture). A technical window does not change that.
- Never depend on runtime internals, its database, its checkout path, or its Python entry points. The fixture client is the only runtime stand-in during bootstrap.
- Never create `cosd`, `artifact-service`, or provider adapters here; never create a monorepo with the runtime.
- Never copy upstream Zed configuration or code beyond the pinned dependency; if vendoring or patching ever becomes necessary, that is a new ADR.
- No empty crates, no placeholder modules, no “MVP” scaffolding: `desktop-gpui` exists when it renders, `cos-client` exists when it speaks the fixture contract.

## Related documents

- [BUILD_AND_RUN.md](BUILD_AND_RUN.md) — observed environment and planned commands.
- [TESTING.md](TESTING.md) — behavior tests and CI intent.
- [ADR-0001-GPUI.md](../decisions/ADR-0001-GPUI.md) — the pinned decision and its evidence log.
- [REPOSITORY_ARCHITECTURE.md](../architecture/REPOSITORY_ARCHITECTURE.md) and [DEPENDENCY_RULES.md](../architecture/DEPENDENCY_RULES.md) — ownership and allowed edges.
- [DESKTOP_PROTOCOL.md](../architecture/DESKTOP_PROTOCOL.md) — design draft; [PROTOCOL_OWNERSHIP.md](../integration/PROTOCOL_OWNERSHIP.md) defines the future Rust-type source and generated derivatives.
