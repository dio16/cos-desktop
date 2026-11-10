# COS Desktop v0.1.0 — initial portable release

This is the first published **unsigned portable** build of COS Desktop. It is a native
Windows GPUI application that connects to a separately maintained COS Runtime over the
versioned Desktop Protocol. The Runtime is **not** bundled.

## Contents

- `cos-desktop.exe` — release build, `x86_64-pc-windows-msvc`.
- `LICENSE`, `NOTICE` — Apache-2.0 / Copyright 2026 dio.
- `THIRD_PARTY_NOTICES.md` — deterministic offline third-party notice bundle
  (`cos-desktop-third-party-notices-v1`, 377 dependencies; legal compliance not assessed).
- `README-RELEASE.md` — this file.

## Verification (release candidate)

The release candidate was gated on Windows with:

- `cargo fmt --all -- --check`
- `cargo metadata --locked --no-deps`
- dependency rules self-test + live allowlist (7 members)
- plan ledger (0 findings / 62 cards)
- `cargo clippy --locked --workspace --all-targets -- -D warnings` (0 warnings)
- `cargo test --locked --workspace` (no failing suite)
- `cargo build --locked --workspace` and `cargo build --locked --release -p cos-desktop`

Release binary: **21,258,240 B**, sha256
`263134b5e8af53451366c81bb2ffe603c5b4c24d9e0c03248c4858ed4cdf6fce`.

Runtime pairing: the release was validated against the COS Runtime `dev` tip
`7764b7c4` (separate repository). No Runtime binary or data is included.

## Known limitations (accepted by the owner)

- **Unsigned portable build.** The signed installer and DD-21 certificate/publisher
  inputs are external prerequisites and were not available; no signature is claimed.
  Do not treat this archive as a signed distribution.
- Windows installer/uninstall/upgrade and file associations are not included; the
  updater (MP-REL2) is deferred for the initial release.
- Runtime-dependent acceptance: multi-recipe breadth (the catalog exposes one recipe),
  Evidence groups not projected, some media variants (PREVIEW/THUMBNAIL/METADATA,
  non-PNG, clock skew), a native wire-410 Expired path and an actual 300 s TTL expiry
  remain measured or accepted as documented in the repository plan/evidence.
- Accessibility: UIA focus/keyboard behavior is measured; a real screen-reader (NVDA/AT)
  pronouncement and mixed-DPI/High-Contrast/Japanese-IME composition remain limited.
- The Runtime full test suite retains pre-existing/environment-specific failures
  documented in the Runtime repository; the Desktop gates above are green.

## Running

Run `cos-desktop.exe` from any directory. With no Runtime endpoint configured it starts
honestly disconnected; a running Runtime can be discovered through its published
discovery descriptor, or an endpoint can be supplied through the documented environment
variables. Do not place the binary where it would imply Runtime portability.

## Source and evidence

Source: `https://github.com/dio16/cos-desktop` (branch `main`, tag `v0.1.0`).
Acceptance matrix and residual list: `docs/development/evidence-published/acc2-matrix-latest/`.
Release plan record: `docs/development/IMPLEMENTATION_MASTER_PLAN.md` §19.338.
