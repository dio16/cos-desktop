# img6-rt6-latest — live media acquisition evidence (2026-11-08)

One isolated, repeatable run of the real Desktop media boundary against a real
isolated Runtime (RC-13 / MP-IMG6 / MP-RT6 stage 3). Recreate with:

```powershell
C:/AI/orchestrator/.venv/Scripts/python.exe `
    apps/desktop/devtools/runtime_integration/run_media_integration.py
```

The harness builds nothing: run `cargo build --locked --release -p cos-desktop`
first. It owns `127.0.0.1:8127`, a fresh DB copy and CAS copy under
`target/media-live/run1/`, its own config/logs, a per-run
`ORCHESTRATOR_AUTH_SECRET` and a per-run `ORCHESTRATOR_COS_MEDIA_HANDLE_SECRET`
(both memory-only), and the native GUI lock. It never starts a generation task:
the DB copy is read only and only existing artifacts are fetched.

## Files

| File | Content |
|---|---|
| `session.json` | Verdict, per-scenario results, source hashes, Runtime identity, attributed native facts. |
| `record.json` | Full record: descriptor fields, byte counts, digests, admission/decode/presentation outcome, negative matrix, native UIA texts, log excerpts, leak check. |
| `descriptor.json` | Harness probe descriptor for the selected artifact (issuance #1). This is a sanitized probe: its `access_handle` is deliberately redacted/absent, while `executor-observations.json` records the handle's presence/length for the executor's own issuance (#2). The absent handle is a redaction, not a contract gap. |
| `executor-observations.json` | The executor live test's own descriptor/bytes/admission/decode results (a second issuance for the same canonical bytes). |
| `artifact-surface-before.png` | Artifact surface before the Dock candidate is viewed. |
| `artifact-surface-media.png` | Artifact surface after the viewed candidate's descriptor and bytes were fetched; the explicit non-available sentence is shown. |
| `runtime.log.sanitized.txt`, `desktop.log.sanitized.txt` | Redacted logs (tokens, issuance/handle secrets and every `mh1.` handle value removed). |

## Observed (this run)

- Artifact `art-v1-0d346285-…`, digest `sha256:59856b19…3117957`, `image/png`,
  1,247,461 B, 1024×1024 8-bit RGB.
- Descriptor: `ORIGINAL` v1, `AVAILABLE`, `VERIFIED`/`sha256` with
  `expected_digest` equal to the content digest, server identity
  `cos-runtime-orchestrator`, audience = the Desktop principal.
- Bytes fetched through the real binary executor: 1,247,461 B; computed
  SHA-256 equals the declared digest; admission verified; decode 1024×1024
  RGBA8 (4,194,304 B).
- Negative matrix through the same executor: bogus context → 401
  `Unauthorized`; foreign-principal descriptor → 403 `Forbidden`; expired
  handle → 410 `DescriptorExpired`; foreign-principal handle → 403 `Forbidden`.
- Native release Desktop: real task title and the Runtime media statement are
  visible; activating a Dock candidate through its UIA `Invoke` pattern made the
  app fetch one descriptor (200) and one blob (200) — both attributed to the app
  in the Runtime access log — and the surface rendered
  `画像を利用できません: 記述子の画像寸法は検証済みの範囲外です。`.

## NOT_MEASURED (honest residual)

- The picture itself: every task-linked artifact in the live DB copy is
  1024×1024 (or 320×320), while the shipped admitted presentation class is the
  pinned 2048×2048 RGB8 media proof. The bytes are admitted and decoded, then
  the presentation request is explicitly refused (`UnsupportedGeometry`); no
  picture is fabricated and this slice does not broaden the pinned class.
- PREVIEW/THUMBNAIL/METADATA byte routes, EXIF/orientation, a real handle-TTL
  expiry or Runtime-instance-change 410, GPU residency/Present, screen
  reader/IME/DPI and product/UX acceptance are outside this run.

## Follow-up: the 1024×1024 artifact is displayed (2026-11-09)

[`follow-up-1024/`](follow-up-1024/README.md) repeats this isolated run on the
expanded admitted class (`media::APPROVED_EDGES = [320, 1024, 2048]`). It
records the same artifact admitted, digest-verified and decoded, the
presentation request built, and the native surface showing the picture with
`メディア画像（記述子のダイジェスト検証済み）: 低解像度プレビュー。…` plus a
screenshot whose media region contains the image. The stage-3 run above is
unchanged history: at that code tip the 2048-only pin was the honest outcome.
