# RT3 media-expiry refetch — 実測（2026-11-11）

**判定: `MEASURED`（native Expired の actionable refetch / wire-410 `MEDIA_HANDLE_EXPIRED` → Expired / 既定 300 s TTL の実失効）**。

`media-expiry-native-latest` と `media-session-identity-latest` が残した UX 欠陥
（EXPIRED が文面のみで再取得操作がなく、zoom/1:1 の control 名だけが空画像の上に残る）を
製品コードで修復し、分離 Runtime と実 Desktop で測定した。同一プロセスの credential 欠陥は
`e7a3a10` の `SessionIdentity` で修復済みであり、本測定はその修復を含む product source を使う。
native wire-410 は **blob 要求側の harness 注入遅延**で実サーバーの expiry 判定に到達させて
測定した（descriptor を書き換えない）。既定 300 s TTL は protocol 境界で実際に 305 s 待って
失効を確認した。

## 1. 修復（commit `98c45b9`）

- `ImageViewer` は media 解決結果の明示状態を `MediaRefetchState` として保持し、
  **`Expired` / `Forbidden` のときだけ** `メディアを再取得 (R)` を 1 つ提示する
  （`Role::Button`、aria name、UIA Invoke、`r` キー）。試行中は control を消し、
  連打・キーリピートでも解決は 1 回だけ（viewer が emit 前に offer を消費）。
- shell は `ViewerEvent::RefetchMedia` を受けて `MediaLoadKey` の manual-refetch nonce を
  1 つ進め、`sync_artifact_media` を再実行する。解決は**同一 canonical identity** を
  現在の session identity（live credential slot、principal/audience、epoch、read context）
  で再取得するだけで、capability gate・cancellation・結果順序は既存のまま。
- 開始できない場合（観測/lease なし、Runtime media binding なし）は
  `media_refetch_unavailable_statement()` の明示文にし、偽の再試行はしない。失敗は
  明示状態を再描画し、成功は `DISPLAYED` に置き換える。bytes の捏造、失効後の stale frame
  再利用、新しい secret/handle ログはない。Compare pane に refetch 経路はない。

## 2. 環境と source

| 項目 | 値 |
|---|---|
| Desktop repo | `C:/dev/cos-desktop` |
| Desktop source | HEAD `885196efa5f0611a05083b8b5ef44ae2661d0a50` + 本修復 `98c45b9`（測定時は dirty worktree。`session.json` に status を記録） |
| Desktop binary | `target/release/cos-desktop.exe`、`sha256:382e8862…05919e5f`、21,271,040 B、built `2026-11-10T23:10:21` |
| Runtime repo | `C:/AI/orchestrator` |
| Runtime head | `06ee825eeb3be741b8a870189ed28f848c811630`（native / default-TTL とも同じ。dirty は測定前からの `dev_caller`/`devplan`/docs で本測定では未使用） |
| 実行 | Python 3.13.14（uv-managed）で harness を実行 |
| native port | **8135**（終了後 listener なし） |
| default-TTL port | 8168（終了後 listener なし） |
| native 分離 | `target/acc1-native/20261109T233420-golden` の fresh DB/CAS copy。DB copy `sha256:597a1c4e…69ff3f8`、11,800,576 B。generation は実行していない。discovery-only |
| native TTL | `ORCHESTRATOR_COS_MEDIA_HANDLE_TTL_SECONDS=60` |
| secrets | `ORCHESTRATOR_AUTH_SECRET` / `ORCHESTRATOR_COS_MEDIA_HANDLE_SECRET` は per-run 生成・memory のみ。token/context/handle は evidence に保存しない |
| harness | `apps/desktop/devtools/runtime_integration/run_rt3_media_refetch.py` |
| launcher | `apps/desktop/devtools/runtime_integration/isolated_runtime_media_delay.py`（descriptor delivery delay / blob request delay の 2 wrapper。flag file で arm） |

**遅延の意味（honest scope）**

- descriptor delivery delay 63.0 s: Runtime が descriptor と handle を**mint した後**、
  応答 bytes を書く前に sleep する。descriptor/handle 文書は書き換えない。wire は 200 の
  まま client 側の `expires_at_unix_ms` 検証が失効を検出する。
- blob request delay 63.0 s: `/blob` 要求を Runtime が処理する**前**に sleep する。
  実 handle の TTL が過ぎた状態でサーバー自身の `open_handle` 判定が走り、実 wire の
  **410 `MEDIA_HANDLE_EXPIRED`** を返す。応答の書き換えはない。

## 3. native 測定（8135、TTL 60 s）

### 3.1 cold start（対照）

| 観測 | 値 |
|---|---|
| artifact view | `DISPLAYED`、`メディア画像（記述子のダイジェスト検証済み）: 低解像度プレビュー。原寸の詳細表示・1:1ではありません。` |
| descriptor / blob | 200 / 200（Runtime access log） |
| descriptor advertised remaining | 59.979 s |
| refetch control | **なし**（`メディアを再取得 (R)` は UIA 名に存在しない） |
| screenshot / UIA | `shots/cold-displayed.png` / `uia/cold-displayed.json` |

### 3.2 TTL 失効 → in-process Runtime restart → descriptor delay で Expired

| 段階 | 観測 |
|---|---|
| 表示後 70.0 s 待機 | descriptor/handle の 60 s TTL を超過。待機前に `DISPLAYED` を確認 |
| Flow へ退避 → Runtime 1 clean stop | exit 0、port free。descriptor delay flag を **restart 前に arm**（Desktop は Flow 上で media 要求なし） |
| Runtime 2 start | epoch 変更（`runtime_2.epoch_changed=true`）、同一 DB/CAS/secret、discovery 再出現 |
| 同一プロセスの reconnect | `POST /credentials` 200 と `POST /observation` 200 を確認（途中 401 → refresh 401 → credentials 200 → observation 409 → 200） |
| task 再選択 | `selected=true`、`scope_task_matched=true` |
| Artifact 再進入 | surface 遷移を**最大 4 回**まで再実行（後述）。4 回目で `harness media descriptor delay begin` を観測し、2 回の descriptor GET が各 63.0 s 遅延（`expires_at` は mint 時 +60 s） |
| native state | **`EXPIRED`**、`画像を利用できません: メディア記述子の有効期限が切れました。再取得が必要です。` |
| refetch control | **あり**: UIA 名 **`メディアを再取得 (R)`**、`ControlType.Button`、InvokePattern |
| 画像 | `画像は未表示です`。`メディア画像（` marker なし（stale frame なし） |
| viewport | `全体表示 (0)` / `物理ピクセル1:1 (1)` / `拡大 (+)` / `縮小 (-)` の control 名は残る（既知の表示） |
| 経過 | 最初の surface 遷移から **126.296 s** |
| screenshot / UIA | `shots/expired-control.png` / `uia/expired-control.json` |

**surface 遷移 4 回の理由（実測事実）**: restart 後の shell は、publication/lease が新しい
epoch を反映するまで media sync の key が変わらず、Artifact 再進入 1–3 回目は
descriptor 要求を発行しなかった（`delayed_descriptor_seen=false`）。4 回目
（00:44:14）で初めて遅延 descriptor 要求が始まった。harness は media を自動再試行せず、
surface 遷移だけを bounded に繰り返して sync の再評価を促した。

### 3.3 refetch の in-flight bound（descriptor delay 継続中）

| 観測 | 値 |
|---|---|
| 操作 | UIA Invoke（`ControlType.Button` / `メディアを再取得 (R)` / pattern `invoke`） |
| 試行中（1.5 s 後） | control **なし**、状態文は `EXPIRED` のまま（画像なし）。`uia/refetch-in-flight.json` |
| 試行の descriptor 要求 | **2 回**（`MAX_DESCRIPTOR_ATTEMPTS`）。遅延 63 s × 2 |
| 再失敗後 | `EXPIRED` の明示状態に戻り、control が**再提示**される（`uia/expired-after-refetch-failure.json`） |

連打・キーリピートは viewer の `pending` が吸収し、追加の解決は始まらない。

### 3.4 native wire-410 `MEDIA_HANDLE_EXPIRED` → Expired

| 観測 | 値 |
|---|---|
| 準備 | descriptor delay flag を解除し、blob request delay flag を arm |
| 操作 | 同じ control を UIA Invoke |
| Runtime access log | descriptor **200** → blob **410** → descriptor **200** → blob **410** |
| 410 応答 body | `{"code":"MEDIA_HANDLE_EXPIRED"}`（31 B、harness wrapper が記録） |
| 遅延ログ | `harness media blob request delay begin/release` 各 2 回（63.000 s、handle は redact） |
| native state | **`EXPIRED`**（同一文言）、control **あり**、画像なし |
| screenshot / UIA | `shots/expired-wire-410.png` / `uia/expired-wire-410.json` |

この 410 は client の status mapping（`410 → DescriptorExpired → Expired`）を通り、
client は descriptor を 1 回だけ再取得して再試行し、2 回目の 410 で `Expired` に終端する。

### 3.5 refetch 成功（遅延なし）

| 観測 | 値 |
|---|---|
| 準備 | blob delay flag を解除 |
| 操作 | 同じ control を UIA Invoke |
| Runtime access log | descriptor **200** + blob **200** |
| native state | **`DISPLAYED`**、`メディア画像（記述子のダイジェスト検証済み）…`、refetch control **なし** |
| screenshot / UIA | `shots/displayed-after-refetch.png` / `uia/displayed-after-refetch.json` |
| 独立 protocol 検証 | 新しい credential + observation で descriptor **200**、blob **200**、**1,363,011 B**、`sha256:eea85a24…3908a58`（descriptor 宣言値と一致） |

**digest 一致**: `sha256:eea85a247125e3365b545f606b5527e5ddfa1489729926539c956e3793908a58`
（`protocol_check.digest_matches_descriptor=true`）。

### 3.6 操作の UIA/AX 証跡

3 回の活性化はいずれも UIA InvokePattern で成功した:

```text
matched_name = メディアを再取得 (R)
matched_control_type = ControlType.Button
pattern = invoke
patterns_tried = ["invoke"]
```

## 4. 既定 300 s TTL の実失効（8168、protocol 境界）

TTL 環境変数を設定せずに Runtime を起動（`ttl_env=null`、既定 300 s）。

| 項目 | 値 |
|---|---|
| descriptor advertised remaining | **299.976 s**（`within_expected_window=true`） |
| 直後 blob | **200**、1,363,011 B、0.088 s、`sha256:eea85a24…3908a58`（宣言 digest と一致） |
| 実待機 | **305.0 s**（descriptor mint から） |
| 同一 handle の再提示 | **410**、31 B、`{"code":"MEDIA_HANDLE_EXPIRED"}`、mint から **305.021 s** |
| fresh descriptor + blob | **200 / 200**、1,363,011 B、digest 一致（サーバーは引き続き配信可能） |
| screenshot | なし（protocol-only。native UI は 300 s 待機していない） |

## 5. tests と gate

```text
cargo fmt --all -- --check                                                        # PASS
cargo clippy --locked -p desktop-gpui --all-targets -- -D warnings                # PASS
cargo test --locked -p desktop-gpui                                               # 289 passed / 5 ignored
TMP=<workspace>/target/host-tmp TEMP=<workspace>/target/host-tmp cargo test --locked -p cos-desktop
                                                                                  # 100 + 141 + 4 passed
cargo build --locked --release -p cos-desktop                                     # PASS（binary sha は測定時と同一）
python -B scripts/check_dependency_rules.py --self-test && ... --check            # PASS
```

追加/更新した test:

| 要求 | test |
|---|---|
| Expired/Forbidden だけが control を提示、cold は提示しない | `media_presentation::tests::a_manual_refetch_is_offered_only_for_expired_and_forbidden_outcomes` |
| 1 回の活性化で 1 解決だけ（試行中は再提示しない） | `one_activation_starts_at_most_one_manual_refetch` |
| 成功で明示状態が消える | `a_presented_picture_replaces_the_expired_state` |
| 再失敗は明示状態を維持して再提示 | `a_repeated_failure_keeps_an_explicit_state_and_reoffers_the_control` |
| 開始不能の明示文 | `a_refused_manual_refetch_has_its_own_explicit_sentence` |
| refetch は key の nonce だけを変える | `media_refetch_tests::a_manual_refetch_rekeys_only_the_refetch_nonce` |
| control の配線（Role/aria/キー/UIA/subscribe/handler） | `the_expired_media_state_exposes_one_accessible_refetch_control` |
| refetch 経路が secret を記録しない | `the_manual_refetch_path_logs_no_secret_and_uses_the_live_identity` |

**注（環境）**: この session では native プロセスが `%TEMP%` 配下にディレクトリを作れない
（`std::fs::create_dir` が `Os { code: 5 }`。bash/PowerShell では成功）。`cos-desktop` の
instance/preferences test は `%TEMP%` を使うため、上記は `TMP`/`TEMP` を workspace 内の
書込み可能ディレクトリへ向けて実行した。touched crate `desktop-gpui` は環境依存なしで PASS。
release build の `LNK4044: '/fuse-ld=lld' は無効` は既存のローカル toolchain 設定による
linker warning で、本変更に起因しない。

## 6. evidence と leak scan

- `record-native.json`: native 測定（cold start、Expired、in-flight、wire-410、success、protocol check、cleanup）
- `record-default-ttl.json`: 300 s TTL 測定
- `session.json`: 環境/source/binary/harness hash、ports、measurements（native PASS / default_ttl PASS）
- `uia/`, `shots/`: 6 状態の UIA と screenshot
- `logs/`: sanitized Runtime / Desktop log（token・principal・handle を redact）
- `leak-scan.json`: **clean**（14 text files checked、secret/handle/bearer/principal の finding なし）
- `session.sha256`: 全 evidence file の sha256（`--manifest-only` で再生成）

**screenshot の限界**: `cold-displayed.png` と `displayed-after-refetch.png` は同じ artifact
内容のため画素一致（sha 一致）。refetch の証明は pixel 比較ではなく、refetch 後の
descriptor+blob 200 pair と独立 protocol fetch の digest 一致である。Expired 3 状態
（descriptor delay / 再失敗 / wire-410）の screenshot は control ありで一致し、
`refetch-in-flight.png` のみ control なしで異なる。

## 7. 残課題（honest scope）

- `Forbidden` からの refetch は実装済みだが native 測定していない（401 → Forbidden の
  実状態を作る再認可経路が本測定の範囲外）。
- PREVIEW/THUMBNAIL/METADATA、非 PNG、clock skew は `NOT_MEASURED` のまま。
- native wire-410 は **blob 要求の harness 注入遅延**で作った実サーバー応答であり、
  実時間 60 s 経過後の client 自動要求ではない。300 s の実失効は protocol 境界で測定した。
- restart 後の同一プロセス re-resolution は publication 反映待ちで最大 4 回の surface
  遷移を要した（shell の media sync は key が変わった時だけ解決する。自動 retry はない）。
- native 300 s（UI を 300 s 待たせて Expired 表示）は未測定。protocol の 410 と native の
  60 s Expired が同一 `Expired` 語彙に到達することは本測定で確認した。
