# Protocol ownership

Status: **Accepted** ownership policy; machine-readable contract in **Observed** bootstrap implementation.

The sole machine-readable Desktop contract source is hand-maintained Rust wire types in Desktop's `crates/desktop-protocol`, with explicit serialization names, version behavior and compatibility tests. Any JSON Schema, OpenAPI fragment, TypeScript type or documentation table derived from them is a generated derivative labelled with source revision/version and never edited independently. No generator exists yet. [DESKTOP_PROTOCOL.md](../architecture/DESKTOP_PROTOCOL.md) owns the approved MP-PUB1 design target2.0/2.0 and otherwise remains a Draft; current Rust1.0 is Observed, not already migrated. STEP2 approves/documents the design but creates no Rust types or second machine-readable wire schema.

This rule concerns the new Desktop contract, not confiscation of the existing Runtime frontend contract. [The survey](CURRENT_COS_RUNTIME_SURVEY.md) records current frontend DTOs, checked-in OpenAPI and generated TypeScript. Preserve existing canonical identities, server validations and authorization. A Runtime-owned adapter maps current API to the agreed Desktop projection; do not copy existing schemas and edit them independently in Desktop. Existing frontend compatibility is a Runtime responsibility.

| Concern | Bootstrap owner | Change boundary |
|---|---|---|
| Contract definitions/version | Desktop desktop-protocol, future Rust types | Contract review and versioned release |
| Server/adapter, canonical projections | Runtime repository | Separate explicit Runtime task/commit |
| Client decode/recovery | Desktop cos-client | Compatible dependency update |
| Compatibility | Joint contract matrix, Desktop fixture tests then real cross-repo tests | Evidence for every supported pair |
| Migration | Explicit migration record with both release coordinates | No silent lockstep assumption |

Authorization boundary: cos-client retains the whole trusted internal registry, never a grant; Runtime chooses principal/read policy and receipt authorization. Historical MP-PUB1 owner approval (see [disclosure](../development/evidence/README.md#historical-records)) ratifies the scoped DD-12/13/15/19 handoff in [Protocol](../architecture/DESKTOP_PROTOCOL.md#authorized-read-handoff): authenticated binding, explicit per-ID decisions and synchronous fail-close invalidation. [DC-MP-PUB1](../architecture/DESKTOP_DESIGN_CONTRACTS.md#dc-mp-pub1) owns app-core's complete gated publication and local lifecycle; it conservatively withholds through disconnect until fresh exact authorization. This is contract approval only; actual Runtime identity/policy/server/retention and real compatibility remain separately implemented and NOT_RUN. No Runtime repository mutation or fake authenticated success follows.

## Transfer procedure

The final canonical owner may move to `<runtime-root>` or a dedicated shared protocol repository; that destination is not decided here. Preconditions: owner assigned, license/distribution policy approved, reproducible versioned artifact or fixed-revision consumption available, and compatibility/migration plan reviewed.

1. Freeze one contract revision; record content digest and supported protocol/schema range.
2. Publish that exact source through the selected owner and versioned package/revision. Consumers must pin a released version or immutable revision, not floating branches or local absolute paths.
3. Update server and client consumption separately with compatibility tests; old and new product releases may coexist through supported protocol ranges.
4. Retire the old handwritten source at cutover and replace its consumption with the package/reference. Do not keep two editable Rust/schema definitions. If transferring desktop-protocol itself, remove obsolete local definitions rather than retaining compatibility aliases.
5. Generated outputs remain reproducible derivatives with source/version provenance; mismatch fails checks. Record rollback to a known artifact, not independent schema forks.

Contract, server, client, compatibility and migration changes must be named separately even when coordinated. Desktop bootstrap must not include Runtime mutation in the same commit or work scope. See [local workflow](LOCAL_DEVELOPMENT_TOPOLOGY.md).
