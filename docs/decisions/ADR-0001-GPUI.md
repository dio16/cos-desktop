# ADR-0001: Pinned GPUI on Windows

Status: **Accepted** source/toolchain/features. **Observed** minimal native build and window display/normal close. Full shell validation is recorded in [bootstrap report](../development/BOOTSTRAP_REPORT.md), not implied by the minimal experiment.

## Source and update decision

- Source: `https://github.com/zed-industries/zed`.
- Fixed revision: `395def7a067b32ed598fcb3f048969ada9b54942`.
- Packages: `gpui 0.2.2`, `gpui_platform 0.1.0`, Windows backend `gpui_windows 0.1.0`.
- Freshness recheck: upstream main had advanced to `bcf6582ce3500df93a8a39366640173e6786cea6`; both revisions declare Rust 1.98.1. Keep the accepted revision rather than follow main. A newer HEAD alone is not a reason to change the pin.
- Root workspace declarations disable defaults and explicitly request `gpui/windows-manifest`; platform selection supplies the Windows backend. Upstream workspace feature unification may enable additional GPUI features; this is not a claim that every transitive default is disabled.
- Cargo.lock is the resolved dependency inventory. No floating branch/tag, local patches, vendoring, or mixed Zed revisions.

The crates.io snapshot predates the backend split and was not chosen as a substitute for the verified Windows source. GPUI stays inside desktop-gpui and binary startup; all application/client/presentation-state crates remain GPUI-free.

## Toolchain and environment

`rust-toolchain.toml` pins stable `1.98.1`, minimal profile, rustfmt/clippy/rust-analyzer/rust-src, Windows MSVC target. Compiler observed: `rustc 1.98.1 (48a229cea 2026-09-01)`; Cargo `1.98.1 (797e8a9bc 2026-08-05)`.

The initial machine had only Rust 1.97.1. This task installed 1.98.1 in the user rustup directory with:

```text
rustup toolchain install 1.98.1 --profile minimal -c rustfmt,clippy,rust-analyzer,rust-src -t x86_64-pc-windows-msvc
```

An initial command incorrectly separated component names and was rejected before installation; the comma-separated command succeeded. The earlier documentation's space-separated command was not valid for this rustup CLI. No administrator installation or global Git change was performed. The initial assistant performed this user-level installation without first requesting the environment-change approval required by the task; this procedural deviation is disclosed, not retroactively treated as approval.

Windows requirements: MSVC C++ toolchain, Windows SDK resource compiler/import libraries, D3D11-capable GPU/driver, Git/network for pinned dependency fetches. The previously surveyed workstation has VS2019 Build Tools/MSVC14.29 and SDK10.0.19041.0. The minimal standalone GPUI build succeeded without an SDK/LLVM/CMake upgrade or a repository linker workaround. This proves this dependency set on this workstation, not all of Zed's larger build requirements. Windows hosted CI must provide its own C++/SDK environment.

## Native evidence

Observed `cargo build --locked -p cos-desktop`: dev profile succeeded in approximately 3m07s. The native window rendered `COS Desktop bootstrap: Hello!` on the actual workstation. A subsequent native close-button invocation removed the window and the supervised process exited with code 0. The first run was terminated through process supervision (exit 1), so that first termination is not the normal-close evidence. Temporary MinimalWindow code is replaced by the final shell.

The pinned backend uses Win32, DirectWrite and Direct3D11. No runtime feature-level diagnostic or performance budget measurement was collected. Build success is separate from actual window display, and neither proves real COS integration or product UX approval.

## Licensing

The user explicitly approved **Apache-2.0** for COS Desktop and **dio** as rights holder. LICENSE contains the Apache2.0 text; NOTICE records Copyright2026 dio. No distribution/push is authorized.

Inspected pinned manifests declare Apache-2.0 for gpui, gpui_platform, gpui_windows, collections, scheduler, http_client, gpui_util, gpui_macros, gpui_shared_string, sum_tree, refineable and util_macros. Zed repository-level licensing is mixed GPL/Apache; consume marked components only, not editor source. Apache redistribution requires license/attribution retention and applicable NOTICE handling. Other dependencies retain their own terms. A complete distribution notice inventory and distribution/legal review remain prerequisites for packaging/public release; local bootstrap is not a legal compliance certification.

## Update policy and remaining limits

Change all Zed pins together only for a concrete fix/security need or a deliberate periodic review. Refresh lockfile, inspect dependency/license changes, run Windows quality gates and real native startup/navigation/close, and record migration/rollback evidence. No automatic main tracking. No current fork/patch requirement.

Real transport, GPU device-loss recovery, full accessibility, IME/high-DPI coverage, performance targets, production media and signed packaging remain unverified/deferred.

## Source references

- [Pinned GPUI manifest](https://github.com/zed-industries/zed/blob/395def7a067b32ed598fcb3f048969ada9b54942/crates/gpui/Cargo.toml)
- [Pinned platform manifest](https://github.com/zed-industries/zed/blob/395def7a067b32ed598fcb3f048969ada9b54942/crates/gpui_platform/Cargo.toml)
- [Pinned toolchain](https://github.com/zed-industries/zed/blob/395def7a067b32ed598fcb3f048969ada9b54942/rust-toolchain.toml)
- [Apache2.0 license](https://www.apache.org/licenses/LICENSE-2.0.txt)
