# COS Desktop v0.1.1 — unsigned portable release

Second published build: the **unsigned portable** Windows release with the media-refetch
UX fix and post-release verification. COS Runtime is **not** bundled.

## What is new since v0.1.0

- **Expired/Forbidden media refetch control.** The artifact viewer now offers
  `メディアを再取得 (R)` as an accessible button (UIA Invoke, `r` key) only in the explicit
  `Expired`/`Forbidden` states; it is hidden while an attempt is in flight, so repeated
  presses cannot cause a retry storm. The refetch advances a media-load nonce and
  re-resolves the same canonical identity through the live session identity
  (credential slot/principal/audience/epoch/read context); no bytes are fabricated and no
  stale post-expiry frame is reused (`98c45b9`).
- **Actual 300 s default-TTL expiry measured at protocol level**: advertised 299.976 s,
  immediate blob 200, the same handle after 305.0 s answers 410
  `MEDIA_HANDLE_EXPIRED`, and a fresh descriptor+blob returns 200 with a digest match
  (`b0bf2da`).
- **Post-release verification carried into the source**: a real NVDA 2026.3.1 per-user
  portable session measured 77 utterances across the main surface, Brief,
  inspector/evidence, History and command palette (commit `6904b69`; F1–F3 focus-pattern
  findings remain and this is not an AX certification), and the Runtime full suite is
  green at Runtime `dev` `06ee825e` (2,502 passed / 16 skipped / 1 deselected).

## Contents

- `cos-desktop.exe` — release build, `x86_64-pc-windows-msvc`.
- `LICENSE`, `NOTICE` — Apache-2.0 / Copyright 2026 dio.
- `THIRD_PARTY_NOTICES.md` — deterministic offline third-party notice bundle
  (`cos-desktop-third-party-notices-v1`, 377 dependencies; legal compliance not assessed).
- `README-RELEASE.md` — this file.

## Verification (release candidate, `d571ed2`)

- `cargo fmt --all -- --check`
- `cargo metadata --locked --no-deps`
- dependency rules self-test + live allowlist (7 members)
- plan ledger (0 findings / 62 cards)
- `cargo clippy --locked --workspace --all-targets -- -D warnings` (0 warnings)
- `cargo test --locked --workspace` (no failing suite)
- `cargo build --locked --workspace` and `cargo build --locked --release -p cos-desktop`

Release binary: **21,271,040 B**, sha256
`382e88629f05b3be233bdbc2f9e4afeb7a6931fec52f83af6379b4e305919e5f`.
This is the same binary that the native refetch/410 measurement used.

Runtime pairing: validated against COS Runtime `dev` tip `06ee825e` (separate repository).
No Runtime binary or data is included.

## Known limitations (accepted by the owner)

- **Unsigned portable build.** Signed installer, DD-21 certificate/publisher inputs and
  Windows install/uninstall/upgrade lifecycle are external prerequisites; no signature is
  claimed. The updater (MP-REL2) is deferred.
- Refetch control: native `Forbidden` behavior is implemented but not separately measured;
  the native 300 s UI wait was not performed (300 s expiry is protocol-measured only).
- Runtime-dependent acceptance: multi-recipe breadth (one-recipe catalog), Evidence groups
  not projected, PREVIEW/THUMBNAIL/METADATA byte routes, non-PNG artifacts and clock skew
  remain documented/accepted residuals; the same-process media resurface can require a few
  publication transitions (propagation, not retry).
- Accessibility: UIA keyboard/focus is measured; the real NVDA session covers bounded
  pronouncement, with F1–F3 focus findings open and no accessibility certification.
- Release-binary byte reproducibility is not claimed.

## Source and evidence

Source: `https://github.com/dio16/cos-desktop` (branch `main`, tag `v0.1.1`).
Evidence: `docs/development/evidence-published/media-refetch-latest/`,
`nvda-at-latest/`, `acc2-matrix-latest/`. Release record:
`docs/development/IMPLEMENTATION_MASTER_PLAN.md` §19.340.
