# ADR-0013 — Evidence lives in the repository, latest only

- **Status:** Accepted — owner-directed 2026-10-25.
- **Owner statement (verbatim):** 「C:\tmp に一時ファイルが大量にある。私が消した。そもそも一時ファイルには証拠など存在しないはず。証拠は常にリポジトリに最新のみ保存すること、そうやってストレージ膨張を防ぐこと」
- **Decision:** a temporary directory is **never** an evidence store. Evidence is the repository, and the repository keeps **only the latest** evidence for each area so the checkout does not grow without bound.

## 1. Rules

1. **No evidence in temp.** `C:\tmp`, `%TEMP%` and any scratch path are disposable working space. A record must never cite a temp path as the place where its evidence lives, and a claim may not rest on bytes that exist only there.
2. **The repository is the store.** A run's evidence is copied into a tracked path before it is cited in a plan entry, a review, or a gate conclusion. If it is not in the repository, it does not exist for the purposes of a claim.
3. **Latest only.** When a new record supersedes an older one for the same area, the older one is pruned from the working tree and the supersession is recorded in one line (what was removed, what replaced it, and its digest). A prune is not a rewrite of history: the ADRs, the decision record and the acceptance record keep their conclusions, and no prior verdict is retroactively changed.
4. **Digests instead of bulk.** Large artifacts — executables, archives, screenshots — are not committed in quantity. The record keeps their **digest and size**, and the artifact itself is committed only when it is small and genuinely needed to re-verify the claim.
5. **Scratch is declared, not implied.** A harness may use a scratch directory, but its manifest must say that the directory is disposable, and no verification may depend on a scratch file surviving.

## 2. What this explains retroactively

The N07 baseline binaries (`a0/cos-desktop.exe`, `a1/cos-desktop.exe`) were referenced by a record whose artifact paths pointed into `C:\tmp`. Those bytes were therefore **never under repository control**, and their deletion is the expected consequence of rule 1 rather than an accident to be reverted. The eleven N07 assertions that needed them stay `NOT_RUN` with that reason; they do not silently become passing.

## 3. Consequences

- **Good:** the checkout stays small, and every claim points at bytes a reviewer can actually retrieve from the repository.
- **Cost:** a bulky artifact can no longer be "kept around" as convenience evidence; it must be re-produced from a pinned commit, and the record must state that reproduction is not bit-identical when that is the case.
- **Rollback:** revert this ADR. Temp-based evidence would again be permissible, and the storage growth it enables returns with it.
