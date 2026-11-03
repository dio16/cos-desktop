# Reproducing the published evidence

This file gives the commands and conditions needed to re-create each recent evidence set.
Every set stores a `record.json` with the SHA-256 of each file; verify with:

```
python - <<'PY'
import hashlib, json, pathlib
base = pathlib.Path('docs/development/evidence-published/<set>')
rec = json.loads((base / 'record.json').read_text(encoding='utf-8'))
for name, info in rec['files'].items():
    actual = 'sha256:' + hashlib.sha256((base / name).read_bytes()).hexdigest()
    print(('OK  ' if actual == info['sha256'] else 'BAD ') + name)
PY
```

## Build used by the native sets

The window-based sets use a release build in an isolated target directory. Rebuilds into some other
fresh directories failed with `os error 5` while rustc wrote dependency files, so use this one:

```bat
call "C:\Program Files (x86)\Microsoft Visual Studio\2019\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
cd /d C:\dev\cos-desktop
set CARGO_TARGET_DIR=C:\dev\cos-build-fresh
cargo build --locked --release -p cos-desktop --bin cos-desktop
```

## Launch conditions shared by the window sets

- `LOCALAPPDATA` is a **fresh temporary directory** containing `cos-desktop/`, so the instance guard and
  preferences start clean.
- Fixture mode: `--fixture --fixture-file crates/cos-client/fixtures/task-184.json`.
- Instrumentation switches: `COS_PREVIEW_TRACE=1` and/or `COS_EXPORT_TRACE=1`; unset means silent.
- Accessibility driving goes through UI Automation (`InvokePattern`, `ValuePattern`); keyboard input is
  **not** available in this session because the window cannot be made foreground, so any set that needs
  keystrokes records `NOT_RUN` rather than pretending otherwise.

## Per-set procedures

### `ax1-a11y-action-fix-20261104`

Open the panel, invoke `support-preview-button` once, and read stderr: with `COS_PREVIEW_TRACE=1` the
sequence `action=reached`, `build=started`, `build=finished`, `install=accepted` appears, and
`support-preview-readonly` becomes present.

Verification: run the digest snippet above against this directory.

### `ax1-export-action-20261104`

Open the panel, invoke the preview button, wait for `support-export-button`, then invoke it. Before the
explicit listener this produced no dialog and no status; the set records that negative result and the
state it did and did not capture.

Verification: run the digest snippet above against this directory.

### `ax1-remaining-listeners-20261104`

Attempt to invoke each migrated control. Only `task-search` exists on the initial surface and it exposes
`ValuePattern`, not `InvokePattern`, so the set records `NOT EXERCISED` rather than a pass.

Verification: run the digest snippet above against this directory.

### `ax1-toggle-lead-reproduction-20261104`

Same procedure as above, but capturing the descendant count around each invoke (49 closed, 56 open at
that revision) and recording the button's `InvokePattern` presence.

Verification: run the digest snippet above against this directory.

### `rel3-diagnostics-latest`

No special procedure recorded; the set is a snapshot of the conditions stated above.

### `rel3-dispatch-experiment-20261104`

Invoke the preview button five times, once after closing and reopening the panel, and read the rectangles
of both buttons while the panel is open.

Verification: run the digest snippet above against this directory.

### `rel3-existing-a9e4d15`

Save onto an existing file and record the OS overwrite confirmation. The status samples in that set are
cp932-encoded; decode as cp932, not UTF-8.

Verification: run the digest snippet above against this directory.

### `rel3-export-inprocess-20261104`

With `COS_EXPORT_TRACE=1`, open the panel, generate a preview, invoke the export button and read stderr; the set records exactly which stage tokens appear and which never do.

Verification: run the digest snippet above against this directory.

### `rel3-inprocess-trace-20261104`

With `COS_PREVIEW_TRACE=1`, invoke the preview button and compare stderr against the two controls: the
variable reaches the child (`cmd.exe /c echo %COS_PREVIEW_TRACE%`) and the same exe writes to the captured
stderr for an unknown flag or a missing fixture.

Verification: run the digest snippet above against this directory.

### `rel3-native-fresh-release-20261104`

No special procedure recorded; the set is a snapshot of the conditions stated above.

Verification: run the digest snippet above against this directory.

### `rel3-native-save-ce14dca`

Ctrl+D opens the panel; the preview button produces a report; Save opens the native dialog and writes
573 bytes. Note: that set needed keyboard input, which this session cannot supply, so it cannot be
recreated here even though its bytes are intact.

Verification: run the digest snippet above against this directory.

### `rel3-native-status-e5f3c27`

Same as above, reading the status line through UI Automation after a save. The status element now has an
automation id and an `aria_value` carrying the closed wording.

Verification: run the digest snippet above against this directory.

### `rel3-positive-control-20261104`

Drive `task-search` with `ValuePattern.SetValue` and read the value back. Note in the record that this
element is a styled div, so the result says little about the accessibility path in general.

Verification: run the digest snippet above against this directory.

### `rel3-preview-invoke-20261104`

Open the panel and invoke the preview button three times with waits; the set records that nothing appeared
before the explicit listener existed.

Verification: run the digest snippet above against this directory.

### `rel3-preview-trace-20261104`

With `COS_PREVIEW_TRACE=1`, open the panel and invoke the preview button, reading
`support-preview-trace` and stderr.

Verification: run the digest snippet above against this directory.

### `rel3-r0b-connected-e5af45e`

No special procedure recorded; the set is a snapshot of the conditions stated above.

Verification: run the digest snippet above against this directory.

### `rel3-trace-038005d`

Open the panel, generate a preview, save onto an existing file and read `support-export-trace`.

Verification: run the digest snippet above against this directory.

## Known limits that apply to all of these sets

- Keyboard input (`Ctrl+D`, Tab, Shift+Tab) is unavailable because the window cannot be granted focus;
  anything needing it is `NOT_RUN`, not passed.
- The native save dialog does not return a result in this session; the export sets record how far the
  flow got rather than claiming a completed save.
- Screen-reader reading quality has never been measured; it is `NOT_MEASURED` throughout.
