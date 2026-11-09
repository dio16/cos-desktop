# Compare pane media: ACC1 closure residual investigation, fix and native evidence (2026-11-10)

Bounded investigation of the residual recorded by [acc1-native-closure-latest](../acc1-native-closure-latest/README.md):
both Compare panes reported `画像は未表示です` although the artifact viewer displayed
the selected task's admitted artifact. This record states the root cause, the verdict,
the minimal product fix, its regression tests and the native observation. It is
evidence and implementation, **not** product acceptance; ACC2 remains the Product
owner's, and the plan wording below is a proposal (the Lead owns the plan).

## Verdict

**The residual was (a) a product defect: the production Compare surface had no media
wiring at all.** It was neither (b) an "expected state because the pair did not resolve
to admitted media" nor (c) a harness limitation.

| Hypothesis | Result | Evidence |
|---|---|---|
| (a) product defect in Compare media wiring/state | **CONFIRMED** | No production call site ever hands a frame to a compare pane, and the reason strings the panes showed are `CompareView::await_side` / `set_projection` texts, not media outcomes (see *Root cause*). |
| (b) pair did not resolve to admitted media | **REJECTED** | The closure's pair was a concrete two-candidate pair from the dock (`[task-00eeff78…, task-ab47119a…]`); the recorded "second selection" `反復回数: 第0回` was a synthetic click on an iteration group **text**, not the pair's source. Both candidates have projected artifacts, and the ACC1 artifact was admitted and displayed by the artifact viewer in the same session. |
| (c) harness limitation | **REJECTED** | The suppression is in the application itself: both panes' `ImageViewer::current` is always `None` because nothing in the production shell calls the pane presentation/repository path. A different harness cannot change that. |

The closure's pair semantics are still worth recording: the harness's row filter
(`"第0回" in line`) matched the iteration header as the "other candidate", so its second
selection was a no-op click; the pair itself came from an earlier anchor invoke of
another task's dock row. That is a harness defect, but it did not cause the empty panes —
even a correct pair could not display a picture before this change.

## Root cause

1. A compare pane is an `ImageViewer`. It can receive a frame only through
   `CompareView::{load_generated_pair, load_generated_side, present_prepared,
   restore_prepared, swap_resources}` (`crates/desktop-gpui/src/compare_view.rs:370,
   :486, :543, :560, :331`).
2. The production shell called none of them. Its compare panel only called
   `set_selection_presence`, `set_projection`, `await_side` and `close`; a repository-wide
   search for `present_prepared|load_generated_pair|load_generated_side` outside
   `apps/desktop/examples/compare.rs` returns no hit. The only caller of that pair-input
   path is the experimental example harness (`apps/desktop/examples/compare.rs`).
3. `CompareView::await_side` (`compare_view.rs:312`) writes
   `比較対象が変更されました。この候補の認可済み画像はまだ表示されていません。` and its own
   doc comment says *"The host presents or loads that side next."* The host never did, so
   the "pending" sentence was permanent rather than transient.
4. The artifact viewer's path (`Shell::sync_artifact_media`,
   `crates/desktop-gpui/src/lib.rs:2410`) returns early unless the surface is `Artifact`,
   owns the single `media_loaded` identity and addresses only the Viewed candidate. It
   never addressed a compare side.
5. `ImageViewer::render` (`crates/desktop-gpui/src/image_viewer.rs:1398`) therefore always
   fell to the `current == None` description `画像は未表示です`.

The closure's own UIA dumps confirm this: A's reason is exactly `await_side`'s text and
B's is `set_projection`'s re-offer text (`compare_view.rs:220`); neither is a
fetch/prepare/failure wording. No descriptor or blob request existed for a compare pane.

## Fix (minimal, one layer)

- `crates/desktop-gpui/src/compare_media.rs` (new): a projection-only decision per side
  (`side_media`) that returns `NotProjected` / `MissingArtifact` / `Refused(reason)` /
  `Ready(selection + capability identity)`, plus `CompareMediaState` that tracks the
  identity each side addresses, the identity already dispatched and its cancellation.
- `crates/desktop-gpui/src/compare_view.rs`: `resolve_media` (cancel the side's generated
  input work, clear its old reason, delegate to the pane's repository resolution) and
  `has_frame`.
- `crates/desktop-gpui/src/lib.rs`: `Shell::sync_compare_media` runs while the Compare
  surface is shown. Per side it resolves the pair candidate through the publication
  (`artifact_of_candidate` + `capability::for_task`), gates `ORIGINAL`, builds the same
  canonical `MediaLoadKey` the artifact surface uses, and dispatches through
  `MediaRuntime` → `ImageViewer::resolve_media`. Absence, a withheld representation and a
  missing session binding each produce an explicit sentence. It is triggered when Compare
  is navigated to, on publication install, on pair change, on swap, and cancelled/forgotten
  on close and task switch. Swap cancels in-flight work, swaps the per-side identity and
  re-resolves only a side that ends without a frame, so the frame stays with its candidate.

No canonical state is derived: the pair remains a local selection, and every displayed
artifact identity comes from the publication.

## Regression tests

```text
cargo test -p desktop-gpui --lib compare_media      -> 7 passed, 0 failed
cargo test -p desktop-gpui --lib                    -> 280 passed, 0 failed, 5 ignored
cargo test -p cos-desktop                           -> 90 passed / 141 passed / 4 passed, 0 failed
                                                       with TEMP=TMP=target/tmp-test (see below)
cargo clippy -p desktop-gpui --all-targets -- -D warnings -> exit 0
cargo fmt --all -- --check                          -> clean
cargo metadata --locked --no-deps --format-version 1      -> ok
python -B scripts/check_dependency_rules.py --self-test   -> 19 tests OK
python -B scripts/check_dependency_rules.py         -> allowlist edges verified (7 members)
```

The app-crate `instance`/`window` tests create their scratch directories under
`std::env::temp_dir()`; on this host the default `%TEMP%` path fails with
`Unavailable(CreateDirectory)` (the same environmental instance-guard deviation the
plan records in §19.304), so the run above uses a workspace TEMP. This is not caused by
the change: with `TEMP`/`TMP` pointed at a writable directory the same tests pass, and
nothing in this change touches `apps/desktop/src`.

The new tests drive the real decision function: a supported `ORIGINAL` becomes a
selection carrying the artifact identity; a candidate outside the projection, a candidate
without an artifact and a withheld/unprojected capability each become their explicit
state; two candidates never share a load identity; renewing a side cancels the superseded
resolution; swap moves identities without touching the other side; forgetting drops them.

## Native evidence

- **Source/binary binding:** `3e381ca` (product crates clean), release binary
  `target/release/cos-desktop.exe`, 21,156,864 B, sha256
  `d2009ebe27ec4c8d7f2736f8cc596f03c29f516c569ba8e3ce6ab3a6e242b4eb`.
- **Isolation:** isolated Runtime on port **8134**, fresh copy of the measured closure
  DB (sha256 `0707f387…`, 11,800,576 B) and its CAS (one blob), new serving epoch
  `d2612d1e…`; **no generation**. Command:

  ```text
  python apps/desktop/devtools/runtime_integration/run_acc1_native.py \
    --scenario artifact \
    --source-run target/acc1-native/20261110T005304-artifact \
    --task-ref task-ab47119a8bec467a8e128d7b82f1220a \
    --no-build --desktop-exe target/release/cos-desktop.exe \
    --evidence-dir docs/development/evidence-published/compare-panes-latest \
    --port 8134
  ```

- **Observed (session `record-artifact.json`, step `steps.surfaces.compare`, verdict PASS):**
  - dock rows selected through UIA `InvokePattern`:
    `task-ab47119a… — 第0回 — C1` (the ACC1 candidate) and `task-be1ffede… — 第0回 — C1`
    (a second, different task's candidate from the same dock);
  - Compare surface: `A: C1` → `機能の利用可否情報が投影されていません（capability not projected）`
    + `画像は未表示です`; `B: C1` → `メディア画像（記述子のダイジェスト検証済み）:
    低解像度プレビュー。原寸の詳細表示・1:1ではありません。` — **the pane displays the
    admitted artifact**, see [compare-surface.png](shots/artifact/compare-surface.png)
    (sha256 `e1c40860…`);
  - `A/Bを交換` → `A` keeps the frame under the moved candidate
    (`ローカル画像: 低解像度プレビュー…`, restore path, no re-decode) and `B` keeps the
    refusal under its own candidate: [compare-swapped.png](shots/artifact/compare-swapped.png)
    (sha256 `52a1818a…`);
  - artifact viewer in the same session: state `DISPLAYED`, fetched bytes
    [artifact.png](artifact.png) sha256 `eea85a247125…3908a58` == the descriptor digest;
  - Runtime media log: descriptor `200` ×3 and blob `200` ×2 for `sha256:eea85a24…`;
    the two blob transfers cover the artifact-viewer presentation and the compare-pane
    presentation. The pane's status line is the admitted-media success text
    (`media_success_text`), which only the verified presentation path can produce.

- **Honest limits of this run:** the reused DB/CAS carries exactly **one** displayable
  artifact, and the ACC1 task carries exactly **one** candidate, so a *same-task* two-image
  pair cannot exist in this DB. The second side was another task whose capability
  projection is absent from that DB copy, so it correctly renders an explicit refusal
  rather than a picture. Two panes showing pixels simultaneously is therefore
  **NOT_MEASURED**; what is measured is that a compare pane now displays the same
  digest-verified admitted preview as the artifact viewer.
- `native-ax-ime-dpi-latest` was not modified: it measures a different lane (UIA/keyboard/
  IME/DPI), and the compare panes' own AX/DPI behaviour remains `NOT_MEASURED`. The
  fixture-mode (`MediaBinding::Absent`) Compare wording was not re-captured natively after
  the change; it now shows the existing `media_binding_statement` sentence instead of the
  permanent "still not displayed" text, but that state is unmeasured here.
- Two panes displaying pixels at the same time is `NOT_MEASURED` (see the DB/CAS limit
  above); the 1:1/full-detail class stays MP-IMG6 work.

## Correct UX state (what Compare should display)

| State | Pane wording |
|---|---|
| no Runtime media boundary (fixture/absent binding) | the session statement from `media_presentation::media_binding_statement` |
| candidate not in the projection | `画像を利用できません: 比較対象の候補は、現在閲覧できる情報に含まれていません。` |
| artifact absent from the complete projection | `画像を利用できません: この成果物のメディアが見つかりません。` |
| representation withheld by the capability projection | the gate's own reason, verbatim |
| resolution in flight | the pane's preparing status |
| admitted | preview pane + `メディア画像（記述子のダイジェスト検証済み）: 低解像度プレビュー…` |
| pair not complete | `A: 未選択` / `B: 未選択` with the dock's next-step sentence |

## Independent findings (not fixed here)

1. **The candidate dock is not scoped to the selected task.** `ui-model` derives
   `ProjectedDisplay.candidates` from every candidate in the snapshot
   (`crates/ui-model/src/lib.rs:1006`) and groups them by `(task_ref, iteration)`
   (`:1809`), so the dock lists candidates of *all* tasks (UIA rows
   `task-be1ffede…`, `task-d493aa7e…`, …). A pair can therefore mix tasks, and live
   `display_label` collisions (`C1` / `C1`) make A and B indistinguishable by label.
   Compare itself stays exact (the pair holds full `CandidateRef`s, so nothing is
   merged), and a cross-task side now renders its capability refusal instead of a wrong
   picture. Correct UX: scope the dock to the selected task. This is a separate
   `ui-model`/dock defect, not part of the Compare pane wiring fix.
2. **Harness row selection** was fixed with this evidence: rows are now the real candidate
   rows (`" — 第"` and not the iteration header), the Compare step waits for the panes to
   settle instead of snapshotting the pending sentence, and it records the swap.

## Proposed plan residual wording (Lead owns the plan; not applied here)

For MP-CMP1:

> Production Compare now acquires each pair side through the authorized media boundary
> (candidate → artifact → capability gate → repository) with per-side identity,
> cancellation and explicit not-projected/missing/refused/no-binding states; natively
> observed at `3e381ca` where one pane displays the digest-verified admitted preview and
> the other states its capability refusal (port 8134, fresh DB/CAS copy,
> [compare-panes-latest](evidence-published/compare-panes-latest/README.md)). Still open:
> a same-task two-artifact pair (the ACC1 DB has one candidate), the candidate dock's
> cross-task scoping (`ui-model` groups every snapshot candidate), native AX/DPI for the
> panes, the R5 reuse/fairness matrix and product acceptance.

For the ACC1 closure residual bullet:

> Compare A/B image display: **closed for the ACC1 candidate** — the compare pane displays
> the same digest-verified admitted preview as the artifact viewer at `3e381ca`; showing
> both panes simultaneously needs a two-candidate task, which the closure DB does not
> contain, and the dock's cross-task candidate scoping is a separate open defect.
