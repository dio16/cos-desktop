# Runtime payload fixtures

`runtime-task-view-v1.json` is the exact JSON the Runtime emits for
`GET /api/cos/tasks/{task_id}`. It is **generated from the Runtime's own model**,
never hand-written: regenerating it and diffing is how a shape drift between the
repositories is caught. The saved copy alone does not detect drift (see
[Limit](#limit)).

## Regeneration (in the Runtime checkout, `orchestrator/`)

```text
.venv/Scripts/python.exe -c "
import json
from orchestrator_app.cos_frontend_models import TaskViewPayload
d='sha256:'+'a'*64
v=TaskViewPayload(
    schema_version=1, epoch='epoch-7f3c', projection_revision=12,
    task_id='task-184', title='Neon alley at dusk', task_status='ACTIVE',
    phase='Generating', iteration=3, capabilities_ref=d, stop_pending=False,
    watch=True,
    accepted_input={
        'objective_text': 'Neon alley at dusk', 'text_prompt': 'neon',
        'recipe_ref': {
            'recipe_id': 'r1', 'source_harness_version_id': 'hvid_'+'b'*64,
            'source_harness_digest': d,
            'graph_ref': {'graph_id': 'g1', 'graph_version': 4, 'graph_digest': d},
            'projection_digest': d,
        },
        'reference_artifact_refs': [], 'parameters': [],
        'execution_limits': {}, 'constraint_summary': '',
    },
    outcome=None,
    available_actions={'stop': {'enabled': True, 'reason_code': None}},
    issues=[],
)
print(json.dumps(v.model_dump(), indent=2))
" > C:/dev/cos-desktop/crates/cos-client/fixtures/runtime-task-view-v1.json
```

Run it from the Runtime checkout (`<runtime>/orchestrator/`). The redirect names
the full path to **this** checkout's fixture, because that directory does not
exist relative to the Runtime repository. Then run `cargo test -p cos-client`
and record the Runtime commit in the change.

## Provenance

| Item | Value |
|---|---|
| Runtime repository | `dio16/ai-generation-loop-orchestrator` |
| Runtime commits | `33ba9770` (projection contract), `6d73541c` (task-view axes), `7b35c935` (required `stop_pending`) |
| Regenerate when | the Runtime payload model or its required axes change |

## Limit

The saved fixture proves the *current* shape only. It cannot detect a later
Runtime change on its own: the comparison happens when the fixture is
regenerated, which is a manual, reviewed step. Continuous drift detection would
need a live integration check, which this repository does not run in its default
gate.
