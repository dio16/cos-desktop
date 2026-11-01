# ADR-0017 — Batch DD closure: keyboard map, packaging, diagnostics retention, support matrix, decoder and measurement authorizations

- **Status:** **Accepted — owner rulings 2026-10-31 / 2026-11-01.** The Product/UX owner was asked to decide each item below and selected the stated value; measurement proposals were approved separately. Caller permission is recorded here only for the named scope, and no missing evidence is promoted.
- **Scope:** DD-05 keyboard map, DD-21 packaging/signing split, DD-22 diagnostics retention, DD-23 supported matrix, DD-08 decoder implementation choice, and the measurement authorizations for R0B, MP-TYP1's remaining scale matrix and MP-FLOW2 A–C.

## 0. Rulings

| ID | Ruling | Consequence |
|---|---|---|
| DD-05 | The implemented map is the ratified baseline: Ctrl+1/2/3 surfaces, Ctrl+Tab next surface, Ctrl+K palette, Tab/Shift+Tab traversal, list arrows, Enter/Space activation, Escape dismissal, Ctrl+D diagnostics. Home/End list jumps stay unimplemented. | The palette inventory and key paths in MP-UX4/MP-NAV5 become the accepted baseline; adding keys is a new decision. |
| DD-21 | Distribution targets are a portable ZIP and a signed MSIX. The certificate, publisher and signing operation remain external Operator inputs. | Packaging work may implement both formats; no unsigned release claim and no automatic updater. |
| DD-22 | Diagnostics keep an in-memory ring of 4096 records and retain at most three crash reports of 5 MiB each; redaction is always applied and credentials are never written. | SEC2/REL3 may implement these bounds; changing them is a new decision. |
| DD-23 | Supported matrix: Windows 10 22H2 and Windows 11 x64, display scales 100/125/150/175/200 %, Narrator and NVDA, High Contrast Black and White. | These are targets; unmeasured cells stay `NOT_MEASURED` and no PASS is inferred. |
| DD-08 | The bounded image decoder uses the pure-Rust `image` crate with only `png`, `jpeg`, `webp` and `gif` features enabled; WIC is a later decision. Decode limits come from the existing `DecodeBudget`, never inline numbers. | MP-IMG2/IMG3/IMG4/IMG5/CMP1 may implement against this backend inside the bounded profile; full-target decode and video remain out of scope. |
| R0B | A fresh instrumented build from current source may be built, its digest recorded, and the idle baseline measured as five runs of 30 s settling plus 60 s observation. | The earlier HOLD is superseded for this measurement only; results are NOT_MEASURED until the runs exist. |
| MP-TYP1 | The DPI matrix may be measured with the new `-DpiScale` probe and Operator-driven scale changes. | 125 % capture is already observed; 150/175/200 % remain unmeasured until run. |
| MP-FLOW2 | The R6 A–C experiment may use a synthetic 10,000-node graph to measure layout and paint cost. | Stage selection follows the measured result; no stage is selected in advance. |

## 1. Boundaries

- None of these rulings is implementation completion, measurement or product acceptance; each named contract still owns its evidence and exit.
- The decoder dependency is admitted only with the four named features; adding another format or a native codec is a new decision.
- Display-scale changes stay Operator actions; the agent never changes OS settings.
- Existing holds not named here (Runtime integration, video, updater delivery) are unchanged.
