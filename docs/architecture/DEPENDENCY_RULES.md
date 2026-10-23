# Dependency rules

Status: **Accepted**. No workspace exists yet. See [repository architecture](REPOSITORY_ARCHITECTURE.md).

## Direct workspace dependency allowlist

Rows are dependents; cells list the only allowed direct workspace dependencies, including normal, build and dev dependencies. Absence means forbidden. Transitive reachability is not permission for a direct import. Third-party crates require role-appropriate review, not blanket approval.

| Dependent | Allowed direct workspace dependencies |
|---|---|
| apps/desktop | desktop-gpui, ui-model, app-core, cos-client, desktop-protocol; media/platform only after introduction |
| desktop-gpui | ui-model, app-core, desktop-protocol; media/platform only after introduction |
| ui-model | app-core, desktop-protocol |
| app-core | cos-client, desktop-protocol; media/platform only after introduction |
| cos-client | desktop-protocol |
| desktop-protocol | none |
| media (future) | desktop-protocol |
| platform (future) | none |

The top-level binary constructs concrete client/provider instances and injects them into app-core. It may reference lower-layer types for assembly, not perform UI-originated Runtime operations around app-core. desktop-gpui uses app-core for every Runtime operation; it cannot directly depend on cos-client. ui-model depends on app-core's UI-independent view/action contracts only; app-core must never depend back on ui-model. Avoid introducing this edge if unused.

Direct desktop-protocol imports are allowed to name projection DTOs and identities. Reading a DTO is not a command/transport call. Do not create trivial re-exports just to pass a type through every layer; do not duplicate same-shaped Domain definitions. UI-specific presentation types have explicit UI meaning and cannot claim canonical authority.

## Prohibitions

- No cycles, including build/dev edges; no Runtime internal crate, checkout path, submodule, DB/CAS/config access.
- GPUI dependencies and GPUI texture/resource management remain in desktop-gpui (and binary startup where required). ui-model/app-core are independently testable without GPUI.
- cos-client depends on neither GPUI, UI Model nor COS internal types. It exposes contract data, connection state and errors, not UI widgets.
- desktop-protocol cannot import a higher layer, persistence, workers, graph kernel, provider implementations or COS semantic internals.
- media/platform have no Task, evaluation, selection, graph revision, outcome or stop authority. Future media decode yields non-GPUI data; GPU upload/lifetime stays at the GPUI boundary.
- Test-only shortcuts across forbidden layers are not exemptions. Put client contract tests in the appropriate owner rather than making cyclic dev-dependencies.

## Planned automated check (bootstrap, not implemented)

`cargo metadata --locked --no-deps --format-version 1` checks parsability/resolution input, not these rules. A later developer/CI check must map workspace package IDs and dependency declarations (renames, normal/dev/build kinds, target-specific edges and path/workspace aliases included) to this allowlist, reject unknown internal edges/cycles, and reject runtime-checkout path dependencies and prohibited external package identities. Inspect all target conditions, not only the host's activated graph. Compare package manifests/source imports as needed for behavioral prohibitions: metadata cannot prove absence of direct DB calls or main-thread I/O.

The check's regression examples should include a renamed forbidden cos-client dependency from desktop-gpui, a target-specific forbidden edge, a dev-edge cycle, and an accepted direct DTO dependency. Maintain the table and checker together. There is no checker/script in this documentation-only delivery, and no dependency-compliance execution is claimed.
