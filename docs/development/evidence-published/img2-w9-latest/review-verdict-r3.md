# Tern review — MP-IMG2-W9 Round 3

MP-IMG2-W9 Round 3 — 固定SOURCE独立レビュー

前回の2指摘：両方CLOSED

1. A-W9-R2-01 — Malformed legの入出力・公開状態

判定：CLOSED

現行w9_progressive_verify.pyに対し、前回問題となった8条件を独立再現しました。

負例

	

改変条件

	

現行verify()




44

	

input_bytes削除

	

拒否




45

	

output.bytes=0

	

拒否




46

	

Decoded WICのpublished_success=false

	

拒否




47

	

Decoded WICのoutput_present=false

	

拒否




48

	

probe_result.output_bytes削除

	

拒否




49

	

Refused WICのpublished_success=true

	

拒否




50

	

Refused WICのoutput_present=true

	

拒否




51

	

Refused WICのoutput_bytes=4096

	

拒否

8件すべて、元Native記録を変更したうえで実際のverify()へ投入し、ok=falseを確認しました。

Malformed WIC/GDI+のfixture identity、probe identity、input bytes、decoder outcomeの照合も維持されています。破損入力でdecoderが画像を返しても、DECODED_TAMPER_NOT_VERIFIEDのままで、content verifiedへ昇格していません。

2. A-W9-R2-02 — SOURCE reconciliation訂正

判定：CLOSED

検証項目

	

結果




Native SOURCE

	

2142ebae822e509c63c4877dd3a81415d4923f3f




製品・Gate候補

	

86cc666b2debc95cb470c456b0b24a986bb9b702




Native SOURCEファイル

	

22/22 SHA256一致




Reconciliationの22行

	

22/22一致を記録




Round 1旧比較

	

historical参照として分離




Candidate→Evidence差分

	

15件、すべてdocs-only

Reconciliationのcomputed_againstは、最終候補ではなく中間commit 24acde13…です。ただし、当該ファイル自身がその中間commitと最終候補の差分である旨を明記しており、現候補SOURCE ZIPの22ファイルとの一致も独立に確認しました。

したがって、同じcommit SHAであるという誤った主張はなく、修正されたSOURCE対応の証拠として受理します。Private Git objectによるcommit ancestryの再検証はNOT_RUNです。

3. SOURCE・Evidenceと実装差分

検証項目

	

独立結果




SOURCE ZIP

	

5,777,672 bytes、SHA256一致




Evidence ZIP

	

556,757 bytes、SHA256一致




Git archive識別

	

両方指定commitと一致




SOURCE manifest

	

826/826原バイト一致




Manifest欠落・重複

	

0件




Evidenceファイル

	

46件




Native SOURCE対応

	

22/22 LF SHA256一致




ZIP破損検査

	

両方PASS

前回Round 2のSOURCE ZIPとも比較しました。

既存の非docsファイルで変更されたのは、w9_progressive_verify.pyとtest_w9_progressive_negative.pyの2ファイルのみです。新規の非docsファイルはありません。

製品WIC/GDI+ probe、crates/media、Runtime、W1～W8実装は変更されていません。

4. Progressive JPEG・負例の独立実行

検証項目

	

結果




Fixture・markerテスト

	

8/8 PASS




固定source

	

5件




Decoder legs

	

10件




Progressive JPEG

	

2/2、WIC/GDI+双方でdecode




Multi-scan

	

SOF2、初期DC/ACの2 scan




Differential

	

2/2 exact




Baseline control

	

1件、PASS




Malformed

	

2件、content未検証を維持




元Native構造replay

	

ok=true




SOURCE未検証時のclosure

	

false

保存された出力PNGを独立デコードし、さらに元encoded progressive JPEGをPillowでデコードして全画素比較しました。

Source

	

RGBA bytes/leg

	

WIC・GDI+・Pillow




256×256

	

262,144

	

全画素一致




1024×1024

	

4,194,304

	

全画素一致

これは追加のdifferential evidenceです。Absolute correctnessとは扱いません。

51規定mutationの再投入結果は、非Gitの50件が50/50拒否でした。残る旧SOURCE commit差し替え1件は構造replayでok=trueですが、source_identity_verified=false、closure_eligible=falseです。

この1件のGit SOURCE-bound検証は今回NOT_RUNとします。

5. 新5 Gateの生receipt

Gate

	

Checks

	

State

	

Artifact




Workspace ee2acb40…

	

7/7

	

SUCCEEDED

	

33/33




IMG2 d98dfeaf…

	

9/9

	

SUCCEEDED

	

33/33




JPEG 4046304f…

	

2/2

	

SUCCEEDED

	

33/33




W9 8a35342a…

	

1/1

	

SUCCEEDED

	

33/33




Build 7b129b5e…

	

1/1

	

SUCCEEDED

	

33/33

5 Gateすべてで以下を確認しました。

status/inspect/verifyのexit codeは0。

Candidate commitは86cc666b2debc95cb470c456b0b24a986bb9b702。

Candidate digestはsha256:e8676e2d064ce54ccbc7a6e28666e314b26d1859021799f5b8aebcda4e1fd846。

Request SHA256とterminal state SHA256が、各Gateのinspect/verify間で一致。

5 Gate × 33 artifactの165組がSOURCEのLF正規化実バイトと一致。

owner_alive=false、recovery_required=false。

前回候補のGateを今回の成功として使用していません。

6. Storage・未測定境界

提出Storage記録では、今回の所有対象8件について次を報告しています。

13,240 files

4,243,075,863 logical bytes

削除後の5 Gate verify：SUCCEEDED

削除対象は成功Gateのwork/repo 5件と既知のcheck targets 3件に限定されています。ただし、物理削除と個別パスの実ファイル数は今回の環境では再検証できません。

Build executable digestはNOT_CAPTURED、preflightは成功へ昇格していません。

Private Git objectによるcommit ancestry、Windows WIC/GDI+の再実行、Refineの再実行、GUI検証もNOT_RUNです。

以下は引き続き未測定・未承認です。

全JPEG・全progressive対応、refinement scan全種

Progressive entropy内容の完全検証

製品decoder採用・tolerance、ICC/EXIF policy

16K full decode、true partial/sequential/tiled decode

Scan単位partial materialization、WIC内部scratch

実予約、true concurrent allocation peak、cooperative cancellation

Product numeric memory SLO、full MP-IMG2 Decode Exit

MP-IMG3、Runtime、measured R0、UX/video、main/release

7. LIMITED TECHNICAL COMPLETION判定

前回の2指摘は解消しました。今回の固定SOURCE、Native記録、5 Gate、生receipt、SOURCE manifest、reconciliationの照合範囲に新たな指摘はありません。

MP-IMG2-W9 LIMITED TECHNICAL COMPLETION：承認

承認範囲は、限定progressive JPEG fixtureにおけるWIC/GDI+実decode、multi-scan構造検証、canonical differential比較、malformed fail-closed分類、およびbounded execution evidenceです。

Full MP-IMG2 Decode Exit、MP-IMG3 prerequisite、製品decoder採用等は引き続き未承認です。

最終判定

分類

	

件数




BLOCKING

	

0




ACTIONABLE NON-BLOCKING

	

0




EXCESSIVE

	

0

A-W9-R2-01：CLOSED

A-W9-R2-02：CLOSED

MP-IMG2-W9 LIMITED TECHNICAL COMPLETION：承認

REVIEW COMPLETE

PASS 0 / 0 / 0
