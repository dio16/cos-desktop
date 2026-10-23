# Process model

Status: **Accepted** target; bootstrap client lifecycle in **Observed** fixture implementation; real process integration **Deferred**. See [transport decision](../decisions/ADR-0005-LOCAL-TRANSPORT.md) and [survey](../integration/CURRENT_COS_RUNTIME_SURVEY.md).

Final Process 1 is COS Desktop/GPUI. Final Process 2 is independently managed cosd/COS Runtime. Runtime owns scheduling, workers, canonical persistence, recovery and projection publication. Closing/crashing/updating Desktop must not terminate real Tasks. This is a future integration requirement, not established by a fixture client or documentation. The current Python HTTP frontend/Rust workers are not evidence that the target Rust cosd exists.

*Desktop never opens Runtime DB/CAS/config or directly spawns internal Python from `<runtime-root>`. Runtime service startup, installation and authority remain outside Desktop bootstrap. A future development-only launcher requires a separate explicit task and must not become production behavior. Do not place Runtime in a Desktop-owned kill-on-close process/job group.*

## Client lifecycle

Implemented bootstrap boundary: `MockCosClient` connects/pumps on a background executor, publishes an immutable `ProjectionState` snapshot, distinguishes receipt acceptance from canonical effect, never auto-resends mutating commands after loss, preserves local presentation across updates, and keeps fixture mode explicitly labelled. `DesktopClient` is the object-safe boundary consumed by app-core; GPUI only renders state and forwards explicit user intents. No production transport exists, so this lifecycle is fixture proof, not real daemon/process separation evidence. This is not a fixed API that splits snapshot from subscription and introduces a race; use the [single observation session](DESKTOP_PROTOCOL.md). app-core orchestrates user intents through this boundary. GPUI sees presentation/connection state, never Named Pipe/HTTP details.

Closing a window cancels its local subscription/resource work, not the process-wide observation owner or unresolved registry. Final app shutdown closes transport and flushes permitted local preferences; it does not send StopTask implicitly. Explicit Stop is a separate intent, and its receipt cannot be treated as completion. Reconnect restores only after fresh authorization and never automatically replays commands. Disconnection is freshness/availability, not Task failure or outcome.

**SPEC_APPROVED MP-PUB1 target:** [DC-MP-PUB1](DESKTOP_DESIGN_CONTRACTS.md#dc-mp-pub1) owns one app-core background observation task; binary composition shares its handle with each window. Client decode/adapter detection and local auth/scope intent synchronously close the same visibility fence before queued processing; generation is not authorization. Windows receive coalesced version wakes and perform checked complete-publication/derived-cache reads without I/O. All disconnect intervals conservatively withhold until exact Runtime-issued reauthorization. Replacement transfers the same client/registry after joining the old task; only final app exit disposes it. These approved target behaviors are not the observed one-shot bootstrap implementation or real process-survival proof.

Current runtime polling can be used only as a clearly labelled transitional adapter with its weaker consistency documented. It cannot pass the Draft gap-free subscription acceptance test. Runtime restart must negotiate identity/epoch, reconcile unknown commands, then replay or replace the complete snapshot. Preserve local presentation state where referenced entities remain valid; stale/removed entities get explicit unavailable states.

## Thread and resource boundaries

Main thread: input, layout, paint, focus, small local presentation updates and checked publication reads. Background: owner pumping/authorized publication assembly/filtering/derivation, IPC, serialization, image/video decode, filesystem, network, search, thumbnail generation and graph layout. No blocking I/O or lock wait on UI thread. Cancel obsolete work; tag background results with publication generation/visibility/revision and reject old tags at install/use. Protocol queues and representation caches remain bounded. GPUI upload/resource lifetime obey platform thread constraints with expensive preparation outside them.

Rendering ownership, bounded work, demand-driven idle behavior and the GPUI shell/specialized-artifact strategy are Accepted in [ADR-0007](../decisions/ADR-0007-RENDERING-PERFORMANCE-VISUAL-ARCHITECTURE.md). Numeric targets and calibration belong to the [rendering companion §10](RENDERING_PERFORMANCE_VISUAL_ARCHITECTURE.md#10-proposed-performance-budget-and-calibration-plan), with actual Release/probe observations in the [evidence report](../development/TERN_BINARY_RENDERING_ANALYSIS.md#10-follow-up-measurements-and-ratification). No measured frame interval, sparse present gap or tiny fixture certifies large-history/media performance. Target misses require evidence and an implementation plan, not fabricated claims.

## Security and diagnostics

IPC is a trust boundary. Server validates every command, access scope and identity; UI disabled state does not enforce security. Credential plaintext storage is prohibited. Use OS/provider secure stores when implemented; redact credentials/tokens/signed media locators from logs, diagnostics, crash reports and projections. Never forward raw provider secrets. Treat external media and metadata as untrusted; metadata is not an executable command.

Separate Application, Client/Protocol, Runtime connection, Media and Diagnostics logs, with levels, bounded retention and correlation IDs. Do not stream all logs into ordinary UX. Future diagnostics exposes desktop version, protocol/schema versions, runtime version/identity, connection state, projection revisions, last update and bounded cache/subscription metrics. Runtime/provider logs remain runtime-owned and require explicit authorized access, not DB or file scraping.

## Evidence split

Bootstrap: native window/shell/navigation, explicit fixture projection, local state preservation, protocol/recovery simulation and quality baseline. Real integration: actual Task survives Desktop exit/crash, actual cosd restart/reconnect and receipt reconciliation, real server command validation, authentication/authorization, Media Service and real data compatibility. Each needs observed external behavior; neither compilation nor mock success proves them.
