# MP-ACC1 Brief text-entry abort — reproduction, classification and fix (2026-11-09)

The ACC1 native harness ([`acc1-native-latest`](../acc1-native-latest/README.md)) recorded an
observation it could not explain: a pure UIA-focus-then-`WM_CHAR` entry into the Brief text
fields aborted the release binary with `thread caused non-unwinding panic. aborting.`, while a
click-first-then-`WM_CHAR` entry worked. The committed harness therefore clicks the field first.
This record reproduces that abort deterministically, identifies the code path, classifies it and
records the fix.

- **Classification: product bug — fixed** in `desktop-gpui`.
- **Affected user path:** a keyboard-only (or screen-reader/UIA) user who types in one Brief text
  field and then moves focus to the **other** Brief text field without a mouse click
  (`Tab` / `Shift+Tab` while the Brief is open, or UIA `SetFocus`) and types again. The click-first
  harness path never hits it because the product's click handler also resets the caret.
- **Severity:** the whole Desktop process aborts (exit `3221226505` / `0xC0000409`); an in-flight
  Task is not stopped or reconciled by the app.
- **Not affected:** the plain first-field entry (with or without a click), the accepted ACC1
  harness path, and the fixed binary (all three re-run below).

## Reproduction

The retained harness is
[`run_acc1_input_panic.py`](../../../../apps/desktop/devtools/runtime_integration/run_acc1_input_panic.py).
It reuses the ACC1 native harness pieces: an isolated Runtime on **8132** (fresh DB copy, copied
presets, per-run issuance/handle secrets, redirected RT8 discovery), the release Desktop launched
with discovery only (no `COS_DESKTOP_RUNTIME_ENDPOINT`, no token env), and
[`probe_window.ps1`](../../../../apps/desktop/devtools/runtime_integration/probe_window.ps1) for UIA
and `PostMessage` input. It never submits a Brief, never starts generation, and never foregrounds
the window.

```text
cd C:/dev/cos-desktop
# pre-fix release binary (rebuilt from the same product source as the ACC1 evidence run)
C:/AI/orchestrator/.venv/Scripts/python.exe -B ^
  apps/desktop/devtools/runtime_integration/run_acc1_input_panic.py ^
  --mode direction-then-goal --exe target/acc1-input-panic/cos-desktop-prefix.exe ^
  --evidence-dir docs/development/evidence-published/acc1-input-panic-latest/prefix
# fixed release binary
C:/AI/orchestrator/.venv/Scripts/python.exe -B ^
  apps/desktop/devtools/runtime_integration/run_acc1_input_panic.py ^
  --mode direction-then-goal ^
  --evidence-dir docs/development/evidence-published/acc1-input-panic-latest/fixed
```

Other modes: `--mode click-first` (the ACC1 harness path) and `--mode focus-only` (UIA `SetFocus`
on the empty Goal of a freshly opened Brief).

The pre-fix binary is not committed (binaries do not belong in evidence). Rebuild it from the same
product source by parking the fix:

```text
cd C:/dev/cos-desktop
git stash push -- crates/desktop-gpui
cargo build --locked --release -p cos-desktop
cp target/release/cos-desktop.exe target/acc1-input-panic/cos-desktop-prefix.exe
git stash pop
```

(Only the abort behavior is claimed; release builds are not byte-reproducible, so a rebuild has
its own hash — `5a8de3e4…7748f3` for the binary used here, versus the retained ACC1 artifact
`5acb4af8…`.)

### Smallest deterministic sequence (pre-fix)

1. Launch the release Desktop with discovery only; wait for the connected rail.
2. UIA `Invoke` on “新しいタスクを作成” (opens `クリエイティブ・ブリーフ`; the Goal is focused
   with an empty caret).
3. Synthetic click (`WM_LBUTTONDOWN/UP` inside the UIA bounds) on “方向性”, then post
   `WM_CHAR` for `abcdefghij` — the readback follows `a…abcdefghij`.
4. UIA `SetFocus` on “目的” (the empty Goal). **No mouse click.**
5. Post `WM_CHAR` `A`.

Step 5 aborts the process. Nothing after step 5 is observable; the UIA readback of the character
returns no value because the process is already gone.

## Observed signals

| Run | Binary (sha256) | Result | Exit code | stderr |
|---|---|---|---|---|
| `prefix/` | `5a8de3e4…7748f3`, 21,109,760 B | **PANIC** | `3221226505` (`0xC0000409`) | `thread caused non-unwinding panic. aborting.` |
| `fixed/` | `1a99d737…6234`, 21,110,272 B | NO_PANIC | `0` after clean `WM_CLOSE` | — |
| `fixed-click-first/` | `1a99d737…6234` | NO_PANIC | `0` after clean `WM_CLOSE` | — |
| `fixed-focus-only/` | `1a99d737…6234` | NO_PANIC | `0` after clean `WM_CLOSE` | — |

`prefix/desktop.log` (verbatim):

```text
--- launch 1 2026-11-09T12:57:10+0900 ---
Runtime discovered: server_identity=cos-runtime-orchestrator epoch=… pid=…
Runtime credential reauthorized after rejection; the session reopens a fresh observation
thread caused non-unwinding panic. aborting.
```

Two crash-report slots were written by the product hook 4 ms apart
(`prefix/crash-reports/report-0000000001.txt`, `report-0000000002.txt`; identical bytes, md5
`3c2805fb…`); both say `Desktop version: 0.1.0`, `Crash category: unknown`,
`Retained events: unknown`. **No Rust panic message, location or backtrace exists anywhere**:
`app_core::panic_report::install` replaces the default panic hook and stores only the closed
category fields, and the panic itself happens inside the window procedure, where Rust cannot
unwind and the standard library prints only `thread caused non-unwinding panic. aborting.` before
`abort()`.

The fixed run kept both fields independent (`fixed/uia/after-typing.json`,
`fixed/shots/after-typing.png`): Goal = `ACC1PANIC`, Creative Direction = `abcdefghij`; the Goal
caret started at the end of the *empty* Goal, not at the Direction caret.

## Focus state: UIA focus vs GPUI focus vs click

- UIA `SetFocus` on the Edit **does** move GPUI focus: in the reproduced sequence the characters
  after step 4 land in the Goal, not in the previously clicked Direction field.
- UIA `SetFocus` does **not** run the field's click handler, so it does **not** reset the
  product's shared caret (see the root cause).
- UIA's own `FocusedElement` report did not follow the element (it stayed `COS Desktop` in every
  probe). UIA self-report is therefore not evidence of focus here; the text readback is.
- A mouse click does both: GPUI focus *and* `brief_goal_input`/`brief_direction_input` caret
  reset (product `on_click`). That is exactly why the click-first harness never aborted and why
  the workaround is not a fix.

## Root cause

`crates/desktop-gpui/src/lib.rs` (pre-fix) kept **one** caret/marked-range pair for both Brief
text fields (`brief_input_selection`, `brief_marked_range`) while routing composition by the
*focused* field (`brief_composition_field`). The click handlers in
`crates/desktop-gpui/src/brief_surface.rs` reset that shared caret to the clicked field's text
length, but the keyboard/UIA focus transitions (`brief_cycle_focus` for `Tab`/`Shift+Tab`, and
GPUI's UIA `Action::Focus`) only move focus.

So after typing ten ASCII characters into the Direction, the shared caret is `10..10`. Focusing
the empty Goal and pressing a key runs:

```rust
// pre-fix replace_text_in_range: range_utf16 is None from WM_CHAR
let range = ime_byte_range(&current, None, self.brief_marked_range.clone(), &self.brief_input_selection);
let (replacement, cursor) = ime_splice(&current, &range, new_text);
```

`ime_byte_range` fell back to the stale `10..10`, and `ime_splice` sliced `&current[..10]` on a
zero-length Goal text. The slice panicked inside the `WM_CHAR` handler, i.e. inside the
`extern "system"` window procedure: Rust marks that frame `nounwind`, so the panic could not
unwind and the process aborted — the exact stderr line above. The same invariant break is possible
whenever a form edit is refused (the Brief is `Submitting`/`Sent`/`Accepted`): the text is
unchanged but the old code still advanced the shared caret by the refused character.

## Fix

1. **One caret/marked range per Brief field** (`BriefFieldInput`: `brief_goal_input`,
   `brief_direction_input`). `selected_text_range`, `marked_text_range`, `unmark_text`,
   `replace_text_in_range` and `replace_and_mark_text_in_range` now use the state of the field
   that is focused; `brief_focus_field` (used by both click handlers and the open surface) sets a
   field's caret at the end of *its own* text, and `brief_reset_field_inputs` resets both on open.
2. **A refused edit no longer moves the caret**: the caret/marked range is updated only when
   `set_goal`/`set_direction` accepted the replacement.
3. **`ime_byte_range` clamps before slicing** (`clamp_byte_range` + `floor_char_boundary`):
   both ends are snapped to a character boundary and bounded by the current text, and
   `start <= end` is restored. Any future stale/foreign range therefore degrades to an in-text
   insertion instead of an abort.

The fix is deliberately not "click the field": the click reset stays as a caret convenience, not
as the correctness guard.

## Tests and gates

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo metadata --locked --no-deps --format-version 1` | clean |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | clean |
| `cargo test --locked --workspace` | pass — every suite, including `desktop-gpui` (268 passed, 5 ignored); see the environment caveat below |
| `cargo build --locked --workspace` | clean |
| `python -B scripts/check_dependency_rules.py --self-test` | 19 tests OK |
| `python -B scripts/check_dependency_rules.py` | allowlist edges verified |
| `cargo test -p desktop-gpui --locked ime_tests` | 12 passed (7 pre-existing + 5 new) |

Environment caveat (not a regression): the first workspace test run failed only in `app-core`
`preferences`/`support_report` tests that create directories under the user `%TEMP%`, with
`OS error 5 (PermissionDenied)`. Re-running with `TEMP`/`TMP` redirected into `target/tmp` passes
every suite. No `app-core` code is touched by this change; the restriction belongs to this
execution environment.

New regression tests in `crates/desktop-gpui/src/lib.rs` (`mod ime_tests`):

- `a_caret_left_by_the_other_brief_field_is_clamped_not_sliced` — the exact abort input
  (`ime_byte_range("", None, None, &(10..10))` → `0..0`, splice yields `"A"`).
- `a_marked_range_outside_the_text_is_clamped`
- `a_caret_past_the_text_stays_inside_after_a_refused_edit`
- `a_reversed_composition_range_is_normalized`
- `the_brief_fields_keep_separate_carets` (source wiring: per-field state, clamp in
  `ime_byte_range`, click handlers use `brief_focus_field`)

## Residual / not measured

- **No Rust panic text, location or backtrace is recoverable** from the release product: the
  product panic hook intentionally drops them. The classification rests on the deterministic
  reproduction, the abort site (window procedure) and the code path, not on a captured panic
  location. A temporary diagnostic build was not required to fix it and is not retained.
- **Real IME composition was not driven.** The harness posts `WM_CHAR` (and never
  `WM_IME_COMPOSITION`); composition enters the same handlers through
  `replace_and_mark_text_in_range`, and the clamp covers those ranges, but Japanese IME
  composition through a native IME is `NOT_MEASURED` here.
- **`Tab` itself was not posted.** GPUI consumes `WM_GPUI_KEYDOWN`, not a posted `WM_KEYDOWN`,
  so the keyboard focus transition is exercised through UIA `SetFocus`, which reaches the same
  `focus()` transition as `brief_cycle_focus` (`tab` → `FocusNext` → `brief_cycle_focus`);
  the keystroke path is `NOT_MEASURED`.
- **Typing after a submit was not exercised** (that would submit a real Task). The refused-edit
  invariant is guarded and unit-tested (`a_caret_past_the_text_stays_inside_after_a_refused_edit`),
  but the end-to-end post-submit typing sequence is `NOT_MEASURED`.
- The pre-fix binary is a **rebuild** of the same product source, not the retained ACC1 artifact
  (`sha256:5acb4af8…` in `acc1-native-latest`); the rebuild hash is recorded above. Release builds
  are not byte-reproducible across rebuilds, and only the behavior (the abort) is claimed.
- Screen-reader text reading, caret rendering/marked-range painting and High Contrast were not
  re-measured; this record changes only the input-state handling.

## Cleanup

Every run stops the Desktop (`WM_CLOSE`; the panic run exits by itself), stops the isolated
Runtime (exit 0), and verifies port **8132** has no listener and neither process survives; the
per-run record is in `run.json` (`cleanup`). No generation plane was started or used, and no Task
was created.

## Files

| Path | Content |
|---|---|
| `prefix/run.json`, `prefix/desktop.log`, `prefix/crash-reports/` | pre-fix run: abort, exit code, stderr, crash reports |
| `prefix/shots/`, `prefix/uia/` | pre-fix window states (`connected`, `brief-open`, `direction-filled`) and UIA dumps |
| `fixed/run.json`, `fixed/shots/`, `fixed/uia/` | fixed run on the same sequence: both fields independent, no panic |
| `fixed-click-first/`, `fixed-focus-only/` | fixed controls for the ACC1 harness path and the UIA-focus-only path |
| `postfix/`, `postfix-click-first/` | verifier-takeover re-measurement (13:08–13:12): the abort path and the ACC1-harness control, both NO_PANIC |

## Verifier takeover addendum (2026-11-09)

- **Attribution:** `postfix/` and `postfix-click-first/` were produced by the **verifier
  takeover** (13:08–13:12), not the ACC1-native lane, as an independent re-measurement while the
  lane was finalizing. `prefix/` is the lane's pre-fix baseline; `fixed/`, `fixed-click-first/`
  and `fixed-focus-only/` are the lane's own post-fix runs (13:05–13:08).
- **Results:** `prefix/` **PANIC** (exit `3221226505` / `0xC0000409`,
  `thread caused non-unwinding panic. aborting.`); verifier `postfix/` (direction-then-goal, the
  abort path) **NO_PANIC** with a clean `WM_CLOSE` exit **0**, Runtime exit 0 and port 8132 free;
  verifier `postfix-click-first/` (ACC1-harness control) **NO_PANIC**, the same clean close.
- **Binary provenance:** the verifier repro used a **copy of the post-fix release binary**
  sha256 `1a99d737…6234` (21,110,272 B, built 12:55:36 from the fixed sources), because
  `cargo build --release` could not replace `target/release/cos-desktop.exe`
  (`failed to remove … Access denied (os error 5)`; even `rm` on that exact path is denied by an
  ACL/delete-lock). The verifier's later source edits were formatting only plus one narrowly
  justified `#[allow(clippy::reversed_empty_ranges)]` on the intentionally reversed-range test, so
  they do not change the binary's semantics. The debug build, `cargo check --workspace
  --all-targets` and the `desktop-gpui` unit tests compile the committed fixed sources and pass.
- **Verifier gates (post-repair, at this commit):** `cargo fmt --all -- --check` 0;
  `cargo clippy -p desktop-gpui --all-targets -- -D warnings` 0 warnings / 0 errors;
  `cargo test -p desktop-gpui --lib` 268 passed / 0 failed / 5 ignored (the five new IME tests
  included); `cargo check --workspace --all-targets` 0; `cargo build --workspace` 0.
- **Environment residual:** the release artifact at `target/release/cos-desktop.exe` cannot be
  replaced in this workspace while its ACL/delete-lock persists. A final release gate must build
  into a fresh target directory (or have the lock removed); debug builds and tests are unaffected.
