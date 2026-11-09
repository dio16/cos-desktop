# Plan §19.318 / §19.319 residual text proposals (Lead applies; the plan was not edited)

Source of the proposals:
[`acc1-native-closure-latest`](README.md) (session `PASS` for the measured steps;
see the README for the exact states and the named residuals).

## 1. §19.318 — replace the closing part of the **Verdict/residual** bullet

Replace this fragment (end of the §19.318 `Verdict/residual` bullet):

```text
… the pure UIA-focus-then-`WM_CHAR` abort
(`thread caused non-unwinding panic`) is fixed in §19.319, and native
keyboard/IME/AX stays `NOT_MEASURED` until the full accessibility matrix is
re-exercised. ACC2/product acceptance remains the Product owner's.
```

with:

```text
… the pure UIA-focus-then-`WM_CHAR` abort
(`thread caused non-unwinding panic`) is fixed in §19.319, and native
keyboard/IME/AX stays `NOT_MEASURED` until the full accessibility matrix is
re-exercised. The native artifact display, STOP, missing-media and
Compare/Evidence/Flow/History steps recorded here as `NOT_MEASURED` are closed by
the bounded RC-11 closure campaign (2026-11-10, [closure evidence](evidence-published/acc1-native-closure-latest/README.md)):
a test-only Rust-validated capability snapshot set in the isolated DB copy lets
the Runtime derive `STOP_TASK: SUPPORTED` and `ORIGINAL/THUMBNAIL: SUPPORTED`,
the native artifact surface fetched the descriptor and blob itself (both 200) and
displayed `メディア画像（記述子のダイジェスト検証済み）: 低解像度プレビュー。…`
(digest-matching 1,363,011-byte PNG), STOP painted the accepted receipt and the
projected state became `CANCELLED`, and removing only the isolated CAS blob made
the reloaded surface state `画像を利用できません: この成果物のメディアが見つかりません。`
while the descriptor stayed 200 and the blob fetch answered 404. Native History,
Flow and Compare were exercised (History/Flow/Compare PASS; Evidence PARTIAL —
no group is projected, the inspector states `判断の根拠 — 利用不可`), a second
recipe remains `NOT_RUN` (one-recipe catalog), and a new generation is
`NOT_RUN` because ComfyUI 0.35.1 cannot restart (`ModuleNotFoundError: No module
named 'comfy_aimdo.storage'`). ACC2/product acceptance remains the Product
owner's.
```

## 2. §19.319 — append to the **Residual** bullet

Append this sentence to the §19.319 `Residual` bullet (after the release-binary
and provenance items):

```text
The MP-ACC1 native residuals from §19.318 are re-measured on the RC-11
projection in the bounded closure campaign of 2026-11-10
([closure evidence](evidence-published/acc1-native-closure-latest/README.md)):
the golden generation produced `COMPLETED`/`SUCCESS_ACCEPT` with **0**
`409 RESYNC_REQUIRED` in 1,986 serving responses and a serving-owned epoch
across the mid-generation restart, the native image display and STOP and
missing-media states are measured, and the remaining gaps are the explicit
`NOT_RUN`/`PARTIAL` items in that record (one-recipe catalog, ComfyUI
`comfy_aimdo.storage` restart failure, Evidence groups not projected, Compare
panes not displaying an image in the bounded window, and the low-resolution
preview label for the displayed class).
```

## 3. Optional new evidence record (only if the Lead wants a dated section)

Suggested short paragraph for a new §19.320 (verbatim):

```markdown
### 19.320 MP-ACC1 native closure campaign on the RC-11 command/media projection (2026-11-10)

- **Authority and scope:** bounded closure of the §19.318/§19.319 native residuals
  for DC-MP-ACC1, after Runtime RC-11 `943e6b21` and the journal fixes
  `b794b637`/`0378fa40` and Desktop `a8f332a`. Test-only harness/evidence; no
  product code, no Runtime file, no push, no plan edit by the lane. Not product
  acceptance; ACC2 remains the Product owner's.
- **Result:** golden generation `PASS` (`COMPLETED`/`SUCCESS_ACCEPT`, digest-
  matching 1,363,011-byte artifact, 0 resyncs, serving-owned epoch), native image
  display `PASS`, STOP `PASS` (accepted receipt → projected `CANCELLED`),
  missing-media `PASS` (native `MISSING`, descriptor 200 / blob 404), native
  History/Flow/Compare `PASS` and Evidence `PARTIAL` (not projected), multi-recipe
  and a further generation `NOT_RUN` (one-recipe catalog; ComfyUI
  `comfy_aimdo.storage` restart failure). The golden generation and the native
  display are a split measurement on the same task/artifact; the record says so.
  Evidence: [acc1-native-closure-latest](evidence-published/acc1-native-closure-latest/README.md).
```
