# RT3 media-expiry native residuals — 実測（2026-11-10）

**判定: `MEASURED`（native Expired UI / 既定 300 s TTL の advertised 値と取得 / ephemeral-secret 401）**。
`rt3-residual-latest` の NOT_MEASURED のうち、native artifact view の Expired 表示、
TTL 既定値 300 s の advertised expiry と blob 取得、`ORCHESTRATOR_COS_MEDIA_HANDLE_SECRET`
未設定（ephemeral）での restart 後 401 を、分離 Runtime と実 Desktop で測定した。
300 s の実時間失効だけは待たず **NOT_MEASURED** のままである。

この測定の副産物として、**実欠陥を 1 件再現した**（下記 §6）。同一 Desktop プロセスで
Runtime を restart すると、media binding が起動時の credential と audience を保持した
ままなので descriptor GET が **401 Unauthorized** となり、native pane は
`Forbidden`（閲覧権限なし）を表示して再試行しない。このため native Expired UI は
**cold relaunch（新プロセス）**で測定した。製品コードは本 commit では変更していない
（修正は別タスクの設計判断が必要。理由は §6）。

## 1. 環境と source

| 項目 | 値 |
|---|---|
| Desktop repo | `C:/dev/cos-desktop` |
| Desktop head（測定開始時） | `2fa792d30e1ba7572245fef4194d378293ab6029` |
| Desktop binary | `target/release/cos-desktop.exe`、`sha256:a2701360…2321742d8`、21,215,232 B、built `2026-11-10T10:57:58` |
| Runtime repo | `C:/AI/orchestrator` |
| Runtime head | `7764b7c4cdc8060bd9934dc72c4dcbab2eb60565` |
| Runtime dirty（測定前から） | `docs/guides/development-loop.md`、`orchestrator_app/dev_caller.py`、`tests/test_dev_caller.py`、`tests/test_dev_caller_e2e.py`、`devplan/*.json` 3 件。本測定では未使用・未変更 |
| 実行 | Python 3.13.14（uv-managed）で harness を実行。Desktop は release binary、Runtime は `isolated_runtime.py` / `isolated_runtime_descriptor_delay.py` |
| port | native 8132、protocol 8165。終了後 listener なしを確認 |
| secrets | `ORCHESTRATOR_AUTH_SECRET` は per-run 生成・memory のみ。native は `ORCHESTRATOR_COS_MEDIA_HANDLE_SECRET` を per-run 生成、protocol は **未設定**（ephemeral）。token/context/handle は evidence に保存しない |

Desktop のその他の dirty/untracked は測定開始時の worktree 状態として `session.json` に
記録した。本 commit には harness 2 本と本 evidence のみを含める。

## 2. 分離構成

### 2.1 native（8132）

- `target/acc1-native/20261109T233420-golden` の隔離 DB/CAS を **fresh copy** して使用
  （`task-ab47119a8bec467a8e128d7b82f1220a`、`sha256:eea85a24…3908a58`、1,363,011 B、
  1024×1024 PNG）。generation は実行していない。capability set は既存の test-only seed。
- DB copy `sha256:597a1c4eae387c6aa62b5e47d678c5a65ac9c39dc60b060b9292591b869ff3f8`、11,800,576 B。
- Runtime 1: TTL **60 s**、per-run handle secret、clean stop 後 DB copy を保持したまま
  Runtime 2 を起動（epoch 変更）。
- Runtime 2: 同じ DB/CAS/secret、TTL 60 s。harness-only の **descriptor delivery delay 63.0 s**
  を arm（`isolated_runtime_descriptor_delay.py`）。delay は Runtime が descriptor を
  **mint した後**、response bytes を client に書く前に sleep するだけであり、descriptor /
  handle 文書は書き換えない。`/blob` と他 route は遅延しない。

### 2.2 protocol（8165）

- `C:/AI/orchestrator/db/generation.sqlite` の read-only copy と CAS copy。
- Runtime 1/2 とも **TTL 環境変数なし**（既定 300 s）、**handle secret 環境変数なし**
  （per-process ephemeral）。Runtime 1 を clean stop し、同じ DB/CAS copy で Runtime 2 を
  起動（新 epoch / 新 ephemeral secret）。
- `art-v1-0d346285-c0ec-47fd-872e-428cbc467bab`、`sha256:59856b19…3117957`、1,247,461 B、1024×1024 PNG
  （`candidate_probe_count=3` で servable ORIGINAL を選択）。

## 3. 測定 1: native Expired UI

### 3.1 前提（同じ process で artifact を表示）

| 手順 | 観測 |
|---|---|
| Runtime 1 起動 | epoch `0e45e8988c9043ccb1a722059682892f`（`session.json` の native record 参照）、server identity `cos-runtime-orchestrator`、TTL 60 s |
| Desktop 起動・task 選択 | UIA `task-ab47119a… — ACC1-NATIVE-golden 202609… production still`、状態 `COMPLETED`、成果物 `AVAILABLE` |
| artifact viewer | state **`DISPLAYED`**、`メディア画像（記述子のダイジェスト検証済み）: 低解像度プレビュー。原寸の詳細表示・1:1ではありません。`、viewport controls 表示。screenshot `shots/native-displayed.png` |
| handle 失効待ち | 表示後 **70.0 s** 待機（TTL 60 s を超える） |
| Runtime 1 stop | exit 0、discovery 削除、port close |
| Runtime 2 起動 | epoch `cfded304674d42448231f038593171a9`（変更）、同一 DB/secret、TTL 60 s、descriptor delay arm 可能状態 |

### 3.2 同一 process の reload/refetch（欠陥の再現）

Runtime 2 起動後、Desktop の bounded reauthorization が完了したことを Runtime log の
`POST /api/cos/desktop/credentials` 200 と `POST /api/cos/desktop/observation` 200 で確認し、
Artifact surface を再表示した。

| 観測 | 値 |
|---|---|
| media descriptor GET | **`401 Unauthorized`**（`GET /api/cos/media/sha256%3Aeea85a…`） |
| native state | **`FORBIDDEN`**、`画像を利用できません: このセッションにはメディアの閲覧権限がありません。` |
| 画像 | `画像は未表示です`（`メディア画像（…）` の admit marker なし） |
| 再試行 | なし（401 は repository の再認可対象ではなく、`media_loaded` も解決開始時に設定済み） |
| screenshot / UIA | `shots/native-same-process-after-restart.png` / `uia/native-same-process-after-restart.json` |

この 401 → `Forbidden` は **欠陥**である（§6）。このため「同一 process で TTL 失効後に
Expired を表示する」測定は、この release では成立しない。

### 3.3 cold relaunch での Expired 表示（測定成立）

先行 process で artifact を表示した後、Desktop を clean close し、同じ Runtime 2・同じ
task で新しい process を起動した（media binding が現在の credential/audience で再構築される）。
descriptor delay は cold process が接続・task 選択を終え、Artifact surface へ遷移する直前に
arm した。

| 手順 | 時刻 / 結果 |
|---|---|
| cold Desktop 起動・task 選択・候補 view | `viewed=true`、`surface_switch=true`、`back_to_artifact=true` |
| 1 回目 descriptor GET | Runtime が descriptor を mint。応答を **63.000 s 遅延**して 200（`expires_at` は mint 時 +60 s） |
| client の descriptor 検証 | `expires_at_unix_ms` が過去 → `DescriptorExpired` → 1 回だけ descriptor 再取得 |
| 2 回目 descriptor GET | 同様に **63.000 s 遅延**して 200。再度 `DescriptorExpired`（`MAX_DESCRIPTOR_ATTEMPTS=2` 終端） |
| native state | **`EXPIRED`**、`画像を利用できません: メディア記述子の有効期限が切れました。再取得が必要です。` |
| 画像 | `画像は未表示です`。`メディア画像（…）` の admit marker なし。`stale_image_absent=true` |
| viewport | `全体表示 (0)` / `物理ピクセル1:1 (1)` / `拡大 (+)` / `縮小 (-)` の **control 名は残る**が、image area は空。`画像は未表示です`。hint 文も残る。これは「画像なし」の表示であり、stale image ではない |
| refresh affordance | actionable な `再取得`/`再読み込み` control は **なし**。文面のみ `再取得が必要です` |
| blob 要求 | **0 件**（descriptor が client 側で失効したため、handle を提示していない） |
| 経過 | cold 起動から Expired 表示まで **147.903 s**（delay 63 s × 2 + 検証・UI 反映） |
| screenshot / UIA | `shots/native-expired-cold.png` / `uia/native-expired-cold.json` |

Expired 表示後に delay flag を外し、harness 自身が **fresh credential + fresh descriptor** を
取得した。descriptor 200、handle 残 TTL 59.593 s、blob **200**、1,363,011 B、
`sha256:eea85a24…3908a58`（descriptor 宣言値と一致）。Runtime 自体はデータを失っておらず、
Expired pane が bytes を捏造していないことを確認した。

**注意（honest scope）**: この native Expired は **descriptor の client-side expiry**
（実 server が mint した `expires_at` を、transport 遅延で超過）によるものである。wire 上の
410 を native UI まで通した測定ではない。410 の実測は `rt3-residual-latest` の
executor/client 境界（`MEDIA_HANDLE_EXPIRED`）にあり、**native wire-410 → Expired UI は
NOT_MEASURED** のままである。同じ `MediaAvailability::Expired` / 同一文言に到達することを
確認した、という位置づけである。

## 4. 測定 2: 既定 300 s TTL

Runtime は TTL 環境変数を設定せずに起動（`ttl_env=null`）。descriptor は
`expires_at_unix_ms = now + 300000` を advertised し、handle も同じ exp を持つ。

| 項目 | 値 |
|---|---|
| advertised remaining（receipt 時） | **299.924 s**（`within_expected_window=true`） |
| handle `exp` remaining | **299.506 s** |
| blob | **200**、1,247,461 B、69.868 ms、`sha256:59856b19…3117957`、descriptor 宣言長・digest 一致 |
| 300 s 実失効 | **NOT_MEASURED**（5 分待機はしていない。advertised 値のみ） |

## 5. 測定 3: ephemeral handle secret の restart 後 401

Runtime 1/2 とも `ORCHESTRATOR_COS_MEDIA_HANDLE_SECRET` 未設定（per-process ephemeral）。
Runtime 1 で mint した handle は、Runtime 2 の新 secret では MAC 検証に失敗する。
Runtime 2 の fresh credential で旧 handle を提示した（MAC 検証は epoch/principal 判定より先）。

| 項目 | 値 |
|---|---|
| epoch | `c01a5c…` → `8585d91e…`（changed） |
| 旧 handle 残 TTL（提示時） | **294.298 s**（時間としては未失効） |
| 旧 handle 提示 | **401**、body `{"code":"MEDIA_UNAUTHORIZED"}`、29 B、69.488 ms |
| 410 か | **いいえ**（`not_410=true`）。ephemeral secret では MAC 不一致が先に成立する |
| Desktop mapping | `cos_client::MediaFetchError::Unauthorized` → `app_core::MediaAvailability::Forbidden`（`crates/cos-client/src/media.rs:423`、`crates/app-core/src/media_presentation.rs:209`）。native 文言は `画像を利用できません: このセッションにはメディアの閲覧権限がありません。` |
| mapping test | `cargo test -p app-core --lib every_fetch_failure_class_maps_to_one_availability_state_and_expiry_is_not_missing` → **1 passed / 0 failed** |
| fresh descriptor + handle | descriptor **200**、handle は旧と異なり epoch が Runtime 2 と一致、残 TTL 299.298 s |
| fresh blob | **200**、1,247,461 B、90.048 ms、`sha256:59856b19…3117957`、digest 一致 |

ephemeral secret の 401 は **Forbidden** に写像され、410 の **Expired** とは別 class である。
native UI で 401 を表示する経路は今回測っていない（client は restart を跨いで handle を
保持しないため）。**native 401 → Forbidden UI は NOT_MEASURED**。

## 6. 発見した実欠陥（未修正）

**同一 process で Runtime が restart すると、media binding の credential と audience が
起動時のまま固定され、再認証後の media descriptor が 401 になる。**

- 実測: `GET /api/cos/media/sha256%3Aeea85a… → 401 Unauthorized`、native
  `Forbidden` 文言、再試行なし（§3.2）。cold relaunch では正常。
- 根拠:
  - `apps/desktop/src/runtime.rs:433-447` の `media_binding` が `settings.token` と
    `audience` を closure に固定している。
  - `apps/desktop/src/runtime.rs:452-503` の `observation` は `CredentialSlot` で
    reauthorization するが、その slot は media 側と共有されない。
  - `apps/desktop/src/reconnect.rs:158` は reconnect 時に新しい `observation` を作るだけで、
    media binding を再構築/更新しない。`apps/desktop/src/main.rs:231` は起動時に media を
    一度だけ構築する。
- 追加の難しさ: Runtime restart 後の再認証は **新しい principal** を発行する
  （`credentials/refresh` 401 → `credentials` 200）。descriptor の `audience` は新 principal
  になるため、token だけを更新しても audience mismatch（Forbidden）が残る。修正は
  credential と audience の **session identity** を media と observation で共有するか、
  reconnect 時に media binding を明示的に再構築する設計判断が必要。
- 本 commit では製品コードを変更していない。測定は release binary
  `sha256:a2701360…2321742d8` で行い、上記は未修正のまま記録した。修正する場合は
  clean tree で build し、同一 process の Expired を再測する必要がある。

## 7. ゲート

| gate | command | result |
|---|---|---|
| harness compile | `python -m py_compile run_rt3_native_expiry.py isolated_runtime_descriptor_delay.py` | exit 0 |
| 401→Forbidden mapping | `cargo test --locked -p app-core --lib every_fetch_failure_class_maps_to_one_availability_state_and_expiry_is_not_missing -- --nocapture` | **1 passed / 0 failed** |
| leak scan | `leak-scan.json`（token/handle/Bearer pattern） | **clean**（`findings=[]`） |
| product code | 変更なし | Rust の fmt/clippy/test/build は本 commit の対象外（harness/evidence のみ） |

## 8. 測定済み / NOT_MEASURED

**測定済み**

- native artifact view の **Expired 表示**（cold relaunch、descriptor delivery delay 63 s、
  実 server mint の `expires_at` 超過、2 回の 200 応答）。`EXPIRED` 文言、画像なし、
  `メディア画像（…）` marker なし、blob 要求 0、fresh fetch 200 / digest 一致。
- 既定 TTL **300 s** の advertised expiry **299.924 s**（handle 299.506 s）と blob 200。
- ephemeral secret の restart 後、旧 handle **401 `MEDIA_UNAUTHORIZED`**（残 TTL 294.298 s、
  410 ではない）と Desktop mapping **Forbidden**。
- fresh descriptor + handle で blob 200（両測定）。
- 分離・clean stop・discovery 削除・port close・Desktop clean close。

**NOT_MEASURED**

- 既定 300 s の **実時間失効**（5 分待機していない）。
- **native wire-410 → Expired UI**（410 は executor/client 境界で実測済み。
  native は descriptor expiry で Expired を表示）。
- **同一 process** の Runtime restart 後の Expired（§6 の欠陥で Forbidden になる）。
- **native 401 → Forbidden UI**（client が restart を跨いで handle を保持しない）。
- PREVIEW / THUMBNAIL / METADATA の byte route、PNG 以外、複数 handle 同時失効、clock skew。

## 9. 再現

```text
C:/dev/cos-desktop> C:/Users/.../uv/python/cpython-3.13.14-windows-x86_64-none/python.exe -u ^
    apps/desktop/devtools/runtime_integration/run_rt3_native_expiry.py --part all
# evidence: docs/development/evidence-published/media-expiry-native-latest/
```

harness は 8132 / 8165 が空いていること、`C:/AI/orchestrator` の venv と source、release
`cos-desktop.exe` を要求する。native は closure run の隔離 DB/CAS を fresh copy し、
protocol は production DB/CAS を read-only copy する。終了時に app/Runtime を停止し、
port/discovery を確認する。token/context/handle は保存しない。Runtime log は sanitize し、
`leak-scan.json` で確認する。

## 10. 証跡

- `session.json` / `session.sha256` — 要約、verdicts、source/heads/binary/DB hash、cleanup
- `record-native-expiry.json` — native の precondition、表示、同一 process 401、cold Expired、
  fresh fetch、UIA/screenshot
- `record-protocol.json` — 300 s advertised TTL、blob、ephemeral 401、fresh descriptor/blob
- `defect.json` — §6 の欠陥の観測値、source refs、未修正の理由、推奨修正
- `uia/*.json`、`shots/*.png` — UIA dump と screenshot
- `logs/*.sanitized.txt` — Runtime / Desktop log（handle/Bearer redact 済み）
- `leak-scan.json` — secret/handle pattern scan（clean）
