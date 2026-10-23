# COS Client Projection Measurement — 2026-10-08

**Status:** Observed temporary, out-of-worktree public `cos-client` measurements against baseline `b4342f30a3aeda1d97f833cf97b58f7a4013957d` source content (the extracted text uses CRLF; see byte provenance below). This is a baseline record, not an in-product R0 harness, an R0 implementation or ratification, an accepted performance gate, or a performance win. No product algorithm, product dependency, Runtime, Refine or Taskern code was changed for this work.
This A01 evidence record follows the relevant §11.2 result categories—endpoints, latency/resource metrics, instrumentation limits, sample loss, evidence hashes and explicit unavailable statuses. It is not the future R0 JSON schema or a general benchmark tool.

## Scope and source path

The external harness exercised `FixtureScenario::validate`, `MockCosClient` connect/pump and scripted/injected delta APIs. It used the complete `task-184.json` fixture from the baseline archive, replacing its graph projections with deterministic synthetic graph projections. It does not exercise a real Runtime transport, app-core publication, UI derivation, GPUI rendering, native Present, image/video work or a production graph surface. Existing COR-01 reproduced-defect cases remain the plan's ground truth and were **not rerun**; the only new semantic observation here is a legal graph rewire.

The baseline was extracted outside the worktree and used as the harness's path dependency. The archived text has CRLF although the Git blobs have LF; the pipeline was observed, but the conversion cause is not established. Parent integration compared all seven listed baseline files after CRLF→LF normalization with `git show <baseline>:<path>`: all matched; the parsed fixture JSON also matched. Listed archive hashes identify the **actual measured bytes**, not Git-blob hashes. For example the fixture archive hash is `efba9bc6…`, whereas its Git-blob/checkout hash is `0129b99e9a81231505bb0729f102e855e740c6af721b06bd92cbc069aa31b97a`. No exact-byte archive identity is claimed.

```text
<workspace-root>/bench/projection-measurement-2026-10-08/
├── Cargo.toml
├── Cargo.lock
├── src/main.rs
├── scripts/sample-memory.ps1
├── baseline/                         # git archive of b4342f30…
└── results/
    ├── bench-raw.csv
    ├── memory-dropped.process-memory.csv
    ├── memory-retained.process-memory.csv
    └── memory mode stdout/stderr
```

Baseline identity/source checks and extraction commands:

```text
git rev-parse b4342f30a3aeda1d97f833cf97b58f7a4013957d
# b4342f30a3aeda1d97f833cf97b58f7a4013957d

git diff --quiet b4342f30a3aeda1d97f833cf97b58f7a4013957d -- crates/cos-client crates/desktop-protocol Cargo.toml Cargo.lock
# exit 0: the measured crates and their workspace lock/manifest match the named baseline

git archive b4342f30a3aeda1d97f833cf97b58f7a4013957d | tar -xf - -C <workspace-root>/bench/projection-measurement-2026-10-08/baseline
```

To recompute the listed file digests from the harness directory:

```powershell
powershell -NoProfile -Command 'Get-FileHash -Algorithm SHA256 -Path Cargo.toml,Cargo.lock,src/main.rs,scripts/sample-memory.ps1,results/bench-raw.csv,results/memory-dropped.process-memory.csv,results/memory-retained.process-memory.csv,results/memory-dropped.stdout.txt,results/memory-retained.stdout.txt,baseline/Cargo.lock,baseline/crates/cos-client/src/client.rs,baseline/crates/cos-client/src/projection.rs,baseline/crates/cos-client/src/mock.rs,baseline/crates/cos-client/src/fixture.rs,baseline/crates/cos-client/fixtures/task-184.json,baseline/crates/desktop-protocol/src/projections.rs | Format-Table -Property Hash,Path -HideTableHeaders'
```

Relevant measured archive-file SHA-256 values (CRLF text; not Git-blob digests):

| File | SHA-256 |
|---|---|
| `baseline/Cargo.lock` | `07f64b7878d61f1d87db8e396c94120d42553bf21d32dc02f440d36edda931d4` |
| `baseline/crates/cos-client/src/client.rs` | `e702a01f2b1676279d3c2c7660776d20ff12021bf720ac4d436c492c1e9899ed` |
| `baseline/crates/cos-client/src/projection.rs` | `31a80a77c961bc3e8644f8d78e0f76539949ae445f23f366c80c1288b545f0e4` |
| `baseline/crates/cos-client/src/mock.rs` | `0e5bde9d64b66a0320db5ecf3d4287667e2c0fd24bc845e59e1740e5155fb9ea` |
| `baseline/crates/cos-client/src/fixture.rs` | `765357e3780a225328b81392a727b5bf1408e45457049d4b3b4f4384980df53a` |
| `baseline/crates/cos-client/fixtures/task-184.json` | `efba9bc6a6a751c661fd1ba69f749bcaa6c396a023652e5e2db265409cbce622` |
| `baseline/crates/desktop-protocol/src/projections.rs` | `8bfec0d41efc57cf9b050acfec0b3b1e325d1a29b7dd96cf9e5df154f2a0ee1a` |

Harness/artifact hashes:

| File | SHA-256 |
|---|---|
| `Cargo.toml` | `0f3f229a23f0135554e6b9ebfd4626bd15563faef0f8a0fe26faa8e2fbf25c8f` |
| `Cargo.lock` (external harness) | `bd16ef86424b8918d0ef5bf9bb471e0b810aeb667507eee31acd7572bfcbb68a` |
| `src/main.rs` | `0030efc64d2da2978ba1d123ad13f08aebfbf457a42460bbd04178166d4641d7` |
| `scripts/sample-memory.ps1` | `479767afc956bfdb840b9dc99b6f3d2eefd52d96e6b55be1fe5e158d980338f6` |
| `results/bench-raw.csv` | `3d28c5dc1da002fe43dd4f07aa7afb141ca57a83ce96bde256644dcb8f39e891` |
| `results/memory-dropped.process-memory.csv` | `53800d0dd8845e1ca4cc4690c4681763152dece61974b9f08c4180acd244803d` |
| `results/memory-retained.process-memory.csv` | `7b58411718ad57311083749e41adbb7831ea170ad9f5b0cf69f87b19042f1ef3` |
| `results/memory-dropped.stdout.txt` | `f3f3a6dde9087e86e8e2ede2d875c76208a01c360ec4cf86141da31bf9b55549` |
| `results/memory-retained.stdout.txt` | `d34414eaaeb5b8e7134cf8441db9b7bfe725b2820b4636bea21e67fd39708f64` |

## Environment and reproducibility

| Item | Observed for this measurement session |
|---|---|
| OS | Windows 11 Pro, build `10.0.26300` |
| CPU / RAM | AMD Ryzen 7 3700X, 8 cores / 16 logical CPUs; `34,267,746,304` bytes reported RAM |
| GPU / driver | NVIDIA GeForce RTX 2080 SUPER, 8,192 MiB; driver `617.42` (CIM `32.0.16.1742`) |
| Displays | 3440×1440 primary at approximately 99 Hz and 1920×1080 secondary; both scale factor 1 |
| Power policy | AMD Ryzen Balanced, GUID `9897998c-92de-4669-853f-b7cd3ecb2790` |
| Contention | Uncontrolled; session environment capture showed 52% CPU, 61% GPU and 225.55 W GPU power. These are system readings, not harness attribution. No frequency/thermal/affinity lock. |
| Rust | `rustc 1.98.1 (48a229cea 2026-09-01)`, `cargo 1.98.1 (797e8a9bc 2026-08-05)`, `x86_64-pc-windows-msvc` |
| Build/profile | External `cargo build --release --offline --manifest-path Cargo.toml`; Cargo reported `release` profile `[optimized]`. Target artifacts remained under the external harness directory. |
| Harness direct deps | Baseline path `cos-client 0.1.0`, baseline path `desktop-protocol 0.1.0`, `serde_json = 1.0.151`; their locked dependencies include `serde 1.0.229`, `sha2 0.10.9`, `thiserror 2.0.20`. |
| Lock provenance | External lock started as a byte copy of the baseline `Cargo.lock`; Cargo pruned workspace-unneeded entries and added only the `projection-measurement 0.1.0` root package. Parsed lock comparison found no package-version mismatches for overlapping name/source pairs. The harness added no repository dependency. |

Commands run from the external harness directory:

```text
cp baseline/Cargo.lock Cargo.lock
cargo build --release --offline --manifest-path Cargo.toml
cargo run --release --offline --manifest-path Cargo.toml -- bench > results/bench-raw.csv
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/sample-memory.ps1 -Mode memory-dropped
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/sample-memory.ps1 -Mode memory-retained
```

The external Release build completed; the measurement run emitted a complete CSV ledger and all correctness assertions passed. The two memory processes exited 0. This work did not run repository build/lint/test/format gates; those remain with the integrating agent.

## Workloads and endpoint definitions

Synthetic graph nodes/edges use stable fixture IDs, short labels, no optional layer/kind payloads and deterministic endpoints: `from = edge_index % V`, `to = (edge_index × 7919 + 1) % V`. Each graph is one `GraphSnapshot`; graph projections in the 50k aggregate are chunked one entity per frame. The original Task-184 fixture's other valid projections remain in the state. The custom graphs are synthetic probes, not production-graph distributions.

| Corpus | Graphs | Nodes | Edges | Max serialized Graph `SnapshotChunk` |
|---|---:|---:|---:|---:|
| Small | 1 | 64 | 1,000 | 83,474 B |
| 10k, smaller V | 1 | 256 | 10,000 | 834,854 B |
| 10k, larger V | 1 | 1,024 | 10,000 | 884,784 B |
| 50k aggregate | 5 | 5 × 1,024 | 50,000 total (10,000 each) | 884,784 B per graph chunk |
| 50k single graph | 1 | 1,024 | 50,000 | 4,238,077 B |

The 50k aggregate permits the public client snapshot path to run without changing protocol bounds: each graph is a separate chunk below the default 1 MiB control-frame cap and the aggregate graph frames fit the default 8 MiB bundle ceiling. The one-graph 50k serialized chunk is 4,238,077 B, so its complete client snapshot/delta transport path is `NOT_MEASURED` under the default limit. No limit was raised. Its `FixtureScenario::validate()` preflight path was measured separately without a wire frame; that result is not a transport acceptance result.

| Endpoint | Timed interval | Preparation intentionally excluded |
|---|---|---|
| `fixture_scenario_validate` | Public `FixtureScenario::validate()`; this calls `ProjectionState::from_snapshot` and integrity validation. | Graph/fixture construction and serialization. No protocol frame is involved. |
| `client_snapshot_roundtrip` | `MockCosClient::connect()` plus `pump()` through `SnapshotApplied`/Live, including mock snapshot generation, framing, client parse, counts/digest checks and client `ProjectionState::from_snapshot`. | Scenario construction, `MockCosClient::from_scenario()` validation/runtime setup. |
| `fixture_delta_end_to_end` | Public `MockTransport::deliver_step()` plus `pump()` through `DeltaApplied`; includes fixture-runtime base-revision check/apply, digest/frame generation and client decode/transaction validation/apply. | Scenario/client initialization and scripted rewire construction. |
| `client_delta_pump_*` | `pump()` only after a pre-encoded valid server `DeltaTransaction` was placed into `MockTransport` via `inject_raw()`. | Graph/delta construction, digest/JSON serialization and queue injection. The retained/dropped handle condition is the only intended difference. |

All latency endpoints were warm-only; each timed endpoint/corpus had one untimed warmup. The two memory captures were separate single-process runs with no cold-cache claim. Timings are `std::time::Instant` wall durations in Release; raw nanoseconds are retained. Summary uses nearest-rank p50 and p95, population standard deviation, min and max. Sample counts are 3–11; p95 is therefore the observed maximum in these small samples, not a qualified tail estimate. No p99 is reported. There was no paired baseline-vs-variant implementation A/B.

## Observed latency results

All latency values below are milliseconds. `p95` is nearest-rank, and in the reported small `n` it equals the sample maximum. Standard deviation is population SD. The complete per-sample ledger is `results/bench-raw.csv` (SHA-256 above).

### Snapshot validation and client snapshot endpoint

| Corpus | n | `FixtureScenario::validate` p50 / p95 | min–max / SD | `connect → SnapshotApplied` p50 / p95 | min–max / SD |
|---|---:|---:|---:|---:|---:|
| 1 graph, V=64, E=1k | 11 / 7 | 0.762 / 0.945 | 0.698–0.945 / 0.069 | 5.154 / 5.229 | 4.991–5.229 / 0.072 |
| 1 graph, V=256, E=10k | 7 / 5 | 10.817 / 11.737 | 10.215–11.737 / 0.480 | 54.854 / 58.516 | 52.724–58.516 / 2.092 |
| 1 graph, V=1,024, E=10k | 7 / 5 | 37.716 / 38.491 | 36.514–38.491 / 0.583 | 83.852 / 87.689 | 79.791–87.689 / 2.923 |
| 5 graphs, each V=1,024/E=10k; total E=50k | 5 / 5 | 192.392 / 196.841 | 187.564–196.841 / 3.467 | 394.192 / 408.927 | 388.779–408.927 / 7.278 |
| 1 graph, V=1,024/E=50k, preflight only | 3 / — | 200.823 / 201.595 | 199.588–201.595 / 0.827 | **NOT_MEASURED** | 4,238,077 B graph chunk exceeds default 1 MiB frame |

Each pair of `n` values is `validation / snapshot`. No snapshot-roundtrip value is inferred for the single-graph 50k case.

### Legal graph rewire delta

Each delta replaces one complete graph projection with a valid revision-2 graph: edge 0's target is rewired to a distinct existing node (not a self-loop), projection revision advances 1→2, `GraphRef.version` and the fixture digest change, and the transaction cursor advances. All samples validated the transaction digest/base revision/references and observed `DeltaApplied`.

| Corpus | Endpoint / handle condition | n | p50 / p95 | min–max / SD | Delta frame bytes |
|---|---|---:|---:|---:|---:|
| 1 graph, V=64/E=1k | Fixture commit/serialize + client pump | 7 | 3.911 / 4.556 | 3.578–4.556 / 0.340 | 83,704 |
|  | Client pump only; no held snapshot handle | 7 | 2.181 / 2.487 | 1.968–2.487 / 0.187 | 83,702 |
|  | Client pump only; old snapshot handle retained through pump | 7 | 2.176 / 2.565 | 2.073–2.565 / 0.169 | 83,702 |
| 1 graph, V=256/E=10k | Fixture commit/serialize + client pump | 5 | 49.679 / 50.519 | 48.902–50.519 / 0.539 | 835,084 |
|  | Client pump only; no held snapshot handle | 5 | 27.468 / 29.908 | 26.117–29.908 / 1.264 | 835,082 |
|  | Client pump only; old snapshot handle retained through pump | 5 | 28.058 / 28.635 | 26.845–28.635 / 0.586 | 835,082 |
| 5 graphs, each V=1,024/E=10k; total E=50k | Fixture commit/serialize + client pump | 5 | 77.500 / 84.688 | 73.486–84.688 / 3.637 | 885,014 |
|  | Client pump only; no held snapshot handle | 5 | 57.195 / 75.607 | 55.061–75.607 / 7.598 | 885,012 |
|  | Client pump only; old snapshot handle retained through pump | 5 | 62.095 / 62.685 | 61.672–62.685 / 0.339 | 885,012 |

The two client-pump handle conditions are independent small samples, not paired runs. The measured retained-vs-dropped difference is not an A/B performance win or a stable percentage estimate. `inject_raw` and JSON encoding are outside the pump-only timer; both are included in neither pump-only result. The fixture end-to-end row is a separate combined endpoint and must not be subtracted from pump-only as a component estimate.

### Repeated snapshot replacement and process memory

The external sampler captured one process per condition while each made eight full `HistoryUnavailable` resync/snapshot replacements on the 50k aggregate corpus. The dropped condition retained no snapshot handles between replacements. The retained condition kept each prior `SnapshotHandle` until all eight replacements completed, then cleared them and waited 2 seconds before exit. Process memory includes Rust allocator/runtime/fixture/protocol buffers; it is not an allocation ledger or exact live snapshot-byte count.

| Condition | Complete process-memory rows | Actual median sample interval (target 50 ms) | Peak working set | Peak private bytes | Final working set / private bytes after release/done |
|---|---:|---:|---:|---:|---:|
| Drop each old handle | 101 | 74.8 ms | 61.78 MiB | 59.90 MiB | 43.39 / 40.92 MiB |
| Retain 8 prior handles, then clear | 104 | 73.3 ms | 128.48 MiB | 128.57 MiB | 42.84 / 43.62 MiB |

These are single-run sampled high-water observations; the polling interval was slower than its 50 ms target and can miss brief peaks. The retained run's higher peak is consistent with intentionally keeping eight detached prior states alive, but process counters cannot attribute exact bytes to snapshots. After clearing handles, process allocator/working-set readings need not return to startup values. This is not evidence of a leak or a plateau guarantee. GPU/device counters are not applicable to this non-GPUI process and were not measured.
The dropped capture had 102 sampler attempts in the script summary, but its last CSV row (after process exit) had empty memory counters; only the 101 complete rows above are used. The retained capture had 104 complete rows. The attempted empty row is preserved in the raw CSV and was excluded from every aggregate. Allocator-allocation counts/bytes were not collected (`NOT_MEASURED`); no global allocator instrumentation was installed. `Instant` clock resolution and instrumentation-on/off overhead were not separately characterized.

## Source-grounded interpretation and limits

**Source observed:** `SnapshotHandle` is `Arc<ProjectionState>` and `snapshot()` returns `Arc::clone` (`client.rs:29–30, 381–384`). Delta apply runs `Arc::make_mut` only after transaction duplicate/revision/reference checks (`client.rs:1279–1318`); a retained strong handle therefore selects the clone-on-write branch. `ProjectionState` derives `Clone` over its `BTreeMap` fields and owns the graph `Vec`s (`projection.rs:117–139`), so a retained current handle can cause a whole-state clone, including graph payloads. The handle accessor itself is a shared clone, not a state copy. The pointer/old-revision assertions observed same Arc allocation after a dropped-handle update, detached allocation when retained, old revision 1 unchanged, and current revision 2 published.

**Source observed:** snapshot integrity validates graph node uniqueness, then uses a linear `graph.nodes.iter().any(...)` scan separately for each edge endpoint (`projection.rs:355–379`). Delta graph-reference validation repeats endpoint scans (`projection.rs:614–629`). This makes the endpoint membership portion O(E·V), in addition to node uniqueness sorting. The measured 10k-edge validation p50 increased from 10.817 ms at V=256 to 37.716 ms at V=1,024; a five-graph 50k-edge aggregate measured 192.392 ms. These observations are consistent with the source path, but do not isolate its CPU instructions from fixture setup/other snapshot validation. No allocation counter was available in this safe harness, so allocation count/bytes and exact COW-clone bytes are **NOT_MEASURED**.

**Transaction/atomicity observation:** this probe sent one legal full-graph upsert, then checked the retained old handle remained at the complete old graph/revision while the client exposed the complete new graph/revision and advanced cursor after `DeltaApplied`. It did not introduce/replay any known defect case, inspect state concurrently during synchronous `pump()`, test concurrent subscribers, or verify a semantic graph digest against an external Domain authority. The graph digest was synthetic fixture content; the transaction integrity digest was computed over the wire mutation using the baseline public protocol API. This proves only the observed client-path behavior for this legal fixture rewire.

**Unavailable or deliberately not claimed:** allocation counts/bytes (`NOT_MEASURED`); one-graph 50k client transport under its default frame cap (`NOT_MEASURED`); real Runtime/Refine/Taskern behavior; `app-core` publication; UI derivation; 100k entities; cold-cache or cold-start behavior; GPU/VRAM/frame/Present timing; GPUI/Tern A/B; and production corpus distributions. Existing Tern §10.5 same-pin GPUI results establish separate rendering mechanisms only; they are not comparable client validation timings or a validation A/B ([TERN_BINARY_RENDERING_ANALYSIS.md](TERN_BINARY_RENDERING_ANALYSIS.md)).

## Minimal future choice (proposed, not implemented)

1. Keep one projection-integrity owner for both complete snapshots and the effective candidate state of a delta transaction; build/validate the candidate fully before atomic publication. Reuse that owner's rules rather than adding a second validation service or snapshot/delta algorithm.
2. Treat the plan's COR-01 findings as correctness-first and preserve their required regression/reproduction evidence; they were not retested here. Only then address COR-02/PERF-01 scaling.
3. If a graph membership lookup is still needed after correctness work, the smallest candidate is one ephemeral `BTreeSet<NodeId>` built per graph validation and reused for both endpoint membership checks. This adds no persistent index, persistent-collection dependency, service, crate or protocol/API change. Its effect remains **NOT_MEASURED** until a behavior-parity before/after variant is actually exercised.
4. Keep §10/§11.2 thresholds as proposed until separately authorized R0 evidence and approval. This temporary baseline does not ratify thresholds, close R0, or establish a performance win.
