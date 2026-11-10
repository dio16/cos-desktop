# RT3 same-process media session-identity repair — 実測（2026-11-10）

**判定: `PASS`。** 同一 Desktop プロセスで Runtime を clean stop / restart（新 epoch・
新 credential・新 Desktop principal）した後、Artifact surface を再表示すると、media
descriptor は **200**、blob は **200**、native view は **`DISPLAYED`** に復帰し、
`Forbidden`（`画像を利用できません: このセッションにはメディアの閲覧権限がありません。`）
には滞留しなかった。

この測定は `media-expiry-native-latest/defect.json` が記録した欠陥
（descriptor **401** → `FORBIDDEN`、retry なし）の修復測定である。修正 commit は
`e7a3a1044c13ad21ef75ec61930f22b8b54c7263`、測定 binary は
`sha256:ba3552b2443bd8c8ade230376eae43991a713c1ae05c65e56aeefc76844e0da0`
（21,258,240 B、release、`2026-11-10T12:39:00` build）。binary はこの commit と同一の
source（detached worktree `cc4effd` + fix）から build した。

## 1. 欠陥（修復前）

既存 evidence（同一 product source、未修正版）:

| 項目 | 値 |
|---|---|
| pre-fix evidence | `docs/development/evidence-published/media-expiry-native-latest/` |
| `defect.json` | `sha256:9f9a7c1996fd1588fbbf9d92b9f6d4ff41cd862a027928c2b65571d889070b7b` |
| `record-native-expiry.json` | `sha256:c2ca914774c4e111a8689d3acf2b164bfe24b9f335b5861ee189d89d8471dbce` |
| 観測 | descriptor **401**、native **`FORBIDDEN`**、image 非表示、retry なし |

## 2. 原因

`apps/desktop/src/runtime.rs` の media binding が起動時の token と audience を
closure に固定していた。同じ session credential を提示する observation transport は
`CredentialSlot` を持ち、401 後に bounded reauthorization で新 credential を
install できたが、その slot も新 principal も media binding には伝播しなかった。
同じプロセスの Runtime restart は credential registry を失い、Desktop は再接続時に
**新しい credential と新しい principal** を発行するため、token だけの更新では
descriptor の audience 検証が一致しない。

## 3. 修復設計（narrowest session-identity propagation）

- `runtime::SessionIdentity` を 1 つだけ起動時に生成し、次の 4 者が同じ実体を共有する:
  observation transport の `CredentialSlot`、bounded reauthorization (`ReauthHttp`)、
  reconnect installer (`RuntimeReconnector`)、media executor (`MediaHttp`)。
- `MediaHttp` は credential を保持せず、**request 時に** 共有 slot から読む。
- `ReauthHttp` と reconnect は、protected record から読んだ token と authenticated
  principal を `SessionIdentity` に install する。media は次の request/resolution で
  その値を使う。
- `desktop_gpui::media_presentation::MediaRuntime` は audience を `String` で固定せず、
  共有 `MediaAudienceCell` を **resolution 時に** 読む。Shell の media load key は
  server identity / epoch / read context / audience / artifact / digest を含むため、
  epoch と principal の変更がそのまま再解決の trigger になる。
- 401 の取扱い: media の request が 401 を返し、共有 slot がその request で提示した
  値と異なる credential に置き換わっている場合に **1 回だけ** retry する。replacement
  が無い 401 は既存の status mapping（`Unauthorized`）へ返す。media 自身は
  credential を発行・refresh・保存しない（credential authority は既存の
  reauthorization/reconnect の 1 系統のみ）。attempt は request あたり最大 2 回で、
  slot が変わらなければ再送しない。
- descriptor の audience 検証は変更しない（`MediaFetcher` は現在の principal と
  descriptor の `audience` を完全一致で検証し続ける）。token・principal・handle を
  ログ/error/evidence に出さない。

変更 file:

| file | 内容 |
|---|---|
| `apps/desktop/src/main.rs` | 1 つの `SessionIdentity` を生成し、media binding と observation/reconnect に共有 |
| `apps/desktop/src/runtime.rs` | `SessionIdentity`、`media_binding` の live slot/audience、`observation` の install、`ReauthHttp` の token+principal install |
| `apps/desktop/src/reconnect.rs` | reconnect client build が同じ identity を install |
| `apps/desktop/src/media_executor.rs` | `MediaHttp` が request 時に shared slot を読む/401 replacement retry 1 回 |
| `crates/desktop-gpui/src/media_presentation.rs` | `MediaAudienceCell`、`MediaRuntime::audience()`/`audience_cell()`、明示 unavailable 文言 helper |
| `crates/desktop-gpui/src/lib.rs` | principal 不在時の明示 unavailable 表示（stale audience で fetch しない） |

## 4. テスト（関連 crate）

`cargo test --locked -p app-core -p cos-client -p desktop-gpui -p cos-desktop` は
すべて成功。追加/更新した主な test:

| 要求 | test |
|---|---|
| (a) token+principal rotation が resolution に反映 | `runtime::tests::a_media_binding_reads_the_live_session_identity_per_request`、`media_executor::tests::a_rotated_session_credential_is_presented_by_the_next_media_request`、`desktop_gpui::media_presentation::tests::a_reinstalled_principal_is_visible_to_the_next_resolution` |
| (b) reconnect install が media を更新（new audience 受容 / old 拒否） | `reconnect::tests::a_reconnect_install_replaces_the_shared_credential_and_principal`、`runtime::tests::a_reauthorization_installs_the_replacement_principal_for_media`（`MediaFetcher` の audience 完全一致 test は既存 `cos-client`） |
| (c) 401 + fresh credential が既存 retry に乗る | `media_executor::tests::a_media_401_with_a_replaced_session_credential_retries_once`、`a_raced_401_admits_through_the_real_repository_after_one_retry`、`a_media_401_without_a_replacement_is_not_retried` |
| (d) cold start 不変 | `runtime::tests::a_media_binding...` の binding 構築、`MediaBinding::Unavailable` 文言 test、既存 media presenter tests |
| (e) secret 非漏洩 | `media_executor::tests::errors_carry_no_token_path_or_handle_value`、本 evidence の leak scan（後述）、新しい log 行は値を持たない |

実行した gate:

```text
cargo fmt --all -- --check                         # PASS
cargo metadata --locked --no-deps                  # PASS
cargo clippy --locked --workspace --all-targets -- -D warnings   # PASS
cargo test --locked -p app-core -p cos-client -p desktop-gpui -p cos-desktop  # PASS
python -B scripts/check_dependency_rules.py --self-test && python -B scripts/check_dependency_rules.py  # PASS
```

## 5. Native 測定（isolated Runtime, port 8167, discovery-only）

| 項目 | 値 |
|---|---|
| Desktop | detached worktree `C:/tmp/rt3-session-identity`（`cc4effd` + fix `e7a3a10`） |
| Desktop binary | `target/release/cos-desktop.exe`、`sha256:ba3552b2…844e0da0`、21,258,240 B |
| Runtime | `C:/AI/orchestrator`、head `7764b7c4cdc8060bd9934dc72c4dcbab2eb60565` |
| Runtime 構成 | `isolated_runtime.py`、port **8167**、discovery descriptor 経由（endpoint override なし）、per-run `ORCHESTRATOR_AUTH_SECRET` / handle secret、memory のみ |
| DB/CAS | `target/acc1-native/20261109T233420-golden` の fresh copy。DB copy `sha256:597a1c4e…69ff3f8`、11,800,576 B。generation は実行していない |
| task / artifact | `task-ab47119a8bec467a8e128d7b82f1220a`、`sha256:eea85a24…908a58`、1,363,011 B、1024×1024 PNG、ORIGINAL |
| 測定 harness | `apps/desktop/devtools/runtime_integration/run_rt3_session_identity.py` |

### 5.1 手順と結果

| 段階 | 観測 |
|---|---|
| Runtime 1 起動 | epoch `e5e71c5819104becab6527611f23ef08`、server identity `cos-runtime-orchestrator` |
| Desktop 接続・task 選択・artifact 表示 | state **`DISPLAYED`**、`メディア画像（記述子のダイジェスト検証済み）`、viewport control 表示 |
| 表示時 descriptor / blob | descriptor **200**、blob **200**（Runtime access log） |
| Runtime 1 stop | exit 0、port free、discovery 削除 |
| Runtime 2 起動（同一 DB copy） | pid 25156、epoch **`cfe8675403d643cba374a7168a8fce29`**（変更）、discovery 再出現 |
| Desktop reconnect | `POST /api/cos/desktop/credentials` **200** と `POST /api/cos/desktop/observation` **200** を確認。途中の observation 401 → credentials/refresh 401 → credentials 200 → observation 200 も Runtime log に記録 |
| restart 後の media 再取得 | restart 後 **13.034 s** で descriptor **200** + blob **200**（新 epoch/read context/principal の再解決）。media の 401 は **0 件** |
| Artifact 再表示 | `成果物 Ctrl+1` で再表示。task 再選択で task / viewed candidate を一致させ、state **`DISPLAYED`**、`メディア画像（…）` あり、`Forbidden` 文言なし、`閲覧中の候補` は現在の認可情報と一致 |
| screenshot / UIA | `shots/identity-after-restart.png`（before と同一 sha。同じ artifact 内容の復帰。refetch の証明は Runtime access の 200 pair）、`uia/identity-after-restart.json` |
| 独立 protocol check | 新規 credential + observation で descriptor **200**、blob **200**、1,363,011 B、`sha256:eea85a24…908a58`（descriptor 宣言値と一致） |
| cleanup | Desktop exit 0、Runtime clean stop（exit 0）、port free、discovery なし |

### 5.2 修復前 negative（参照）

同一 product source の未修復測定（再実行はせず、公開済み evidence を参照）:
descriptor **401** → native **`FORBIDDEN`**、image 非表示、retry なし。
`defect.json` の sha は §1、observed 値は `record-native.json` の
`prefix_reference` に記録した。

### 5.3 focus / 干渉

cos-desktop は 1 instance のみ（instance guard）。`Ui.invoke` は全 action で成功し、
foreign instance による focus 奪取は観測されなかった。再選択は synthetic click
fallback（`synthetic_wm_lbutton_at_rail_row`）で成功した。

## 6. evidence と leak scan

- `record-native.json`: 測定 record（UIA 名、access status、epoch、reconnect、cleanup）
- `session.json`: 環境/ハッシュ/binary/source
- `uia/`, `shots/`: 前後の UIA と screenshot
- `logs/`: sanitized Runtime / Desktop log（token・principal・handle を redact）
- `hashes.json`, `session.sha256`: 全 evidence file の sha256
- `leak-scan.json`: **clean**（6 text files checked、secret/handle/bearer/principal の finding なし）

## 7. 残課題（honest scope）

- 実時間の TTL 失効（300 s）と native wire-410 → `Expired` は本測定の対象外（既存
  `media-expiry-native-latest` の residual を継承）。
- 同じ artifact 内容の再表示なので、screenshot の pixel 比較だけでは refetch を
  証明できない。refetch の証明は restart 後の descriptor+blob 200 pair と、
  independent protocol fetch の digest 一致である。
- PREVIEW/THUMBNAIL/METADATA、非 PNG、clock skew は `NOT_MEASURED` のまま。
