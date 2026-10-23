# ADR-0002: Independent repositories

Status: **Accepted**, 2026-09-21.

Runtime owns COS/REOS authority and server implementation; Desktop owns native presentation/client lifecycle. [Repository architecture](../architecture/REPOSITORY_ARCHITECTURE.md) is the complete allocation. Canon workspace diagrams are logical, not a physical monorepo mandate.

Choose an independent Desktop repository at `<desktop-root>`; never nest it under `<runtime-root>`, use a Runtime submodule, merge histories/releases or import internal crates/DB/config. Runtime inspection does not authorize Runtime mutation. Rejected: monorepo convenience, path dependencies and launching checkout-internal Python, because they couple release and authority boundaries.


Bootstrap implementation: `<desktop-root>` is an independent Git repository (no commits before final bootstrap commits; history/remotes preserved by not altering anything else). Runtime at `<runtime-root>` remains `dev`, HEAD `f94985bd4c5a3a0714153f9d068f24439d57a1f2`, clean including untracked, origin unchanged; containment verified both directions. No Runtime mutation, build, test, process launch, DB/config read, submodule, path dependency or hardcoded checkout launch. License/rights holder are decided for this repository only (Apache-2.0, Copyright 2026). Remote creation/visibility/push status is recorded in separate dated history; current visibility is not established by this historical bootstrap note.
