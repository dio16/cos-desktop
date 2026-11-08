# NAV2 authenticated catalog owner contract

Status: implemented Desktop-side; live Runtime fetch NOT_MEASURED in this
workspace. Frozen by the NAV2 S1 review (task-31) against the task-29
authenticated reads.

## Route and authority

```text
GET /api/cos/desktop/catalog
Authorization: Bearer <desktop credential with read:observe>
Accept: application/json
```

- The credential is presented on the catalog route (profile of
  `is_protocol_route`); no `X-Cos-Context` is used because the catalog is not
  bound to one observation context.
- The response body is byte-bounded by the transport's configured maximum and
  is parsed with closed, typed structures (`CatalogDocument`, `deny_unknown_fields`).
- A `401` is `TransportError::Unauthorized` -> `ClientError::Unauthorized` ->
  `CommandReadError::AuthorizationUnavailable` (label: credential/observation
  authorization required). A `403` is `TransportError::Forbidden` ->
  `ClientError::Forbidden` -> `CommandReadError::AuthorizationDenied` (label:
  the operation is not permitted). Neither is a generic I/O failure.
- The owner thread waits at most 3 s for the worker's catalog reply
  (`CATALOG_FETCH_TIMEOUT`); the exchange itself runs on the transport worker.

## Document shape

```json
{
  "schema_version": 1,
  "server": {
    "server_identity": "cos-runtime-orchestrator",
    "epoch": "epoch-<opaque>"
  },
  "catalog": {
    "schema_version": 1,
    "can_submit": true,
    "reason_codes": [],
    "recipes": [
      {
        "recipe_ref": {
          "recipe_id": "recipe-<opaque>",
          "source_harness_version_id": "hvid_<opaque>",
          "source_harness_digest": "sha256:<64 hex>",
          "graph_ref": {
            "graph_id": "graph-<opaque>",
            "graph_version": 1,
            "graph_digest": "sha256:<64 hex>"
          },
          "profile_ref": null,
          "projection_digest": "sha256:<64 hex>"
        },
        "label": "Still",
        "description": "One still image",
        "input_limits": { "max_images": 2 },
        "accepts_image_references": true,
        "execution_limits": {},
        "constraint_summary": "read-only constraints",
        "parameters": [
          {
            "id": "seed",
            "label": "Seed",
            "type": "integer",
            "value": 7,
            "bounds": { "min": "1", "max": "99", "max_length": null },
            "choices": [],
            "editable": true
          }
        ]
      }
    ]
  }
}
```

- `server.server_identity` must equal the identity verified by the HELLO
  handshake (`Transport::authenticated_server`). A missing verified identity or
  a mismatch refuses the whole catalog as a protocol violation; the Brief never
  submits against an unverified catalog. `server.epoch` is carried into the
  create envelope; a catalog without a validated epoch cannot submit.
- An absent `input_limits.max_images`, malformed/inverted bounds, an empty
  choice set, or a default outside its bounds is a catalog-wide refusal
  (`ui-model::brief::BriefCatalogError`), never a silent default.
- Create submission: `POST /api/cos/desktop/commands` with a `CREATE_TASK`
  envelope (`target = Creation(recipe_ref)`, `epoch = catalog epoch`,
  `expected_revision = null`); acceptance is a receipt, not a local task row.

## Tests

- `crates/cos-client/tests/protocol_transport.rs`:
  `catalog_read_uses_the_authenticated_route_without_a_read_context`,
  `catalog_refusals_keep_the_authorization_taxonomy`,
  `catalog_identity_must_match_the_verified_handshake`,
  `create_task_posts_a_create_envelope_on_the_command_route`.
- `crates/app-core/tests/nav2_owner_forwarding.rs`: owner-thread forwarding,
  `catalog_401_maps_to_authorization_unavailable`,
  `catalog_403_maps_to_authorization_denied`.
- `crates/ui-model/src/brief.rs`: typed catalog mapping, epoch/limits
  fail-closed tests and the envelope epoch/expected_revision test.
