# F1-path negative control — probe `foreground` (`SetFocus`) without an activation cycle

This pass used the same listener, the same release binary and the same isolated Runtime lane as
the parent record, but foregrounded the window with the probe's `foreground` action
(`FocusWindow`: `AttachThreadInput` + `BringWindowToTop` + `SetForegroundWindow` + `SetFocus` on
the top-level HWND) and ran no deactivate/reactivate cycle.

Result over 183 s: **0 focus events**, `AutomationElement.FocusedElement` = the top-level
`COS Desktop` window in every read, while the app's UIA tree and its keyboard shortcuts still
worked (the palette opened and closed). That is exactly the symptom recorded as F1 in
`native-ax-ime-dpi-latest`, and it shows the earlier negative was a property of the measuring
path, not of the provider's event implementation.

Files:

- `session.json` — the full record of that pass (probe dumps are summarised inside it; the raw
  `uia/*.json` dumps of this control pass were not retained to keep the evidence bounded).
- `desktop-focus-events.jsonl`, `-summary.json`, `-status.json` — the raw listener output
  (start + 2 polls, 0 events).
