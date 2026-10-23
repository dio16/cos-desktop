# Architecture / plan quality finalization evidence

**Scope:** 2026-10-08 design, execution-plan and testing-policy finalization from `dev` baseline `b4342f30a3aeda1d97f833cf97b58f7a4013957d`, on `feature/architecture-plan-quality-finalization`. This report records observations and their limits; it does not authorize product implementation, ratify R0 thresholds, accept the MVP, or approve Runtime/Protocol/Video changes. Product code, dependencies, GPUI revision and external repositories are unchanged. Only the existing app-core test module changes executable source.

## Provenance and measurement environment

Initial `git fetch origin` completed; local `dev`, `origin/dev`, HEAD and the reviewed baseline were identical, with a clean worktree and no intervening baseline changes. A separate feature branch was created without discarding changes. Measurements use the original production source with documentation/test-only changes; they are not a new feature implementation.

| Field | Observed value / limitation |
|---|---|
| OS | Windows 11 Pro, `10.0.26300`, x64 |
| CPU / RAM | AMD Ryzen 7 3700X, 8 cores / 16 logical processors; 34,267,746,304 bytes RAM |
| GPU / driver | NVIDIA GeForce RTX 2080 SUPER, 8,192 MiB; NVIDIA `617.42`, CIM `32.0.16.1742` |
| Displays | MSI MAG341CQ primary 3440×1440, 99 Hz, 100% scale; RDT272WX secondary 1920×1080, 100% scale; secondary refresh NOT_MEASURED |
| Power | AMD Ryzen Balanced, GUID `9897998c-92de-4669-853f-b7cd3ecb2790`; unchanged |
| Toolchain | Rust `1.98.1 (48a229cea 2026-09-01)`; Cargo `1.98.1 (797e8a9bc 2026-08-05)`; MSVC 2019 x64 build environment |
| GPUI | unchanged Zed revision `395def7a067b32ed598fcb3f048969ada9b54942` |
| Build / binary | `cargo build --locked --release -p cos-desktop`; optimized Release, 12,840,448 bytes; SHA-256 `1c57d85d1e7edbd064a14354ae53c848f6f22eaceac105641487b0eb64af8fc5` |
| Source / dirty state at build | baseline SHA above; documentation and app-core test-module changes only; Cargo.lock and production source unchanged |
| Fixture | `crates/cos-client/fixtures/task-184.json`, SHA-256 `0129b99e9a81231505bb0729f102e855e740c6af721b06bd92cbc069aa31b97a` |
| Tools | OMP computer screenshot/AX/native input; process CPU/private-memory sampling; existing PresentMon 2.6.0 where supported; public Taskern/Refine CLI descriptions |
| Background control | no clean-room isolation, OS/power changes, elevation or profiler installation; initial unrelated system load CPU 52%, GPU 61%, 225.55 W. Process-isolated samples do not establish whole-system power efficiency. |
| Earlier evidence | [Tern rendering analysis](TERN_BINARY_RENDERING_ANALYSIS.md) is historical, with NVIDIA driver `610.88`; not same-driver paired data and not re-reported as new measurements. |

Investigation assets and raw ledgers stay outside the repository in `%LOCALAPPDATA%/Temp/cos-finalization-20261008`; referenced hashes identify local retained evidence, not a durable public artifact service. Missing local files prevent replay of that ledger. No private content, credentials, giant traces or synthetic Runtime success is included.

## Quality gates and test correction

| Check | Result / exact scope |
|---|---|
| A-06 narrow regression | PASS: `cargo test --locked -p app-core local_disconnect_never_sends_stop_implicitly`, 1 passed / 3 filtered. Recording wrapper delegates to the existing connected MockCosClient; disconnect does not add a Stop send and does not mutate the retained canonical Task. No new production Test API or mock framework. |
| Format | PASS: `cargo fmt --all -- --check` |
| Metadata | PASS: `cargo metadata --locked --no-deps --format-version 1` |
| Dependency checker | PASS: `python -B scripts/check_dependency_rules.py --self-test`, then `python -B scripts/check_dependency_rules.py` |
| Clippy | PASS: `cargo clippy --locked --workspace --all-targets -- -D warnings` |
| Tests | PASS: `cargo test --locked --workspace` |
| Workspace build | PASS: `cargo build --locked --workspace` |
| Native measurement binary | PASS: `cargo build --locked --release -p cos-desktop` |

The unchanged `scripts/quality.ps1` ran its stop-on-failure gate sequence, followed by the Release build; the successful repaired run exited 0 (74.83 s wall time). An initial narrow test compile failed because the test imported `ResyncReason` from the wrong crate; only the import was corrected to `desktop_protocol`, then the test passed. The first aggregate run stopped at the Microsoft WindowsApps `python` alias (`No runtime installed that matches 3`); existing OMP Python `3.13.14` was selected by a **process-only** PATH prefix and all gates completed. No runtime install, global PATH/config change or failure-to-PASS skip occurred. A getter/constructor wording-echo test was removed rather than repinned; the meaningful disconnected Stop error boundary remains.

## Development integration evidence

[Refine / Taskern execution boundary](../integration/REFINE_TASKERN_EXECUTION_BOUNDARY.md) records exact current source SHAs, independently observed CLI/version axes, closed public fields, Caller ownership and RC-01–17 gaps. A required nonempty-context `check → compile → adapt` pure CLI chain succeeded against the existing Taskern binary, without dispatching Refine. No COS Runtime server contract is supplied by these products. Current-source binary provenance, actual Refine consumption and COS-product end-to-end compatibility remain unverified; source-shape agreement is not execution acceptance.

The subsequent concrete MP-COR1 derived packet also passed Taskern `check/compile/adapt`, with exact frozen contract/card context from the first review commit and no execution. The boundary report records its digests, corrected selection rejection and limits; it qualifies one representation, not all 62 generated packets, implemented COR1 behavior or Refine consumption.

## Fresh Windows startup, idle and interaction

Native control used live human-approved OMP `computer` helpers and was released after closing the measured windows. The Release fixture shell visibly showed Japanese title `新製品キービジュアル`, Task 184 / ACTIVE / REFINE / iteration 3 and explicit FIXTURE labelling. Default launch showed RUNTIME, `Disconnected — Runtime not configured` and no observed Task, with no automatic fixture fallback. Actual screenshots/AX observations, not Rust tests, established these limited native results.

Startup used one discarded warm-up and 30 **warm-cache fresh processes**, `Bun.spawn([binary, "--fixture"])`. The monotonic endpoint was `performance.now()` immediately before spawn to the first new-PID entry from `computer.windows()` (100 ms query interval plus bridge overhead). It measures observed window appearance, **not first useful frame or fixture-ready latency**. Shutdown spans immediately before native AX Close press to process exit. All 31 processes exited 0. Median averages the middle pair; p95 is nearest-rank; SD is sample SD. No meaningful p99 or reproducible cold-cache claim is made.

| Endpoint (ms) | n | mean | p50 | p95 | min | max | SD |
|---|---:|---:|---:|---:|---:|---:|---:|
| Window appearance | 30 | 459.804 | 422.577 | 529.208 | 418.515 | 564.759 | 50.781 |
| Native close → exit | 30 | 68.856 | 69.090 | 78.546 | 58.051 | 85.057 | 5.852 |

Idle sampling used the existing external `cos-render-probes-20261008/sample_cpu.ps1`: `-TargetPid 42220 -Seconds 60 -Output <state>.csv`. Each sample is a nominal one-second `Get-Process.TotalProcessorTime` delta divided by elapsed wall time and 16 logical processors. Values are percent of whole-machine CPU capacity, **not percent of one core**; the table's mean is the arithmetic sample mean. One interval per state is descriptive, not a repeated SLO or power qualification. Memory is sampled process working set/private bytes, not attributed allocations.

| State | samples / wall s | CPU mean % | CPU p50 % | CPU p95 % | CPU max % | CPU SD | WS high-water MiB | private high-water MiB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Foreground idle | 59 / 60.205 | 0.10374 | 0.09583 | 0.38306 | 0.47920 | 0.11440 | 64.254 | 88.145 |
| Background, Tern foreground | 60 / 60.988 | 0.10255 | 0.09604 | 0.19327 | 0.48238 | 0.11035 | 63.945 | 87.734 |
| Minimized | 60 / 60.994 | 0.09611 | 0.09577 | 0.28912 | 0.38452 | 0.10764 | 64.090 | 87.781 |

The minimized GPU process-counter spot had n=1 and a 23.66 s CIM query wall time: dedicated 26.078 MiB, shared 0.914 MiB, committed 48.738 MiB. A single 3D counter value of zero does not prove zero GPU work, low wakeups or power efficiency. CPU from that slow GPU-query script is not substituted for the separate idle ledger.

PresentMon 2.6.0 ran non-elevated against PID 42220 in unique 30 s foreground-idle and 20 s interaction sessions. Both commands exited 0 with an elevation warning; the foreground session produced no CSV, so zero idle Presents/wakeups is **NOT_MEASURED**. Interaction did produce 22 captured Present rows spanning 4,781.257 ms during 24 keyboard/Tab actions. `MsBetweenPresents` p95 230.716 ms follows deliberately spaced input and is not frame-pacing/input latency; `MsInPresentAPI` p95 0.146 ms and `MsUntilDisplayed` p95 17.246 ms are instrument endpoints, not event-to-photon. GPU-time attribution, continuous animation, frame starvation and input-to-photon remain NOT_MEASURED. No FPS/power claim is inferred.

| Native path | Observation / limit | Acceptance owner |
|---|---|---|
| Fixture/default/start/close | Observed expected fixture and honest disconnected surfaces; clean exit including 30 repeat starts | apps/desktop, RT8, REC3 |
| Keyboard/local selection | Root-focused Ctrl+2/Ctrl+3 changed surfaces; Tab/Shift+Tab exposed focus, Enter activated focused surface; candidate C changed Viewed without a Formal Outcome | NAV4, DOCK1, AX1/2 |
| Focus recovery | Focus moved to existing Tern then returned; Tab/Enter remained responsive | NAV4, AX1/2 |
| Shortcut limitation | Repeated Ctrl+1/2/3 with button focus did not consistently change the selected surface; Enter did. Complete global-shortcut/focus acceptance is not PASS | NAV4, AX1/2 future implementation |
| Resize/monitor move | Maximize/restore and move from primary to secondary 100%-scale monitor preserved Viewed C, focus and Japanese text | REC3, AX1/2 |
| Text/diagnostics | Japanese shell labels rendered; diagnostics toggle worked. Inspector content clipped at the small window, visible after maximizing; no typography/UX acceptance claim | TYP1/2, INS1, AX1/2 |
| Popup / IME / fullscreen | NOT_IMPLEMENTED: no popup, editable composition control or Artifact viewer to exercise | NAV5, AX1/2, IMG5 |
| Scroll/continuous projection/media/video | NOT_IMPLEMENTED for the requested product paths; static shell actions are not those workloads | PUB1/2, IMG*, FLOW*, VID* |
| Mixed DPI / assistive technology / high contrast / reduced motion / device loss | NOT_RUN: both monitors are 100%; no settings changed or declared release matrix exercised | AX1/2, REC1 |

Thirty fresh-process closes check repeat startup/shutdown, not a resident cache/GPU plateau. Existing fixture updates are not continuously pumped after shell creation; no product long-running update/recovery measurement is possible without prohibited implementation. Eight repeated snapshot replacements and release are measured separately below. Popup-open/closed idle comparison, IME composition and full native E2E remain explicitly unimplemented rather than skipped as PASS.

## Projection and existing image mechanisms

[Projection baseline](PROJECTION_MEASUREMENT_2026-10-08.md) owns exact corpus, public APIs, clock endpoints, raw/harness/source hashes and memory conditions. Validation p50/p95 grows from 0.762/0.945 ms (64 nodes, 1k edges) to 192.392/196.841 ms (five 1,024-node / 10k-edge graphs); n is 5–11 depending on corpus. Single-graph 50k-edge preflight is measured, but its 4,238,077-byte transport exceeds the existing 1 MiB frame limit and is NOT_MEASURED through the client. Legal transaction rewire succeeds; retained old handles preserve the old projection. Eight replacement memory samples show approximate private high-water 59.90 MiB without retained handles versus 128.57 MiB with eight handles, returning to 43.62 MiB after their release. They are unpaired allocator-inclusive observations, not exact allocations, leaks or optimized A/B proof. Archive files have CRLF while corresponding Git blobs use LF; listed normalized source hashes match the baseline, and actual measured archive hashes are retained.

An existing external `gpui-measurement-probe.exe` image mode rendered a generated 7680×4320 PNG visibly in a native window; 35 s resource and separate 10 s visual-proof runs exited 0. Its source uses the unchanged GPUI pin, but the retained executable was not freshly rebuilt, so source-to-binary qualification remains unverified. Its `first_render_ms` values 409/434 are shell render calls, **not first image-visible/Present** (those endpoints are null/unsupported). One image-process resource spot recorded WS 313.082 MiB, private 461.910 MiB, dedicated GPU 146.699 MiB, shared 127.539 MiB and committed 292.813 MiB; n=1, 18.58 s query wall time. This proves only an existing generic image mechanism is feasible; COS product upload/admission/fidelity/eviction is NOT_IMPLEMENTED.

Fresh decode probes used existing `<tool-path>/ffmpeg.exe`, generated PNGs, one discarded warm-up and five new processes per case: `-hide_banner -nostats -loglevel info -benchmark -i <input> [-vf scale=320:320:force_original_aspect_ratio=decrease] -frames:v 1 -f null NUL`. All 20 observed runs exited 0. FFmpeg `rtime` and monotonic spawn-to-exit wall time are different endpoints; nearest-rank p95 equals max at n=5, and p99 is not reported.

| Input / path | n | rtime mean ms | rtime p50 ms | rtime p95=max ms | rtime SD ms | wall p50 ms | wall p95 ms |
|---|---:|---:|---:|---:|---:|---:|---:|
| 7680×4320 full | 5 | 131.2 | 130 | 134 | 2.168 | 322.717 | 328.229 |
| 7680×4320 thumbnail 320 | 5 | 223.8 | 225 | 227 | 3.114 | 412.235 | 416.573 |
| 8192×8192 full | 5 | 261.0 | 260 | 264 | 2.449 | 601.325 | 603.989 |
| 8192×8192 thumbnail 320 | 5 | 393.8 | 395 | 398 | 4.764 | 742.720 | 756.417 |

Thumbnail scaling still decodes the PNG; unlike-quality outputs are not an equivalent-quality optimization A/B. Oversized COS admission is NOT_IMPLEMENTED. A new 16K generic decode/upload run was not attempted because the unbounded existing probe can allocate GiB and cannot test the required admission boundary; historical 16K data is not current evidence. No video/Runtime path was implemented to obtain measurements.

### Retained local evidence hashes

| Asset | SHA-256 |
|---|---|
| startup.json | `396895f2ca2648866b88ec1b02a8ff717abe12d89b5a403a2bd1cb429669a096` |
| idle-foreground.csv | `969cf9f1065b8af71aab0ec549bf17704e83fdd18616fce8364105fa7ada4d85` |
| idle-background.csv | `fb33d6d2a70d75a142f91871cf3d5d701f551f4cefe71301b346ae521d91f1de` |
| idle-minimized.csv | `3f6be9b4f21fd6e2f08effd2d3ebce2bd4e9b9baac11051ed4f1a7f607c7760b` |
| present-interaction.csv | `83c844ad78d3bb76b415ca8ca4a2be6eef7447a3848a7958f696455dbd9d79eb` |
| PresentMon 2.6.0 executable | `b2a706bc6ad475749e3b7e3409263aa1e6906d45bdcf993f6dbc0f660188f1af` |
| gpui-measurement-probe.exe | `2b86e13fee6f13dc0a8fc4584e8666aeb36a28199b7f187fc5deca13f0dacac6` |
| External image-probe source | `e4e17c7563cb1f9eeb3de017eadfa143bd93bfebe01a5913a1c15852354b97a8` |
| image-decode.cjs / image-decode.json | `2b9dd51d79a282af74f316d8ac30b16f97a38f2be1b7444179036cadeae234f1` / `10f201f638722b8bf5222648007d2bb31fbc6580ce511f1e562783b3e4958c3b` |
| FFmpeg executable | `81b5832f6c548d64feba0ec7d643397e2351d9f3460a65c6c296956045eebaea` |
| 7680×4320 / 8192×8192 PNG | `9b56b7df7ae9994daccee60a59a5f6d6c449a22b405f46800c11426946a46583` / `a5db4a02544ba20e81b9ce9f9928642a4e75fbb7ea775cb78c115bee4f0f6dd2` |

## Closure, ownership and decision traceability

The following PASS labels mean the **design/plan/test-policy finding is closed**, not that the future product path is implemented or accepted. Full native/Runtime acceptance remains open under its stated owner.

| Finding | Design closure / sole owner | Evidence / remaining implementation boundary |
|---|---|---|
| B-01 | PASS — Canon state ownership; Protocol authorization; DC-MP-PUB1 publication/lookup | Internal registry survives scope/epoch/replacement, but old-scope projection/receipt is withheld until Runtime validation. Ten scope scenarios frozen; DD-13/RC-03 pending; no new wire field |
| B-02 | PASS — [20 native requirements plus §53 windows](../architecture/DESKTOP_DESIGN_CONTRACTS.md#windows-native-requirements-crosswalk) | Existing tasks cover all named integration requirements; conditional/open Product decisions are explicit, not agent-deferred |
| B-03 | PASS — DC-MP-NAV4 A/B/C/D and Plan hard graph | Fixture/native completion independent of RT4; real receipt reflection and product completion remain conditional; all 62 hard prerequisites reconcile |
| A-01 | PASS — DC-MP-COR1/COR2; rendering validation decision | Shared correctness owner, legal rewires and atomic rejection; measured baseline; minimal ephemeral ordered membership index, no product algorithm change |
| A-02 | PASS — DC-MP-PUB1/NAV4; Runtime receipt authority | Disjoint 64+64/128 bounds, indefinite unknown blockage, no eviction/retry/fabricated settlement; lookup/reflection or approved manual intervention |
| A-03 | PASS — [media input admission](../architecture/DESKTOP_DESIGN_CONTRACTS.md#media-input-admission-and-integrity) | Local/fixture/no expected digest not Verified but bounded experiments allowed; external authorization/expected integrity retained; unique IMG1/2/6, SEC3, RT6 ownership |
| A-04 | PASS — owner documents first; Plan §§3/4/7/10/11/13/14/17 index only | Behavior/security/test/performance clauses resolve to the Canon/DC/Protocol/TESTING/rendering/decision registry, not independent Plan rules |
| A-05 | PASS — Canon approval axes | SPEC approval, implementation permission, implementation/test/integration/product acceptance separate; internal HOW inside contract requires no new DD |
| A-06 | PASS — existing app-core behavior regression | Connected recording seam proves no Stop send/no canonical mutation on local disconnect; narrow and full suites pass; no production seam added |
| A-07 | PASS — TESTING minimum native smoke contract | Actual startup/fixture/default/keyboard/focus/resize/monitor/close observations; absent popup/IME/viewer marked NOT_IMPLEMENTED, not Golden Workflow PASS |
| E-01 | PASS — Rendering §11.3 and TESTING impact routes | Changed-axis regression for internal changes; full declared supported matrix at integration/release; quality-equivalent performance comparisons |

| Trace / quality category | Owner and status |
|---|---|
| Product → DC → MP | Product semantics unchanged; 62 single DC contracts/backreferences and 62 task cards. Native §52–53 crosswalk is exact; general Product coverage remains grouped in Plan §15.3 with product acceptance ACC1/2 retaining unverified requirements |
| MP → Taskern → Refine | Boundary report's standard-field mapping applies to all 62 stable IDs; derived revision/digests/Binding are Caller-owned, no second editable master plan, invented TaskPacket fields or scheduler |
| Test contract → owner | TESTING single protocol/client/publication/presentation/native/application/external-integration owner table; no unit/integration count quota |
| Measurement → choice | Rendering §10.3 classifies FIXED / CONDITIONALLY_FIXED / PROPOSED / BLOCKED / REJECTED with fallback. Ordered graph validation and short Arc retention are constrained future choices, not measured optimized performance |
| Open DD/RC/R0/Video → gate | Canon decision registry + Runtime request register. Pending source/release/UX/Runtime decisions retain named owners; no implicit promotion from CLI success, fixture/native smoke or this review |
| Performance | Partially covered: startup window, idle process CPU/memory, graph/COW and PNG mechanisms measured; useful-frame/input latency, full frame/GPU/wake attribution and long-run plateau NOT_MEASURED (R0/PUB/IMG/REC) |
| Reliability | Covered by contracts; current default/close/local disconnect and snapshot behavior exercised. Real restart/scope/receipt/Task survival and GPU device-loss paths NOT_IMPLEMENTED or NOT_RUN (RT3/4/8, PUB1, REC1/3) |
| UX | Partially covered native keyboard/focus/Japanese/same-scale monitors; shortcut/clipping limits recorded. IME/popup/viewer NOT_IMPLEMENTED; mixed-DPI/AT/high-contrast/motion matrix NOT_RUN (NAV/IMG/TYP/AX) |
| Security | Covered by fail-closed scope/media/credential/redaction/update contracts; actual credential store, real authorized receipts/media and update signature path NOT_IMPLEMENTED (SEC1/2/3, RT4/6, REL2) |
| Distribution | Covered design with pending DD-21/22 choices; signed installer, portable release, update/rollback, uninstall, single-instance/deep-link/association product behavior NOT_IMPLEMENTED (REL*, RT8, NAV3) |
| Approved future scope | Product §53 detached Artifact/Compare/Flow windows only; no newly deferred MVP requirement |

The plan retains 62 task IDs and 18 milestones, with 41 hard edges instead of 86. The 45 removed graph relations remain decision/authorization, measurement, conditional Runtime/product, release or advisory gates/consumers; they are not deleted acceptance requirements. Primary readiness is 13 IMPLEMENTATION_READY, 15 DESIGN_GATE_REQUIRED, 9 EXPERIMENT_READY, 14 RUNTIME_BLOCKED, 3 UX_BLOCKED, 8 NOT_AUTHORIZED = 62. Independent tasks may run in parallel only under Caller permission with separated conflicting file scopes. Recommended first product task is COR1, then COR2; neither is started or authorized here.

## Structural verification and review boundary

The out-of-tree `audit-docs.cjs` SHA-256 `92863e9c6d694490228c6752674e133ffa8f439a0432b676a5b2c1de146c008f` completed with exit 0: 33 Markdown documents, 468 relative links/anchors, 62 MP cards, 62 DC contracts, exact per-card own-contract/reverse execution references, 62 DAG nodes / 41 hard edges, no cycle, exact incoming-edge prerequisites, and exact readiness card/table membership/counts. Errors and warnings were empty. `git diff --check` exited 0; Git reported only existing LF→CRLF working-copy conversion notices, not whitespace errors or Rust quality warnings.

Scope inspection found only README, the owning architecture/protocol/integration/plan/testing/evidence documents and the existing app-core test module. Product Canon, ADRs, Cargo manifests/lockfile and all production Rust source remain unchanged. The Plan normative audit relocated intervention/attention gap ownership to the Runtime request register and removed the private derivation-counter/key-layout oracle from PUB2 acceptance; task metadata/cross-references do not define independent product rules. New reports are the minimum separate measurement/provenance and public integration records requested, not persistent tools or a second editable plan.

This structural/local audit is not an external independent review PASS. The Tern GPT round below evaluates committed repository bytes and the current evidence; pending Product/UX, Runtime, R0, Video and release approvals remain implementation/product gates, not documentation defects or authorization granted by review.

### Tern independent review — round 1 and corrections

Tern GPT reviewed committed revision `cb540c2a438fb4b47f34654a5a09d7530857482f` in the existing private review conversation (identifier and link omitted). It confirmed direct attachment access, exact 1,034,454-byte bundle SHA-256 `d247f3b141d5e4de55f8ebef29eca24cf1469da20b06f8ec5000d295998a5f05`, and independently reconstructed 62 MP/DC/node cardinality, 41 acyclic hard edges, prerequisite/backreference/readiness correspondence and 20 Windows rows. It did not independently run Windows/Cargo or resolve all local links, inspect original raw ledgers, or verify private unpublished Git objects.

**Round-1 verdict: DESIGN HOLD — 1 BLOCKING, 3 ACTIONABLE NON-BLOCKING, 0 EXCESSIVE.** B-01–03, A-01–03/A-05–07 and E-01 passed design review; A-04 was partial because the Plan still required an optional counter. All four findings were accepted and minimally corrected:

| Review finding | Correction |
|---|---|
| BLK-01 M1 counter made mandatory despite COR2 | M1 now references COR1/COR2 observable acceptance/parity and same-condition 10k/50k evidence; internal counters explicitly optional |
| ACT-01 old docs-only scope read as current | Plan §1 table explicitly historical; current test-only correction/measurements remain in §1.1 |
| ACT-02 R0D incorrectly requested first | Plan risk route now requests R0A authorization, then R0B/C evidence, then R0D ratification; no DAG change |
| ACT-03 optional Product §54 fields absent from REC3 | REC3 distinguishes optional permission from its initial closed schema; keyboard bindings/recent locations/explicit Brief presets are not offered/persisted by this slice, retain existing UX/input/Brief owners and need canonical amendment/authorization before later admission; no Product change/new Store |

Parent integration also made TESTING authority explicit, removed the stale “current Runtime spot check” heading, and recorded the concrete MP-COR1 pure adaptation probe. These are document/evidence corrections only. The supplemented structural audit passed again with 469 relative links, unchanged 62/62 tasks/contracts and 41-edge graph/readiness, no errors/warnings; `git diff --check` passed. Executable source is unchanged since the successful narrow/full gates and Native runs; no redundant full native matrix or new product implementation was introduced.

### Tern independent review — round 2

Revision `e162bb1a56d06964194a05b75b814aadc50e788f` was reviewed using the corrected 387,581-byte bundle SHA-256 `1b917eb9c5f09b86126ac0a74f93e655b64798cd5d869d002ebec750232ee81c`. The reviewer verified its digest, reconstructed all 14 included raw source/ledger byte streams with matching individual hashes, and independently recomputed startup/close, idle CPU, graph validation and retained/dropped memory values consistent with the reports. It also reconstructed MP-COR1's 4,874-byte context/digest and the recorded check/compile/adapt identities/outcomes. This strengthens recorded-data verification, not independent Windows execution or Refine/COR1 acceptance.

All four round-1 findings were CLOSED. **Round-2 verdict: DESIGN HOLD — 1 BLOCKING (BLK-02), 0 ACTIONABLE, 0 EXCESSIVE.** The remaining pre-existing mismatch was MP-PUB2's mandatory “unit tests + counters” evidence despite DC-MP-PUB2's optional counters. The card now requires observable behavior/integration and candidate-insertion native evidence, with counters explicitly optional and no private-helper call-count oracle. No canonical behavior, DAG, test source or measurement changed for this correction; all original B/A/E findings except A-04 were again PASS.

### Tern independent review — round 3 (final)

Revision `e8262ab520a000de2a7bce7f676a2f162c09bda4` was reviewed using the 135,267-byte bundle SHA-256 `5e7a0e06834b3ec8282f986de41ccedec8550ba52b813c0c43539c09df77e508`; the reviewer matched its digest and the two-document change scope, re-derived 62 tasks/nodes with 41 acyclic hard edges, zero card↔DAG prerequisite mismatches and the 13/15/9/14/3/8 readiness distribution, and the corrected round-1/round-2 items remained in force. BLK-02 was CLOSED.

**Round-3 verdict: DESIGN READY — 0 BLOCKING, 0 ACTIONABLE NON-BLOCKING, 0 EXCESSIVE.** B-01–B-03, A-01–A-07 and E-01 all PASS. The reviewer explicitly did not re-run Cargo/native commands, did not verify unpublished Git object identity, and stated that this verdict is design/plan approval only, not authorization for product implementation, DD-01 or other decisions, R0 execution/ratification, real Runtime integration, Golden Workflow acceptance, Video work or product release, and no `dev`/`main` merge. That boundary matches this report; the remaining approvals stay with their owners.
