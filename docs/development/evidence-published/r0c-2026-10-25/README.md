# Published MP-R0C native-stress records — sanitised, bounded

Six scenarios (keyboard, held-key, pointer, scroll, Ctrl+Tab surface navigation, combined)
were driven with **synthetic** PostMessage input through the approved S2B capture path on
the approved instrumented candidate. Each scenario is one 30 s capture scope and one
observation; these are not distributions.

## Provenance

- Source commit `bcce8167591f6f142232b3c5ca7c5c14c7da5cdf`; instrumented executable
  `sha256:850b73a029b80fa8ad49d3cc5ac38a5683df8ae962d0a66f3f4fb282c0141ccb` (approved),
  helper `sha256:4d64f7c3ab8074af0d507c51b70e1a2757de47763c0bf088390df9e4d803b3ec`,
  fixture `sha256:2e33ed061c07bd97b8e4c4f823c028987e9415e83858878270489fb5b8c18c52`,
  GPUI pin `395def7a067b32ed598fcb3f048969ada9b54942`, feature `r0-s2b-instrumentation`,
  Windows 11 build 26300, Ryzen 7 3700X, RTX 2080 SUPER.
- Each run went through `s2b_capture.py` (approved S2A launcher + passive S2B capture +
  replay); the per-scenario execution identity is in the table.
- Input is synthetic: WM_KEYDOWN/WM_KEYUP, WM_MOUSEMOVE, WM_LBUTTONDOWN/UP and
  WM_MOUSEWHEEL posted to the app window, not a real user session. The window was closed by
  an operator WM_CLOSE at 40 s.

## Scenarios (requested vs achieved)

| scenario | requested | achieved | messages | retained/eligible | dropped/unassociated | retained kinds | draw gaps | coalesced records | settle after last input | execution |
|---|---|---:|---|---|---|---|---|---|---|
| `keyboard` | 20 Hz | key 20.039 Hz | key 500 | 256/2420 | 2164/968 | input 102, dirty 52, draw 51, submit 51, surface-mutation 0, notify 0 | max 27.022 ms / 51 intervals | 0/102 (max 1) | n/a (inputs incomplete) | `a4fc67ef08f947f39e28ef2086fd0675` |
| `held-key` | 30 Hz | held-keydown 30.043 Hz | held-keydown 750 | 256/2904 | 2648/728 | input 64, dirty 64, draw 64, submit 64, surface-mutation 0, notify 0 | max 14.519 ms / 64 intervals | 0/128 (max 1) | n/a (inputs incomplete) | `efa2d133b04a469a8a453feb8a673a97` |
| `pointer` | 30 Hz | move 30.037 Hz, click 1.541 Hz | move 750, click 38 | 256/3023 | 2767/838 | input 71, dirty 62, draw 62, submit 61, surface-mutation 0, notify 0 | max 18.824 ms / 62 intervals | 123/123 (max 6) | n/a (inputs incomplete) | `17031106ba3d420598c0e1b574ab1239` |
| `scroll` | 10 Hz | wheel 10.039 Hz | wheel 250 | 256/482 | 226/241 | input 128, dirty 0, draw 0, submit 0, surface-mutation 0, notify 0 | no draw intervals | 0/0 (max 0) | n/a (inputs incomplete) | `e0499a69a36c4a05bd46e9b008818ec3` |
| `navigation` | 2 Hz | ctrl-tab 2.041 Hz | ctrl-tab 50 | 240/240 | 0/96 | input 96, dirty 48, draw 48, submit 48, surface-mutation 0, notify 0 | max 474.867 ms / 48 intervals | 0/96 (max 1) | n/a (inputs incomplete) | `c9aed2ff23ea4dbf869f87a6729117d2` |
| `combined` | 55 Hz | key 20.04 Hz, move 30.041 Hz, wheel 5.041 Hz | key 500, move 750, wheel 125 | 256/2962 | 2706/1898 | input 167, dirty 3, draw 85, submit 0, surface-mutation 0, notify 0 | max 2.208 ms / 85 intervals | 42/85 (max 5) | n/a (inputs incomplete) | `e3a26ee9b0c74a31af219c8584cf77a5` |

Scenario notes: keyboard/held-key/pointer/combined hit the 256-record retention cap
(thousands of entries dropped), so retained counts are not event totals; `input-unassociated`
is large because the pin's InputTiming carries no window ID. `scroll` produced no
dirty/draw/submission records (the wheel did not invalidate the fixture surface) but 128
internal accounting records. `navigation` (Ctrl+Tab) produced input/draw/submission records
but no `local-surface-mutation`/`local-notify` record, so surface navigation could not be
confirmed with this fixture; its ~475 ms draw gaps match the 500 ms inter-action spacing and
are not stall evidence. `combined` retained no submission record under the cap.

## Queue, cancellation and restoration

- **Backlog evidence:** the journal records 256 retained of up to 428 eligible entries under
  its fixed retention cap, with `retention_dropped` and `excluded` counters in each capture.
  Queue depth is **NOT_MEASURED**: the build has no queue-depth instrumentation.
- **Supersession/coalescing:** the "coalesced records" column counts retained draw/submission
  records that carry more than one invalidation, i.e. dirty notifications that were superseded
  by a newer change before being drawn. This is evidence of coalescing, not of a queue bound.
- **Cancellation:** no cancellable in-flight work can be produced by this fixture workload and
  the build exposes no cancellation counter; cancellation is **NOT_MEASURED**.
- **Settle:** the settle column is the interval between the last retained input record and the
  last retained dirty/draw/submission record. UI state restoration after input stops is
  **NOT_MEASURED** (no AX/visual probe in this path); only journal quiescence is observed.

## Stall attribution

The only attributable evidence is internal journal timing: gaps between consecutive retained
draw intervals (table) and submission intervals (per-result `draw-gap` metric). Because no
present/displayed/scanout endpoint exists on this host, the §11.1 visible-frame stall gate is
**`UNVERIFIED`** for every scenario: a journal gap is not a visible-frame stall, and this
record does not attribute any gap to message servicing, frame processing, submission, Present
or composition. No cause is inferred. The stall-gate metric in each result carries the closed
reason.

## NOT_MEASURED / UNVERIFIED

- `NOT_MEASURED`: present-call, presented-frame, input-to-photon, scanout, gpu-submission
  (no probe on this host); frame-requested/frame-processing-started (app-side only at the pin);
  paint sub-split, VRAM high-water, process CPU/working-set, queue depth, cancellation,
  restoration, and all visual/AX state.
- `UNVERIFIED`: the §11.1 visible-frame stall gate (no present endpoint), per scenario.

## Volume

One repetition per scenario (6 runs). §10 asks for ≥30 interaction repeats and ≥5 × 60 s runs;
this record is **one observation per scenario**, not a distribution. A repeatable
self-terminating workload driver still needs: scripted per-scenario action traces with
achieved-cadence recording, deterministic in-process close/exit, longer declared scopes, a
repetition harness with recorded order, and aggregation/estimators (the S1 contract rejects
percentile claims).

## Replay

```
python -B apps/desktop/devtools/r0/s2b_validate.py capture --capture capture-run/<scenario>/s2b-capture.json
python -B apps/desktop/devtools/r0/validate.py pair r0c/<scenario>-manifest.json r0c/<scenario>-result.json
sha256sum -c SHA256SUMS
```

Files are bounded records only: no credentials, no user content, no absolute local paths.
