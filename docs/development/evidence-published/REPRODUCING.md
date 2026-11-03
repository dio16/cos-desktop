# Reproducing the published evidence

This document covers the **21 recent sets** that were produced by native measurement:
**8 AX1 sets** and **13 REL3 sets**. Older sets keep their own notes and are not
described here.

## Verifying the stored bytes

Every set below stores a `record.json`. Two shapes exist: `files` values may be an object with a
`sha256` field, or a bare digest string, so the check must accept both, and sets without a
`record.json` are skipped rather than failed.

```python
import hashlib, json, pathlib
root = pathlib.Path('docs/development/evidence-published')
for setdir in sorted(p for p in root.iterdir() if p.is_dir()):
    record = setdir / 'record.json'
    if not record.is_file():
        continue
    files = json.loads(record.read_text(encoding='utf-8')).get('files') or {}
    for name, info in files.items():
        digest = info['sha256'] if isinstance(info, dict) else info
        actual = 'sha256:' + hashlib.sha256((setdir / name).read_bytes()).hexdigest()
        print(('OK   ' if actual == digest else 'MISMATCH ') + f'{setdir.name}/{name}')
```

Run from the repository root. Current state: **every stored digest matches** (`missing 0`, `mismatch 0`).

## Building the binary the native sets used

```bat
call "C:\Program Files (x86)\Microsoft Visual Studio\2019\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
cd /d C:\dev\cos-desktop
set CARGO_TARGET_DIR=C:\dev\cos-build-fresh
cargo build --locked --release -p cos-desktop --bin cos-desktop
```

Rebuilds into several other fresh target directories failed with `os error 5` while rustc wrote
dependency files; the directory above is the one that produced the measured binaries. The debug profile
built from the same source aborted with a stack overflow at startup, so the native sets use release.

## Launching and reading a set

```powershell
$exe = 'C:\dev\cos-build-fresh\release\cos-desktop.exe'
$local = Join-Path $env:TEMP ('cos-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path (Join-Path $local 'cos-desktop') -Force | Out-Null
$env:LOCALAPPDATA = $local          # fresh profile: instance guard and preferences start clean
$env:COS_PREVIEW_TRACE = '1'       # preview instrumentation; unset means silent
$env:COS_EXPORT_TRACE = '1'        # export instrumentation; unset means silent
$p = Start-Process -FilePath $exe `
    -ArgumentList @('--fixture','--fixture-file','C:\dev\cos-desktop\crates\cos-client\fixtures\task-184.json') `
    -PassThru -NoNewWindow -RedirectStandardError (Join-Path $local 'err.txt')
Start-Sleep -Seconds 8
Add-Type -AssemblyName UIAutomationClient
$root = [System.Windows.Automation.AutomationElement]::RootElement
$cond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ProcessIdProperty, $p.Id)
$window = $root.FindFirst([System.Windows.Automation.TreeScope]::Children, $cond)
# read an element by automation id
$find = { param($id) $c = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::AutomationIdProperty, $id)
    $window.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $c) }
# drive a control
(& $find 'diagnostics-toggle').GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke()
# finish: read stderr, then close
Get-Content (Join-Path $local 'err.txt')
$p.Kill(); Get-Process -Name cos-desktop -ErrorAction SilentlyContinue | Stop-Process -Force
```

Interaction rules that matter for reading these sets:

- **Keyboard is unavailable.** The window cannot be granted focus in this session (`SetForegroundWindow`
  returns false, Orca reports `window_not_focused`), so `Ctrl+D`, Tab and Shift+Tab never land. Anything
  requiring them is `NOT_RUN`.
- **Drive controls with `InvokePattern`**, not clicks. Elements whose handler only exists behind a synthetic
  click need an explicit accessibility listener; that is what the AX1 slice added.
- **`ValuePattern.SetValue`** on a styled div is not wired to application state (measured), so it is not a
  usable driving path.

## Per-set procedures

Each entry states what to do, what to expect, and when the set cannot be recreated here.

### `ax1-a11y-action-fix-20261104`

- **Do:** Open the panel, invoke `support-preview-button` once, read stderr and the tree.
- **Expect:** `action=reached`, `build=started`, `build=finished`, `install=accepted`; `support-preview-readonly` present.
- **Availability here:** Recreatable.

### `ax1-contract-matrix-latest`

- **Do:** No specific procedure recorded.
- **Expect:** —
- **Availability here:** Unknown.

### `ax1-diagnostics-toggle-16d6c9a`

- **Do:** Read `diagnostics-toggle`, invoke it, re-read the tree.
- **Expect:** Panel ids appear while open, disappear when closed; the toggle's Name switches between 開く and 閉じる.
- **Availability here:** Recreatable.

### `ax1-export-action-20261104`

- **Do:** Open the panel, invoke the preview button, wait for `support-export-button`, invoke it.
- **Expect:** Before the explicit listener: no dialog and no status element.
- **Availability here:** Partly recreatable; the dialog stage cannot complete here.

### `ax1-ime-deep-latest`

- **Do:** No specific procedure recorded.
- **Expect:** —
- **Availability here:** Unknown.

### `ax1-ime-latest`

- **Do:** No specific procedure recorded.
- **Expect:** —
- **Availability here:** Unknown.

### `ax1-remaining-listeners-20261104`

- **Do:** Invoke each of the eight migrated controls.
- **Expect:** Only `task-search` is on the initial surface and it exposes `ValuePattern`; the rest are `NOT EXERCISED`.
- **Availability here:** Not recreatable beyond `task-search` - the other surfaces need navigation.

### `ax1-toggle-lead-reproduction-20261104`

- **Do:** As above, also counting descendants around each invoke.
- **Expect:** 49 descendants closed against 56 open at that revision, with `InvokePattern` offered.
- **Availability here:** Recreatable.

### `rel3-diagnostics-latest`

- **Do:** No specific procedure recorded.
- **Expect:** —
- **Availability here:** Unknown.

### `rel3-dispatch-experiment-20261104`

- **Do:** Invoke the preview five times, once after reopening the panel; read rectangles.
- **Expect:** Both buttons measure 288x38 on screen; five invokes all return cleanly with no trace lines (pre-fix).
- **Availability here:** Recreatable only against that build.

### `rel3-existing-a9e4d15`

- **Do:** Save onto an existing file and record the OS overwrite confirmation.
- **Expect:** Existing file unchanged; status samples recorded (the log is cp932 and must be decoded as such).
- **Availability here:** Not recreatable now.

### `rel3-export-inprocess-20261104`

- **Do:** With `COS_EXPORT_TRACE=1`, open the panel, generate a preview, invoke export.
- **Expect:** Stage tokens stop at `dialog=opened`; the wait window was six seconds.
- **Availability here:** Recreatable up to the dialog stage.

### `rel3-inprocess-trace-20261104`

- **Do:** With `COS_PREVIEW_TRACE=1`, invoke the preview, plus the two controls.
- **Expect:** Variable reaches the child; the same exe writes to captured stderr for an unknown flag or missing fixture.
- **Availability here:** Recreatable.

### `rel3-native-fresh-release-20261104`

- **Do:** No specific procedure recorded.
- **Expect:** —
- **Availability here:** Unknown.

### `rel3-native-save-ce14dca`

- **Do:** Ctrl+D, generate the preview, save to a chosen path.
- **Expect:** 573 bytes, valid UTF-8 without BOM, byte-identical to the preview.
- **Availability here:** Not recreatable now: it needed Ctrl+D and a completed save dialog.

### `rel3-native-status-e5f3c27`

- **Do:** As above, reading `support-export-status` through automation after saving.
- **Expect:** Status Name carries the closed wording; `support-export-status` has an aria value.
- **Availability here:** Not recreatable now for the same reason.

### `rel3-positive-control-20261104`

- **Do:** `ValuePattern.SetValue` on `task-search`, then read the value back.
- **Expect:** Value reads back empty and the row list is unchanged; the element is a styled div, so this says little.
- **Availability here:** Recreatable.

### `rel3-preview-invoke-20261104`

- **Do:** Open the panel and invoke the preview button three times with waits.
- **Expect:** Nothing appears before the explicit listener exists.
- **Availability here:** Recreatable as a negative only on a pre-fix build.

### `rel3-preview-trace-20261104`

- **Do:** With `COS_PREVIEW_TRACE=1`, open the panel and invoke the preview button.
- **Expect:** `support-preview-trace` and stderr stay at `action=none` on a pre-fix build.
- **Availability here:** Recreatable only against that build.

### `rel3-r0b-connected-e5af45e`

- **Do:** No specific procedure recorded.
- **Expect:** —
- **Availability here:** Unknown.

### `rel3-trace-038005d`

- **Do:** Open the panel, generate a preview, save onto an existing file, read `support-export-trace`.
- **Expect:** `dialog=Selected;data=Ready;writer=AlreadyExists;ui=StatusUpdated`.
- **Availability here:** Not recreatable now.

## Limits that apply to all of these sets

- Keyboard input is unavailable, so anything needing it is `NOT_RUN`.
- In the export sets, the save dialog produced **no result within the six-second observation window**.
  That is the whole claim: not that it never returns, and not that it failed.
- Screen-reader reading quality has never been measured; it is `NOT_MEASURED` throughout.
