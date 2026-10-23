# Bootstrap implementation plan

Status: **Observed** completed sequence in this bootstrap task. This plan remains the stage order; evidence for each stage is in [BOOTSTRAP_REPORT.md](BOOTSTRAP_REPORT.md).

## Entry decisions and scope

Read [preparation report](DOCUMENTATION_PREPARATION_REPORT.md), [canon](../architecture/COS_DESKTOP_PRODUCT_UX_ARCHITECTURE.md), [GPUI ADR](../decisions/ADR-0001-GPUI.md), [runtime survey](../integration/CURRENT_COS_RUNTIME_SURVEY.md) and [protocol](../architecture/DESKTOP_PROTOCOL.md). License/rights holder were decided during this implementation (Apache-2.0/Copyright 2026 dio); GitHub visibility/creation/initial push and administrator-requiring environment change remain user-gated. No LICENSE is missing now; the plan retains its original historical claim that no GitHub repository was created at that point. Stages 6–10 below are observed in this task; the full ten-stage table in version history remains the governing order and is not repeated here.

No full Creative Brief; production Artifact viewer; production video playback; complete DMLG renderer; full Compare UX; real evaluator integration; real cosd; production transport integration; full Media Service; installer production pipeline; updater production system; complete UX; complex graph editor. No successful stubs to fill these omissions. Unsupported features must be hidden/disabled with accurate reason or explicitly unavailable, not return fake success.

## Required quality evidence

See [TESTING.md](TESTING.md) and [BUILD_AND_RUN.md](BUILD_AND_RUN.md). Exact planned checks after bootstrap artifacts exist:

```text
cargo fmt --all -- --check
cargo metadata --locked --no-deps --format-version 1
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked --workspace
```

Also actual native startup smoke and dependency allowlist verification. CI build success does not prove visible native startup. Do not claim performance targets or real runtime tests based on fixture tests.
