# follow-up-1024 — live 1024×1024 media display evidence (2026-11-09)

Follow-up to the stage-3 run in `../session.json`. The earlier run admitted,
digest-verified and decoded the live 1024×1024 artifact but the presentation
admitted class was pinned to 2048×2048 RGB8, so the surface honestly refused.
This run repeats the same isolated integration with the expanded admitted class
and records the picture actually being displayed.

Recreate with (the release binary must be built from the same source commit):

```powershell
C:/AI/orchestrator/.venv/Scripts/python.exe `
    apps/desktop/devtools/runtime_integration/run_media_integration.py `
    --port 8128 --label geom1024 `
    --evidence-dir docs/development/evidence-published/img6-rt6-latest/follow-up-1024
```

The harness builds nothing: run `cargo build --locked --release -p cos-desktop`
first. It owns `127.0.0.1:8128`, a fresh DB copy and CAS copy under
`target/media-live/geom1024/`, its own config/logs, a per-run
`ORCHESTRATOR_AUTH_SECRET` and a per-run `ORCHESTRATOR_COS_MEDIA_HANDLE_SECRET`
(both memory-only), and the native GUI lock. It never starts a generation task:
the DB copy is read only and only existing artifacts are fetched.

## Files

| File | Content |
|---|---|
| `session.json` | Verdict, per-scenario results, source hashes, Runtime identity, attributed native facts. |
| `record.json` | Full record: descriptor fields, byte counts, digests, admission/decode/presentation outcome, negative matrix, native UIA texts, screenshot diff, log excerpts, leak check. |
| `descriptor.json` | Harness probe descriptor for the selected artifact. This is a sanitized probe: its `access_handle` is deliberately redacted/absent, while `executor-observations.json` records the handle's presence/length; the absent handle is a redaction, not a contract gap. |
| `executor-observations.json` | The executor live test's own descriptor/bytes/admission/decode/presentation-request results (a second issuance for the same canonical bytes). |
| `artifact-surface-before.png` | Artifact surface before the Dock candidate is viewed. |
| `artifact-surface-media.png` | Artifact surface after the viewed candidate's media was fetched, admitted, decoded and painted: the picture is visible and the status sentence names the admitted low-resolution preview. |
| `runtime.log.sanitized.txt`, `desktop.log.sanitized.txt` | Redacted logs (tokens, issuance/handle secrets and every `mh1.` handle value removed). |

## Observed (this run)

- Source: DB copy sha256 `f8771100…bfb3e3d` (11,493,376 B), 73 CAS files /
  53,340,103 B, 72 image artifacts (36 × 1024×1024 and 36 × 320×320, all 8-bit
  RGB PNG). Desktop binary sha256 `29ee307f…4ff6`, built from `4ef265b`.
- Isolated Runtime `cos-runtime-orchestrator` pid 36912, epoch
  `756ea5fe3bc44706954006598ab84bf2`, protocol/schema 2.0; fresh Desktop
  principal `desktop:b3a34a90…bfaf`, fresh scope `scope-c9c83fd2…84bd`,
  39 TASK projections; the Runtime was stopped cleanly afterwards.
- Executor artifact `art-v1-0d346285-…`, digest
  `sha256:59856b19…3117957`, `image/png`, 1,247,461 B, 1024×1024 8-bit RGB.
  Descriptor `descriptor-fbe9c92b…` `ORIGINAL` v1 `AVAILABLE` `VERIFIED` with
  the declared digest; 1,247,461 B fetched, computed SHA-256 equals the
  declared digest, admission verified, decode 1024×1024 RGBA8 (4,194,304 B).
- Presentation request: built (`presentation = request`) with descriptor
  geometry 1024×1024 and decoded geometry 1024×1024, i.e. the expanded admitted
  class accepts the live artifact instead of the former `UnsupportedGeometry`.
- Negative matrix through the same executor: bogus context → 401
  `Unauthorized`; foreign-principal descriptor → 403 `Forbidden`; expired handle
  → 410 `DescriptorExpired`; foreign-principal handle → 403 `Forbidden`.
- Native release Desktop (endpoint + per-run issuance secret only, fresh
  `LOCALAPPDATA`): window found, task title and the Runtime media statement
  visible; activating a Dock candidate through its UIA `Invoke` pattern made the
  app fetch one descriptor (200) and one blob (200), both attributed to the app
  in the Runtime access log. The surface then showed the picture with
  `メディア画像（記述子のダイジェスト検証済み）: 低解像度プレビュー。原寸の詳細表示・1:1ではありません。`
  for `art-v1-560c8100-…` (`sha256:b0af1941…`, 1,285,309 B, 1024×1024 8-bit
  RGB; 256-px preview mip, the current preview-only viewer path).
- Screenshots: `artifact-surface-before.png` sha256 `5b81faa6…f123`;
  `artifact-surface-media.png` sha256 `873eb6bd…6691`. Measured pair diff:
  201,345 of 1,087,344 pixels changed, bbox 1296×751, 22,586 distinct colours.
- Clean `WM_CLOSE` (exit 0, both close markers); leak check clean (no handle,
  token or secret in any text evidence file).

## NOT_MEASURED (honest residual)

- The shown picture is the admitted preview mip because the artifact viewer runs
  in `PreviewOnly` mode; full-detail/1:1 presentation, product fidelity and UX
  acceptance are not claimed.
- PREVIEW/THUMBNAIL/METADATA byte routes are not exercised by this slice
  (PREVIEW stays `UNSUPPORTED`; no transcode), and no EXIF/orientation
  interpretation exists (`Orientation::Normal` is the only admitted class).
- A real handle-TTL expiry and a Runtime-instance-change 410 are not separately
  measured (the 410 above uses a harness-minted expired handle).
- GPU transfer duration/completion/residency and Present remain `NOT_MEASURED`;
  the evidence is PrintWindow + UIA, not a GPU readback.
- The 320×320 edge is admitted by the same closed class and covered by tests,
  but this live run selected 1024×1024 artifacts; a live 320×320 display was not
  this run's scenario.
