# Native same-process Runtime reconnect (2026-11-10, MP-ACC1 round-2 repair)

Evidence-only run of the Desktop Runtime-reconnect repair. No generation was
performed and no task state is claimed beyond what the publication rendered.
One Desktop process served both phases; only the Runtime was restarted.

- Desktop binary: `target\release\cos-desktop.exe`, 21214720 B,
  sha256 `f05474d4ede9464cfe483b7b2274d2b35e786dea9adc62808114508654f19493`.
- Isolated Runtime on port 8134 (`C:/AI/orchestrator`), fresh
  read-only DB copy, per-run `ORCHESTRATOR_AUTH_SECRET` / media-handle secret and
  a redirected discovery descriptor. The Desktop ran with **no** endpoint or
  token environment: only the per-run issuance secret and the discovery path.
- Epochs: startup `118d6f08a5a3479ca22693a900a2e17f`, after the phase-1 restart
  `64b27c2c15c94165b19b32f823be1a8c`, after the phase-2 restart `5f2862c735d14ad997c5686d4a1a87f9`.
- Every restart reused the **same** discovery path and port; each new Runtime
  process advertised a new epoch in the descriptor.

## 1 — Automatic reconnect (no user action)

- The Runtime was stopped cleanly and stayed down for
  30.0 s — longer than the client's five bounded
  recovery attempts (~16 s), which is the exact round-2 loss condition.
- Withheld state observed in the UI: `True`
  (`情報源なし / データ非公開` + `再接続する`).
- Desktop attempts while the Runtime was down: 5
  (the Desktop keeps retrying instead of staying terminal).
- After the restart the **same** process recovered with no user action:
  `True` in 16.03 s.
- Serving requests for that recovery: `INFO:     127.0.0.1:63885 - "GET /api/cos/desktop/server HTTP/1.1" 200 OK | INFO:     127.0.0.1:63885 - "POST /api/cos/desktop/observation HTTP/1.1" 401 Unauthorized | INFO:     127.0.0.1:63886 - "POST /api/cos/desktop/credentials/refresh HTTP/1.1" 401 Unauthorized | INFO:     127.0.0.1:63887 - "POST /api/cos/desktop/credentials HTTP/1.1" 200 OK | INFO:     127.0.0.1:63885 - "POST /api/cos/desktop/observation HTTP/1.1" 200 OK | INFO:     127.0.0.1:63885 - "GET /api/cos/desktop/deltas?cursor=cursor-3da164045e294b9f8ea9d6ececa84bcc HTTP/1.1" 200 OK | INFO:     127.0.0.1:63885 - "GET /api/cos/desktop/deltas?cursor=cursor-3da164045e294b9f8ea9d6ececa84bcc HTTP/1.1" 200 OK | INFO:     127.0.0.1:63885 - "GET /api/cos/desktop/deltas?cursor=cursor-3da164045e294b9f8ea9d6ececa84bcc HTTP/1.1" 200 OK | INFO:     127.0.0.1:63885 - "GET /api/cos/desktop/deltas?cursor=cursor-3da164045e294b9f8ea9d6ececa84bcc HTTP/1.1" 200 OK | INFO:     127.0.0.1:63885 - "GET /api/cos/desktop/deltas?cursor=cursor-3da164045e294b9f8ea9d6ececa84bcc HTTP/1.1" 200 OK | INFO:     127.0.0.1:63885 - "GET /api/cos/desktop/deltas?cursor=cursor-3da164045e294b9f8ea9d6ececa84bcc HTTP/1.1" 200 OK | INFO:     127.0.0.1:63885 - "GET /api/cos/desktop/deltas?cursor=cursor-3da164045e294b9f8ea9d6ececa84bcc HTTP/1.1" 200 OK | INFO:     127.0.0.1:63885 - "GET /api/cos/desktop/deltas?cursor=cursor-3da164045e294b9f8ea9d6ececa84bcc HTTP/1.1" 200 OK | INFO:     127.0.0.1:63885 - "GET /api/cos/desktop/deltas?cursor=cursor-3da164045e294b9f8ea9d6ececa84bcc HTTP/1.1" 200 OK | INFO:     127.0.0.1:63885 - "GET /api/cos/desktop/deltas?cursor=cursor-3da164045e294b9f8ea9d6ececa84bcc HTTP/1.1" 200 OK`
  — the expected `observation 401` → `credentials/refresh 401` →
  `credentials 200` → fresh `observation 200` → `deltas 200` sequence.

## 2 — Explicit `再接続する` action

- Second outage: 45.0 s (the supervisor's pending
  delay grows to its 15 s cap).
- Attempts while down: 7; invocations
  of `再接続する`:
  `{'at': '2026-11-10T07:55:14+0900', 'invoked': True, 'found': True, 'matched_name': 'Desktop内 · 観測情報を利用できません — 再接続する', 'connected_before_click': False, 'attempts_between_restart_and_click': 0, 'restart_to_click_seconds': 0.699}`.
- Attempts between the Runtime restart and the click:
  `0`
  (a scheduled attempt in that window could not have been the recovery).
- Result: `True` in
  1.42 s after the click, with
  1 fresh attempt(s) recorded.

## Verdict

**PASS** — automatic `True`,
explicit `True`
(attributed to the action:
`True`, i.e. the UI was still
withheld at click time and no scheduled attempt intervened).

Raw (redacted) Desktop/Runtime logs, UIA dumps and screenshots are in `logs/`,
`uia/` and `shots/`; `manifest.json` pins the size/sha256 of every file and
`leak-scan.json` records the per-run secret scan. Cleanup: Desktop
`{'pid': 52896, 'wm_close_sent': True, 'exited': True, 'exit_code': 0, 'forced': None}`, Runtime `{'pid': 52908, 'clean': True, 'exit': 0}`.

Residuals: none recorded.
