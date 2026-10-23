# ADR-0005: First local transport candidate

Status: **Accepted** candidate direction; security/stream adapter design **Proposed** and unimplemented. 2026-09-21.

## Decision

Use loopback HTTP for the first real Windows-local integration candidate, reusing the existing Runtime HTTP boundary through a Runtime-owned Desktop adapter. Use a task-scoped resumable HTTP event stream (SSE candidate) for observation once the required guarantees exist. This is not adoption of the current generic `/api/events` stream as the Desktop protocol. Bootstrap has no real transport.

Observed current HTTP routes include COS catalog/tasks/candidates/evaluations/history/details/commands/stop/media. The current generic SSE is in-process, unscoped and lacks replay cursors. Current reads trust loopback rather than authenticating each read. Both are gaps, not acceptable production claims. See [survey](../integration/CURRENT_COS_RUNTIME_SURVEY.md) and [required changes](../integration/COS_RUNTIME_REQUIRED_CHANGES.md).

## Evaluation

| Criterion | HTTP candidate / required work |
|---|---|
| Current boundary fit | Reuses authoritative frontend validations, command receipts and media; avoids new Domain implementation |
| Windows implementation/distribution | Existing server boundary, standard HTTP clients; no Named Pipe server implementation required initially, but independent service packaging remains unverified |
| Server identity | Loopback hostname/port is not identity. Discovery must bind endpoint to authenticated installation/server identity; fail on mismatch. Port hijacking/rebinding must be considered |
| Access control | Require explicit per-user pairing and scoped credentials or equivalent authenticated channel; current unauthenticated reads must be addressed. Same-machine is not same-user. A token only proves client authorization, not server identity |
| Discovery | Explicit development endpoint via COS_DESKTOP_RUNTIME_ENDPOINT; production discovery/pairing record is user-scoped and integrity/access protected. No scanning ports, Runtime DB reading or fixed checkout path |
| Framing/limits | HTTP Content-Length/chunked bounds plus protocol frame/bundle limits; SSE framing must limit a complete event before JSON allocation. Reject oversized bodies, decompression expansion and redirects |
| Cancellation/timeouts | Cancel observation/request handles; command timeout becomes ResultUnknown. Proposed 5 s connection, 15 s command receipt timeout, 30 s heartbeat/liveness threshold; tune with evidence, not implemented guarantees |
| Reconnect | Session negotiation, epoch/scope-bound cursor replay or full snapshot. Existing SSE cannot satisfy this without changes |
| Remote evolution | Replace endpoint/auth adapter with authenticated TLS transport and remote policy; retain DTO/command/observation semantics and UI model. Never expose today's unauthenticated loopback API remotely |

## Alternatives

- Windows Named Pipe: strong same-user ACL/peer-identity options and no TCP port; requires new server adapter, discovery, byte framing, cancellation and distribution proof. Retain as fallback if secure HTTP pairing cannot be established; do not claim implemented Windows peer verification.
- Unix Domain Socket: useful future Unix local adapter, not first Windows-first boundary.
- HTTP + WebSocket: reasonable duplex option, but no current WebSocket was observed; adds a new server channel without eliminating replay/atomicity work.
- Raw Local IPC: category, not a concrete protocol; must specify framing/identity/access control before adoption.
- Remote secure transport: Deferred, not a reason to weaken local security or merge repositories.

## Security gate before real integration

No real integration acceptance until server identity verification, same-user/scoped access, secure credential provisioning/storage and revocation, authenticated reads/media, framing limits and restart/resume are demonstrated. Choose a concrete pairing/server-authentication mechanism in a follow-up Runtime task (e.g. verified local peer channel or pinned authenticated TLS); plaintext loopback HTTP alone does not prove identity or protect against hostile local peers. Endpoint setting must not contain bearer secrets; logs redact credentials. Never auto-connect an untrusted endpoint or downgrade after an identity error.

## Consequences

Bootstrap implementation: no production transport exists. `--fixture` selects scripted file loading through provider/background code; default startup is disconnected/not configured and malformed flags exit nonzero without a window. No automatic fixture fallback exists in either direction. `COS_DESKTOP_RUNTIME_ENDPOINT` is read only to distinguish configured vs unconfigured default text; it is never rendered. The loopback-HTTP/SSE direction remains a candidate for later Runtime-owned adapter work; `HelloAccepted.server_build` is an adapter identity, not a cosd existence claim.
This decision chooses the least duplicative first integration direction, not a finished security design. cos-client isolates UI from HTTP/SSE and may later implement Named Pipe without changing Domain meaning. No transport implementation, network service, environment installation or connection test was performed here.
