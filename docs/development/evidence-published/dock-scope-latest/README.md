# Candidate dock task scoping: ui-model fix and native evidence (2026-11-10)

Bounded investigation and fix of the residual recorded by
[compare-panes-latest](../compare-panes-latest/README.md) finding 1 and plan
§19.328: the candidate dock listed candidates from **every** task in the
snapshot. This record states the root cause, the product fix, its regression
tests and the native observation. It is evidence and implementation, **not**
product acceptance; ACC2 remains the Product owner's and the plan text is a
proposal (the Lead owns the plan).

## Verdict

**Product defect confirmed and fixed in `ui-model`.** `candidate_groups` and the
flattened dock rows were built from all snapshot candidates without a selected
task parameter, so the live ACC1 session rendered eight tasks' `C1` rows, pair
selection could mix tasks and the labels collided (`C1`/`C1`). Compare itself
resolved each side by full `CandidateRef`, so there was no wrong binding — the
defect was what the dock exposed and allowed the user to pair.

| Hypothesis | Result | Evidence |
|---|---|---|
| (a) product defect: the dock projection is snapshot-wide | **CONFIRMED** | `candidate_groups` grouped every `snapshot.candidates` row (`crates/ui-model/src/lib.rs:1809` before the fix) and `derive()` never passed the selected task into the projection. |
| (b) only a harness/selection issue | **REJECTED** | The same eight-task rows appear in the live UIA tree, not only in the harness step (`uia` names in `compare-panes-latest`). |
| (c) Compare binds the wrong candidate | **REJECTED** | A pair holds complete `CandidateRef`s and the compare media path resolves by that identity; the defect is the input set, not the binding. |

## Root cause

1. `DerivationCache::derive_snapshot` derived `CandidateDisplay`s and
   `CandidateGroupDisplay`s from **all** `snapshot.candidates`; it had no
   selected-task argument (`Scope` carried only `session_generation`).
2. `DerivationCache::derive` resolved the selected task only afterwards, for the
   title/axes/stop/inspector projections, so the dock and those surfaces could
   describe different task sets.
3. The projection short-circuit compared only the snapshot revision. A rail
   selection change at the same publication revision would have returned the
   previous task's dock even if a filter were added, so the dock task is part of
   the reuse key.
4. The dock is the only pair-select surface (`Presentation::select` is fed from
   `text.artifacts`), so the snapshot-wide row set is what allowed a cross-task
   pair and the label collision.

## Fix (`2fa792d`, ui-model only)

- `derive()` resolves the effective task first — the explicitly selected
  identity, otherwise the first projected task — and passes
  `task.map(|task| &task.task_ref)` to `derive_snapshot`. A selected identity
  that is no longer projected does **not** fall back: the dock stays empty.
- `derive_snapshot` keeps only candidates whose authoritative
  `CandidateSnapshot.task_ref` is that identity. Grouping, flattened order and
  the `反復情報なし` group are unchanged; candidates of other tasks no longer
  enter either. `None` (no projected task) leaves the dock empty instead of
  listing every task.
- `candidate_task` is stored with the derived groups and is part of both the
  short-circuit key and the rebuild condition, so changing the rail selection at
  an unchanged snapshot revision re-derives the dock.
- The cache still holds display records for every snapshot candidate so a task
  switch reuses the rows; the filter applies to what the window may present.
  No candidate is synthesized and no Task semantics are inferred.
- `ProjectionDisplay`/`WorkbenchText` docs now state the selected-task scope.
- `desktop-gpui`/`app-core` needed no change: the dock, Compare labels/presence,
  palette facts and the persisted-reference restore all read
  `text.artifacts`, and `dock::switch_task` already calls
  `Presentation::reset_for_task` (D-9).

## Regression tests

New/updated ui-model tests (all drive the real derivation):

- `dock_groups_exclude_candidates_and_iterations_of_other_tasks` — an
  authoritative candidate and iteration of another task are absent from the
  groups and rows, while the selected task's unattributed row stays.
- `selected_task_replaces_the_dock_without_a_snapshot_revision_change` — one
  publication/revision, two selected tasks with the same labels `C1`/`C2`; the
  dock replaces, each label is unambiguous inside its selected task, and
  switching back reuses the row displays.
- `switching_tasks_clears_the_compare_pair_and_a_cross_task_pair_is_unavailable`
  — a pair from the selected task's rows is complete; `reset_for_task` clears
  viewed/pair; a persisted cross-task pair is refused (`pair_missing`) against
  the selected task's rows and a same-task pair still forms.
- `a_task_without_projected_candidates_keeps_an_empty_dock` — an empty selected
  task, a vanished selected identity and the initial-task path (which still
  shows its own row) keep the honest empty state.
- `selected_task_drives_title_axes_and_stop_at_the_same_publication_revision`
  now also asserts the selected task's dock is empty when it projects no
  candidates.

```text
cargo test -p ui-model                                  -> 164 passed, 0 failed
cargo test -p desktop-gpui                              -> 280 passed, 0 failed, 5 ignored
cargo test -p cos-desktop                               -> 93 + 141 + 4 passed, 0 failed (2 ignored)
cargo clippy -p ui-model --all-targets -- -D warnings   -> exit 0
cargo fmt --all -- --check                              -> clean
cargo metadata --locked --no-deps --format-version 1    -> ok
python -B scripts/check_dependency_rules.py --self-test  -> 19 tests OK
python -B scripts/check_dependency_rules.py             -> 7 members OK
```

`TEMP`/`TMP` were `target/tmp-test` for every run. The sccache server had to be
restarted once after that environment change (the previous server had inherited
a `/c/...` temp path); no product code was involved.

## Native evidence

- **Source/binary binding:** code and tests `2fa792d30e1ba7572245fef4194d378293ab6029`
  (product crates clean); release binary `target/release/cos-desktop.exe`,
  **21,215,232 B**, sha256
  `a2701360d35ad7391cf60256ee115d91982533ba6dc7bc4966a50bc2321742d8`.
- **Isolation:** isolated Runtime on port **8134**, fresh copy of the measured
  run's DB (sha256 `0707f38725e163fc6d8ac31f51fbeeafeaaa35eac1a10931714db34ead7d4a29`,
  11,800,576 B) and its CAS (one blob), new serving epoch
  `360b6c2d06b549298e0d2c5f327df310`; **no generation**; redirected discovery
  descriptor removed on the clean lifespan shutdown.
- **Command:**

  ```powershell
  C:/AI/orchestrator/.venv/Scripts/python.exe -u `
    apps/desktop/devtools/runtime_integration/run_acc1_native.py `
    --scenario dock-scope `
    --source-run target/acc1-native/20261110T005304-artifact `
    --task-ref task-ab47119a8bec467a8e128d7b82f1220a `
    --second-task-ref task-88cf3e16dac94436a9396b364f6d5bd0 `
    --no-build --desktop-exe target/release/cos-desktop.exe `
    --evidence-dir docs/development/evidence-published/dock-scope-latest `
    --port 8134
  ```

  The new `--scenario dock-scope` is test-only harness code; it launches the
  real release window against the isolated Runtime, drives UIA and records the
  names/screenshot hashes above. The previously measured `compare-panes-latest`
  run selected one row per task because the dock offered them; this scenario
  measures the new expectation instead of reproducing that pair.

- **Observed — two-candidate task `task-88cf3e16…` (`production automatic
  dispatch acceptance 2`):** the dock showed **exactly two rows, both that
  task's** (`C1`, `C2`, no foreign row). Invoking each row produced element ids
  whose decoded task identity is the same task
  (`candidate:37:task-88cf3e16…48:run-…`), the pair completed and the Compare
  surface opened (`A: C1`, `B: C2`). `pair_same_task = true`; the row screenshot
  is [dock-scope-two-compare.png](shots/dock-scope/dock-scope-two-compare.png)
  (sha256 `ab6c7de6…`, UIA names hash
  `sha256:4831a4ae3e38d0e5b8b78d94cd060e14055354d202ef93a3ec47fe453e82ae6f`).
  Both panes report the explicit capability-not-projected refusal (the reused
  DB has no CAPABILITIES projection for that task), not a wrong image.
- **Observed — one-candidate ACC1 task `task-ab47119a…`:** the dock showed
  **exactly one row, ACC1's own**, with zero foreign rows. The previously
  exposed other-task row is not in the tree: probing
  `task-88cf3e16… — 第0回` returns `match_not_found`. The Compare switcher states
  `候補を比較 — 候補を2つ選ぶ必要があります`, the `Ctrl+2` label is gone, and the
  surface stays on Artifacts (`surface_open = false`) — a cross-task pair cannot
  be selected at all. Screenshot
  [dock-scope-one-compare-refused.png](shots/dock-scope/dock-scope-one-compare-refused.png)
  (sha256 `932c8a79…`, UIA names hash
  `sha256:a1dd5989f3de72341e4699c8d1c4889bc806ee6c21ac811d0012e818c6408126`).
- **Cleanup:** Desktop closed with `WM_CLOSE` (exit 0), isolated Runtime exited 0,
  ports 8134/8133 free, discovery descriptor absent, credential-store targets
  reported `not_found`; leak scan 96 files / 3 secrets / **0 findings**;
  `session.json` sha256 `9e57dddc…` (see `session.sha256`).

## Honest limits

- The reused DB/CAS carries no CAPABILITIES projection for the two-candidate
  task and only one blob, so both panes showing pixels simultaneously is
  `NOT_MEASURED`; the pair/admission result does not depend on it.
- No generation was run; the session measures the dock projection and pair
  admission, not provider execution.
- Switching tasks while a pair is open was covered by the ui-model regression
  test (`reset_for_task`), not by a native step; the native session closed
  Compare before the second selection.
- The two-candidate task's two candidates share one artifact digest and are in
  the same iteration; a multi-iteration/multi-artifact pair remains the Compare
  lane's open item (plan §19.328).

## Plan wording (proposal; Lead owns the plan)

> The candidate dock and its iteration groups are now derived only for the
> selected task (`ui-model` `2fa792d`): no other task's rows/labels reach the
> dock or Compare's pair inputs, a task switch replaces the dock at an unchanged
> snapshot revision, and a vanished selected identity leaves it empty instead of
> falling back. Native evidence on the isolated port-8134 Runtime
> ([dock-scope-latest](evidence-published/dock-scope-latest/README.md)) shows a
> two-candidate task pairing only its own two rows and the one-candidate ACC1
> task exposing exactly one row with Compare refused and the other task's row
> absent. The closure DB's both-pane pixels and multi-artifact pairs remain open.
