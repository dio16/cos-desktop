# ADR-0003: Independent runtime process

Status: **Accepted** target, implementation **Deferred**, 2026-09-21.

Use two processes: Desktop/GPUI and independently managed cosd/COS Runtime. [Process model](../architecture/PROCESS_MODEL.md) defines lifecycle. Tasks survive Desktop exit/crash/update; only canonical Runtime evidence establishes completion. Rejected: embedded Runtime or kill-on-close child ownership, because UI lifecycle must not terminate work.


Bootstrap implementation is client-side/fixture only: background executor loads `MockCosClient`, publishes an immutable observation, disconnect only cancels local observation, and stop acceptance never implies canonical completion. No real cosd/service exists and no Runtime process is started or supervised by Desktop. Fixture disconnect/reconnect is only client proof, not real Task survival.
