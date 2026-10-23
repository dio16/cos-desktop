# ADR-0004: Versioned projection and client boundary

Status: **Accepted** ownership/architecture; interaction details **Proposed Draft**, 2026-09-21.

Desktop consumes projections and sends explicit commands through app-core → cos-client. DTO imports may directly reference desktop-protocol as permitted by [dependency rules](../architecture/DEPENDENCY_RULES.md). cos-client is GPUI/UI-model/Domain-crate independent. Adding it is transport isolation, not transfer of semantic authority.

Choose hand-maintained Rust wire types in the future Desktop desktop-protocol crate as bootstrap's sole machine-readable source; schemas/types generated from it are derivatives. [Protocol ownership](../integration/PROTOCOL_OWNERSHIP.md) specifies future transfer with one source at all times. Existing Runtime frontend APIs remain authoritative current interfaces and are mapped, not ignored or independently copied.


Bootstrap implementation: `crates/desktop-protocol` is the sole machine-readable contract source; `cos-client` owns atomic staging/recovery and honest receipts behind object-safe `DesktopClient`; app-core orchestrates through that boundary and GPUI only renders explicit intents. Fixture `task-184-refine` tests negotiation, snapshot/delta staging, duplicate/gap/stale/overflow/epoch/unknown-event/authorization recovery and acceptance-vs-effect command handling. No real server guarantees are claimed.
