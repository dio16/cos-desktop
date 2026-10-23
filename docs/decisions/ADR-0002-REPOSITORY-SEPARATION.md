# ADR-0002: Independent repositories

Status: **Accepted**, 2026-09-21.

Runtime owns COS/REOS authority and server implementation; Desktop owns native presentation/client lifecycle. [Repository architecture](../architecture/REPOSITORY_ARCHITECTURE.md) is the complete allocation. Canon workspace diagrams are logical, not a physical monorepo mandate.

Choose independent Desktop and COS Runtime repositories. Never nest Desktop under Runtime, use a Runtime submodule, merge histories/releases or import internal crates/DB/config. Runtime inspection does not authorize Runtime mutation. Rejected: monorepo convenience, path dependencies and launching checkout-internal Python, because they couple release and authority boundaries.


Bootstrap implementation: `<desktop-root>` is an independent Git repository; history and remotes were preserved during bootstrap. COS Runtime (`<runtime-root>`) was inspected without mutation, build, test, process launch, DB/config read, submodule, path dependency or hardcoded checkout launch. License/rights holder are decided for this repository only (Apache-2.0, Copyright 2026 dio); publication status is external and not implied here.
