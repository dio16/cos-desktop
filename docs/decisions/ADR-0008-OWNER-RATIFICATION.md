# ADR-0008 — Owner ratification: decoder adoption, declared budgets, R0D gates and design gates

- **Status:** Accepted (owner-ratified). 2026-10-24.
- **Decision owner:** the repository owner / Caller, by a direct instruction in the implementation session.
- **Supersedes:** nothing. It ratifies decisions that earlier documents left `Proposed` or `PENDING`; it does not rewrite any historical record, Gate receipt or evidence note.
- **Scope:** this is the owner decision record. `IMPLEMENTATION_MASTER_PLAN.md` §19.28 is its derivative execution note, and the task cards whose gates this ADR closes are updated there. Behavioural requirements still live in the owning Design Contract (`DESKTOP_DESIGN_CONTRACTS.md`) and the Product Canon; this ADR decides gates, not behaviour.

## 1. Authority recorded verbatim

The Caller wrote:

> 全て採用。予算は適切な値を仮定し実装。可変可能なこと＝ハードコードしない。数値批准。設計ゲート承認。

Read literally and in the context of the report it answered (MP-IMG2 remaining items, R0D ratification and the fourteen design-gated tasks), this is a blanket owner decision that (a) adopts the pending technical choices, (b) authorises assumed budget values to be implemented, (c) requires those values to be changeable rather than hardcoded, (d) ratifies the numeric gates, and (e) approves the design gates.

## 2. Decoder adoption

- **Product decode path:** Windows Imaging Component (WIC) through the existing managed WIC/WPF probe boundary. It is the only measured candidate that refused the hostile cases without allocating the declared surface first, and it agreed pixel-for-pixel with the other candidate on valid PNG and BMP input (§19.25, §19.27).
- **GDI+ is not adopted as the product decoder.** Measured reasons: no header-only load (a 65-byte 20000×20000 claim raised the working set 91.3 → 1243.2 MiB before any pixel access, so `-MaxPixels` refusal is post-allocation), no unsupported-versus-invalid distinction, and a corrupt zlib payload is accepted and re-encoded. It remains an exploratory comparison candidate only.
- **FFmpeg 8.0 remains an external experiment harness only.** No product dependency is introduced; `check_dependency_rules.py` and the allowlist are unchanged.
- **What this does not decide:** colour management/profile conversion, EXIF orientation, region/sequential/tiled capability and wide-gamut/HDR presentation remain open and are recorded `NOT_MEASURED`/`Deferred` where they are not implemented.

## 3. Declared provisional budgets (ratified defaults, configurable)

Every number below is a **declared provisional default on an unmeasured basis**: the owner authorised assuming appropriate values, and they are ratified as the desktop defaults, not as measured attainment. Each must be changeable through an explicit configuration value — a hardcoded literal at a use site is a defect, not an implementation choice. Numeric revisions require re-ratification; measurement may replace a default without inventing a new one.

| Budget | Ratified default | Basis |
|---|---|---|
| Decode `max_dimension` | 16,384 px | rendering §10 stress-input ceiling |
| Decode `max_pixels` | 268,435,456 | 16,384² |
| Decode `max_bit_depth` | 16 | exceeds the 8-bit path exercised so far |
| Decode `max_decoded_bytes` | 268,435,456 (256 MiB) | aligned to the CPU decoded-image cache |
| Decode `max_scratch_bytes` | 268,435,456 (256 MiB) | bounded decoder scratch claim |
| CPU decoded-image cache | 268,435,456 (256 MiB) | rendering §10 tunable default |
| GPU image/graph cache | 536,870,912 (512 MiB) | rendering §10 tunable default |
| Upload staging | 67,108,864 (64 MiB) | rendering §10 tunable default |
| In-flight decoded/upload payload | 67,108,864 (64 MiB) | rendering §10 tunable default |
| Upload cap per frame @60 Hz | 4,194,304 (4 MiB) | rendering §10 tunable default |
| Upload cap per frame @120 Hz | 2,097,152 (2 MiB) | rendering §10 tunable default |
| Concurrent decode jobs | 2 thumbnail/preview + 1 full/tile | rendering §10 tunable default |
| Queued visible image requests | 16 | rendering §10 tunable default |
| Graph-layout jobs | 1 active + 1 pending revision per graph | rendering §10 tunable default |
| Background search/index jobs | 2 per window | rendering §10 tunable default |
| First window / useful shell | p95 ≤ 1,500 ms warm-cache fresh process | rendering §10 measured-baseline candidate |
| Main-thread frame work | ≤2.5 ms p95 / 4 ms p99 @120 Hz; ≤5 ms p95 / 8 ms p99 @60 Hz | rendering §10 |
| Input-to-visible | ≤50 ms p95 / 100 ms p99 ordinary interactions | rendering §10 |
| Idle CPU/GPU | ≤1 % of one logical CPU average, ≤2 % p95 over measured intervals | rendering §10 |

Ceilings are upper bounds, never targets to fill, and no budget may force unsafe pressure on a lower-memory device.

## 4. R0D and the measurement gates

- **R0D is ratified provisionally:** the declared budgets above and the rendering §10 hypotheses are now the ratified gates for downstream tasks to cite, with the explicit status `RATIFIED_DECLARED_BASIS_NOT_MEASURED`. Downstream work may cite them; no document may present them as measured attainment, and R0B/R0C measurement, when it lands, replaces the declared value through this ADR rather than through an ad-hoc numeric change.
- **R0A/N06 keeps its own status:** N06's independent frame count is still `NOT_MEASURED` (no per-app presentation counter exists in this build), and `refine` is absent on this host so no independent Refine Gate was dispatched. Those facts are unchanged by this ratification.
- **MP-IMG2 remaining items** (real decoder scratch and concurrent memory, in-decoder cancellation granularity, per-codec coverage, region/sequential capability) stay measurable work with the ratified budgets as their ceilings; they are not claimed complete.

## 5. Design gates approved

The fourteen design-gated tasks are authorised to proceed, and the documented `Proposed` values become `Accepted` where a document already proposes a concrete value. Where a gate is a *question list* rather than a value, this ADR adopts the documented default and leaves any question the documents do not answer open — an unanswered question blocks only its own task.

| Gate | Decision |
|---|---|
| DD-03 / UX-01 (Compare and viewing) | Approved. The linked-transform model, initial viewed candidate, second-selection behaviour, fit/1:1 default, clamping, swap and pane-focus defaults are those already documented in rendering §4.1 and DC-MP-CMP1/DC-MP-IMG5; any behaviour those documents do not specify stays open and blocks only that behaviour. |
| DD-04a / DD-04b (typography) | Approved. Rendering §9.1 typography tokens and §9.2 geometry tokens become Accepted as the initial adopted scale, with the mixed-script and DPI validation conditions unchanged. |
| DD-05 (palette/keyboard inventory) | Approved for the documented inventory; keys not documented remain open. |
| DD-06 / DD-25 (brief fields, start/next-iteration semantics) | Approved for the documented field set and command semantics; a Runtime operation that does not exist is still reported unavailable rather than invented. |
| DD-07 (history/reopen) | Approved for the documented behaviour. |
| DD-22 (retention/export/split) | Approved for the documented bounded retention; any export path still requires its own secret-scanning evidence. |
| DD-23 (release matrix) | Approved as a declared matrix; unmeasured rows stay `NOT_MEASURED`. |
| DD-26 (inspector) and the remaining design-gated tasks | Approved against their existing DC text; nothing is added to a task's behaviour by this approval. |

## 6. Video

The MP-VID1 capability/interop probe is now authorised (it was the blocking authorisation). MP-VID2's path decision remains an owner decision after the probe reports, and MP-VID3/MP-VID4 stay unauthorised until then.

## 7. Boundaries this ADR does not move

- No Runtime acceptance, no Runtime mutation, no `cos-desktop`-side reimplementation of COS semantics. The RC-* Runtime-acceptance gates in the Design Contracts and the master plan are **not** closed by this ADR, and no task card may read this ratification as Runtime permission.
- **Task-card statuses are not overwritten:** where a §7 card still shows its pre-ratification status (for example a `DESIGN_GATE_REQUIRED` label), this ADR supplies the gate decision for the DD design gates, the declared-basis numeric ratification and the VID1 probe authorisation only. Any prerequisite a card names that is not one of those (a real Runtime operation, measured native evidence, Product/UX validation) stays unfulfilled and keeps blocking its own acceptance.
- No Product/UX validation claim, no release or `main` integration approval, no credentials or permission change, no GPUI pin change.
- A ratified budget is not a measured result; a PASS from an external reviewer is not owner acceptance.
- `refine` remains unavailable on this host, so independent Refine Gates for future candidates remain `NOT_RUN` unless the tool is installed.

## 8. Consequences and rollback

- **Consequences:** the tasks listed in the master plan §19.28 become implementation-eligible in DAG order; every numeric default must be reachable through configuration, and a literal budget at a use site is a review finding.
- **Rollback:** revert this ADR and the derivative plan subset (§19.28 plus the status lines it changed). Because a rollback would re-open gates rather than change behaviour, it needs no code rollback; budgets introduced as configuration can stay, but they would no longer be ratified defaults.
