# RT3 media-expiry residual — live measurement (2026-11-10)

**判定: `MEASURED`** — RT6/RC-13 の未測定残件だった **実ハンドル TTL 失効** と
**Runtime インスタンス（epoch）変更後の 410** を、分離 Runtime と実 Desktop
executor で初めて実測した。製品コードの変更はなく、Runtime 側も無変更。
変更は「harness 1 本 + executor の有界 stub テスト + 本 evidence」のみ。

- 対象: `apps/desktop/src/media_executor.rs`（binary-boundary executor）と
  `cos-client` / `app-core` の media 経路。GPUI/native 描画は対象外。
- 実行: `C:/dev/cos-desktop`（Desktop）と `C:/AI/orchestrator`（Runtime）。
- 結果: TTL 失効 410、epoch 変更 410、Desktop の `DescriptorExpired` /
  `Expired` 写像、および 410→descriptor 再取得→byte 再取得の経路を確認。

## 1. ソース・実行環境

| 項目 | 値 |
|---|---|
| Desktop head | `c1d97a4f028fed099c2e86f4150a7cc043029603`（tree `16c5b9af…c274979`） |
| 測定時の未コミット | `apps/desktop/src/media_executor.rs` と `run_rt3_residual.py` は測定時点で未コミット（測定後に commit。本 README は測定時の source 状態を記録し、commit SHA は最終報告に記載） |
| executor source | `sha256:9969c614a33ee9e83c37477ad24b1b82aba0d14938f7220df1b416a7ff05d019` |
| harness | `apps/desktop/devtools/runtime_integration/run_rt3_residual.py`、`sha256:f038811c…7f5a3850`（測定時） |
| Runtime head | `e1bc3e68944f4c4658efa7310dbab1dc8698fd5d`（Python 3.13.14 venv） |
| Runtime の既存 dirty | 7 件（`docs/guides/development-loop.md`、`dev_caller.py`、dev_caller テスト 2 件、`devplan/*.json` 3 件）。いずれも本測定では未使用・未変更 |
| DB source | `sha256:fbfe3aba…bc0c56f6`、11,493,376 B |
| DB copy (start) | `sha256:f8771100…bfb3e3d`、11,493,376 B |
| DB copy (end) | `sha256:261bc609…a3a5192`、11,522,048 B（journal 等の process-local 書込みは copy のみ） |
| CAS copy | 73 files、53,340,103 B |
| 対象 artifact | `art-v1-0d346285-c0ec-47fd-872e-428cbc467bab`、`sha256:59856b19…3117957`、`image/png`、1,247,461 B、1024×1024 |
| Desktop 実行 | 実 `MediaHttp` executor + `MediaFetcher` + `MediaRepository`（`cargo test --bin cos-desktop` の ignored live test） |

Desktop のその他の dirty entry（他レーンの docs/evidence と `run_ax_ime_dpi.py` など）は
測定時点の worktree 状態として `session.json` に記録し、本 commit には含めない。

## 2. 分離構成

| 項目 | 値 |
|---|---|
| endpoint | `http://127.0.0.1:8160`（自前ポート、run 終了後に listener なしを確認） |
| TTL 設定 | `ORCHESTRATOR_COS_MEDIA_HANDLE_TTL_SECONDS=60`（Runtime の clamp 最小値） |
| epoch (1 回目) | `36c7a5e51a2845a0bbc453136b8604a4`（serving pid 31,644 / launcher pid 52,568） |
| epoch (2 回目) | `d8b30946619b4cf6ae3f2836bafe252a`（serving pid 44,416 / launcher pid 22,744） |
| server identity | `cos-runtime-orchestrator`（両 instance で同一） |
| discovery | `runtime-1` `sha256:824ec1b9…f4e642`、`runtime-2` `sha256:f31b03cd…8efff6`（各 307 B、origin 8160、`ready=true`、clean stop で削除） |
| DB/CAS | `target/rt3-residual/run1` に read-only backup + copy。config の `db_path`/`project_root`/`output_root`/`log_dir` と `ORCHESTRATOR_COS_*` を workspace 内へ redirect |
| secrets | `ORCHESTRATOR_AUTH_SECRET` と `ORCHESTRATOR_COS_MEDIA_HANDLE_SECRET` は per-run 生成・memory のみ。token / context / handle は一切保存しない |

`python.exe` は uv-managed venv の launcher shim のため、Popen の pid と
discovery の `pid`（= uvicorn の serving process）は一致しない。discovery の pid は
Runtime log の `Started server process [n]` と一致することを確認した
（`pid_matches_serving_process=true`）。これは Runtime の欠陥ではなく、
launcher のプロセス会計である。

## 3. 測定 1: 実 TTL 失効（同一 handle）

descriptor `descriptor-93c7e19d…3ef43540`（ORIGINAL v1、AVAILABLE、VERIFIED、
handle length 474、`expires_at_unix_ms=1794244743769`）を発行後、直ちに同一 handle で
blob を取得し、TTL 経過後に同じ handle を再提示した。

| 手順 | 時刻 | 結果 |
|---|---|---|
| descriptor 発行 | 2026-11-10T02:18:03+0900 | 200、`expires_at_unix_ms=1794244743769` |
| 直後の blob | 02:18:03 | **200**、1,247,461 B、71.681 ms、`sha256:59856b19…3117957`（descriptor 宣言値と一致） |
| TTL 待機 | — | 発行から **65.001 s**、server expiry の **5.052 s 後**まで待機 |
| 同一 handle の blob | 02:19:08 | **410**、body `{"code":"MEDIA_HANDLE_EXPIRED"}`（31 B、42.626 ms） |
| 新 descriptor + 新 handle | 02:19:09 | **200**、descriptor `descriptor-d69c53df…ccd87c`、handle は旧 handle と異なる（`new_handle_differs_from_expired=true`）、blob **200**、76.215 ms、同一 digest |

期待値（410 / `MEDIA_HANDLE_EXPIRED`）と一致。失効後に descriptor を再取得すると
新しい handle が発行され、その handle で 200 が返ることを確認した。

## 4. 測定 2: Runtime instance 変更（epoch）後 410

TTL 待機の直後に、**まだ有効な** handle（`expires_at_unix_ms=1794244810644`、
再起動要求時点の残 TTL **60 s**、handle の `epc` は現 epoch と一致）を発行して
blob 200 を確認し、その直後に Runtime を clean stop → 同一 DB copy / 同一 per-run
handle secret で新 process を起動した。

| 手順 | 時刻 | 結果 |
|---|---|---|
| 再起動前 probe | 02:19:10 | descriptor 200、blob **200**、1,247,461 B、handle epoch = `36c7a5e5…` |
| Runtime 1 stop | — | exit 0、port closed、discovery removed |
| Runtime 2 start | 02:19:14 | epoch `d8b30946…`（変更）、server identity は不変、HTTP ready |
| 旧 handle を新 credential で提示 | 02:19:14 | **410**、body `{"code":"MEDIA_HANDLE_EXPIRED"}`（31 B、75.748 ms）。提示時点で残 TTL **56 s**、handle の epoch は旧・serving epoch は新、handle の principal は新 credential と不一致 |
| 再認証後の新 descriptor + handle | 02:19:14 | **200**、descriptor `descriptor-54e0a360…5b1ee03`、blob **200**、67.501 ms、同一 digest |

旧 handle は TTL 内（残 56 s）であり、epoch 不一致が 410 の原因である。
`open_handle` は `sid` → `epc` → `sub` の順に検査するため、principal も異なる状況で
410 が返ることは epoch 判定が principal 判定より先であることの実測でもある
（403 ではない）。新 Runtime では credential を再発行し、新しい observation context で
descriptor を取得し直すと blob 200 が返る。

## 5. Desktop executor / client の 410 写像と再取得

### 5.1 live server の 410（実 executor）

同じ live test を TTL 失効後と epoch 変更後の 2 回実行した（各 exit 0、1.59 s / 1.588 s）。
いずれも実 `MediaHttp` executor が live Runtime の 410 を受け、次の写像を記録した。

| 入力 | client 結果 | availability | Missing か |
|---|---|---|---|
| TTL 失効後の旧 handle | `Err(DescriptorExpired)` | `Some(Expired)` | `false` |
| epoch 変更後の旧 handle | `Err(DescriptorExpired)` | `Some(Expired)` | `false` |
| bogus `X-Cos-Context`（対照） | `Err(Unauthorized)` | — | — |
| 別 principal の credential（対照） | `Err(Forbidden)` | — | — |

live test の主経路も実測: descriptor 取得 → 1,247,461 B 取得 → `admitted_verified=true`
→ digest 一致 → decode 1024×1024 RGBA（4,194,304 B）→ presentation request。

### 5.2 410 → descriptor 再取得 → byte 再取得（有界 stub）

live Runtime は「同じ repository 呼出しの中で handle だけ失効させ、直後の descriptor
取得は成功させる」状態を外部から作れないため、この 1 回再認証の経路は loopback stub
server で実 executor / `MediaFetcher` / `MediaRepository` を通して固定した。

- テスト: `apps/desktop/src/media_executor.rs::tests::an_expired_handle_reauthorizes_the_descriptor_and_admits`
- 応答列: descriptor 200（handle A）→ blob **410** → descriptor 200（handle B）→ blob 200
- 結果: `MediaRepository::load` が **descriptor 2 回 + byte 2 回** を発行して bytes を
  admit。`admitted_verified=true`、digest 一致。`Missing` にはならない。
- 単体の写像: `a_410_from_the_real_executor_is_descriptor_expired_not_missing` が
  executor の 410 を `MediaFetchError::DescriptorExpired` → `MediaAvailability::Expired`
  に写像し、`Missing` と異なることを assert。

該当実装: `crates/cos-client/src/media.rs:427`（410 → `DescriptorExpired`）、
`crates/app-core/src/media_presentation.rs:215`（→ `Expired`）、
`crates/app-core/src/media_presentation.rs:649`（1 回だけ descriptor を再取得）。

## 6. ゲート

| gate | コマンド | 結果 |
|---|---|---|
| Python harness | `python -m py_compile .../run_rt3_residual.py` | exit 0 |
| Rust format | `cargo fmt --all -- --check` | exit 0 |
| executor tests | `cargo test --locked -q -p cos-desktop --bin cos-desktop media_executor::tests:: -- --test-threads=1` | **15 passed / 0 failed / 1 ignored**（ignored は live test 本体） |

`live_media_evidence` は harness が `--ignored` で 2 回実行し、各 exit 0。

## 7. 測定済み / NOT_MEASURED

**測定済み**

- TTL=60 s の実失効: 発行から 65.001 s、server expiry の 5.052 s 後に同一 handle が
  **410 `MEDIA_HANDLE_EXPIRED`**（body 31 B）。
- 失効後の新 descriptor が新しい handle を発行し、blob 200（digest 一致）。
- Runtime instance（epoch）変更: TTL 内（残 56 s）の旧 handle が **410
  `MEDIA_HANDLE_EXPIRED`**。server identity は不変、epoch のみ変更。
- 再認証後の新 descriptor + handle で blob 200（digest 一致）。
- 実 executor による live 410 の `DescriptorExpired` / `Expired` 写像（2 例）。
- 410 → descriptor 再取得 → byte 再取得 → admit（有界 stub、4 HTTP exchange）。
- 分離・停止・discovery 削除・port 閉塞・work copy 削除。

**NOT_MEASURED**

- 既定 300 s TTL（最小値 60 s のみ）。300 s の実時間測定はしていない。
- expiry 秒ちょうどの境界（本測定は 5.05 s 後。秒精度の境界は未測定）。
- per-run handle secret を設定しない既定（ephemeral secret）での restart。この場合は
  MAC 不一致で 401 になる設計であり、本測定の 410 とは別経路。
- GPUI/native での Expired 表示・再試行 UI。本 evidence は executor/client 境界まで。
- PREVIEW / THUMBNAIL / METADATA の byte route、PNG 以外の media type。
- 長時間 repository session 中の TTL 失効（単発 load のみ）。
- TTL 中の clock skew、複数 handle の同時失効。

## 8. 残件（residual）

- live server では handle のみを失効させた 1 回再認証を外部から強制できないため、
  refetch 経路の固定は bounded stub test による。live 側は「410 の実 status と
  executor の写像」を測っている。
- instance 変更 410 は同一 per-run handle secret を使った。secret が ephemeral な
  既定運用では restart は 401（MAC 不一致）になる。両者は別の failure class であり、
  本測定は 410 の epoch 経路を対象にしている。
- `open_handle` の検査順（sid → epc → sub）により、epoch と principal が同時に
  変わった場合は 410 が返る。今回の測定はこの順序を実測したもので、欠陥ではない。
- descriptor id は取得ごとに新規発行される（同一 artifact でも異なる）。これは
  RC-13 の設計どおり。
- Runtime の product 変更は不要だった。Runtime 側の dirty 7 件は測定前から存在し、
  本測定では触れていない。

## 9. 再現

```text
C:/dev/cos-desktop> C:/AI/orchestrator/.venv/Scripts/python.exe -u ^
    apps/desktop/devtools/runtime_integration/run_rt3_residual.py
# evidence: docs/development/evidence-published/rt3-residual-latest/
```

harness は 8160 が空いていること、Runtime venv Python、`C:/AI/orchestrator` の
source を要求する。DB は read-only backup で copy し、CAS も copy する。Runtime は
`isolated_runtime.py` で起動し、clean lifespan shutdown、discovery 削除、port 閉塞を
確認してから work copy を削除する。token / context / handle は保存しない。
runtime log は handle を redact した sanitized 版のみを残す。

## 10. 証跡ファイル

- `session.json` — 要約、source hash、instance/epoch、全 status/timing、cleanup、verdict
- `record.json` — 詳細（descriptor、待機時間、410 body、handle epoch、principal 一致、ログ hash）
- `executor-live-ttl.json` / `executor-live-epoch.json` — 実 executor の value-free 出力
- `runtime-first.log.sanitized.txt` / `runtime-second.log.sanitized.txt` — handle redact 済み
- `session.sha256` — 上記 + 本 README の SHA-256
