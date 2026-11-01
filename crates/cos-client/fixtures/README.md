# Runtime payload fixtures

`runtime-task-view-v1.json` is the exact JSON the Runtime emits for
`GET /api/cos/tasks/{task_id}`. It is **generated from the Runtime's own model**,
never hand-written, so a shape drift between the repositories fails the adapter
tests.

Generation (in the Runtime checkout, `orchestrator/`):

```text
.venv/Scripts/python.exe -c "
from orchestrator_app.cos_frontend_models import TaskViewPayload
print(TaskViewPayload(...).model_dump_json(indent=2))"
```

Provenance:

| Item | Value |
|---|---|
| Runtime repository | `dio16/ai-generation-loop-orchestrator` |
| Runtime commits | `33ba9770` (projection contract), `6d73541c` (task-view axes) |
| Regenerate when | the Runtime payload model or its required axes change |

Regeneration is a manual, reviewed step: replace the file, run
`cargo test -p cos-client`, and record the Runtime commit in the change.
