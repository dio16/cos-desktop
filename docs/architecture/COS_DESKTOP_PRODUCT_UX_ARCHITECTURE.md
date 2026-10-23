# COS Desktop Application
## Product / UX / Conceptual Architecture / Technical Architecture Design

**Status:** Target Design  
**Version:** 1.0  
**Target Platform:** Desktop  
**UI Framework:** GPUI  
**Primary Language:** Rust  
**Architecture:** Native Client + Independent COS Runtime Daemon

---

# 1. Product Definition

COS Desktopは、Assigned Creative Taskを入力として、生成・評価・改善・比較・収束までを一つのNative Workbench上で扱うCreative Optimization環境である。

本製品の中心は、生成AIとの会話や個別モデルの操作ではない。

中心となる概念は、

```text
Creative Task
    ↓
Artifact
    ↓
Evaluation
    ↓
Evidence
    ↓
Refinement
    ↓
Iteration
    ↓
Formal Outcome
```

である。

ユーザーは一つのCreative Taskについて、

- 何を作ろうとしているか
- 現在何が生成されているか
- 各候補がどう評価されたか
- なぜ次の改善へ進んだか
- どのようにIterationが変化したか
- 最終的に何が正式成果となったか

を同一WorkBench上で追跡できる。

COS Desktopの役割は、複雑なCreative Optimization Runtimeを、複雑さをユーザーへ押し付けず制御・観察可能にすることである。

---

# 2. Product Goals

最上位目標を次の7項目とする。

1. **Artifact-first**  
   Creative成果物を常に画面の中心に置く。

2. **Task-centric**  
   Session、Model、ProviderではなくCreative Taskを作業単位とする。

3. **Evidence-first**  
   評価結果だけでなく、その判断根拠を確認可能にする。

4. **Refinement-visible**  
   Generate → Evaluate → Refineの過程を理解できる。

5. **Complexity on demand**  
   DMLG、ExecutionPlan、Provenance等の内部構造は必要時のみ開示する。

6. **Native performance**  
   大量Artifact、動画、Graph、Evaluation、履歴を高速・低負荷で扱う。

7. **Runtime independence**  
   UIの終了・クラッシュ・更新によってCreative Taskの実行を失わない。

---

# 3. Non-goals

初期製品では以下を目的としない。

- 汎用ノードベース画像生成エディタ
- ComfyUI互換GUI
- 汎用動画編集ソフト
- Adobe系DCCの代替
- Agent Chat Client
- Model管理画面を中心としたUI
- Provider固有設定を全面露出する管理ツール
- DMLGの自由編集を必須操作とすること
- Desktop UIによるCOS意味論の再実装
- UI独自のCandidateランキング
- UI独自のFormal Outcome決定

COS DesktopはCreative Optimization Workbenchに集中する。

---

# 4. UX North Star

## 4.1 5秒で現在状況が分かる

Taskを開いたユーザーが5秒以内に次を認識できること。

```text
何を作っているか
現在どのIterationか
何が生成されたか
評価結果はどうか
改善は継続中か
問題が存在するか
正式成果が存在するか
```

内部ID、Graph digest、Provider設定等を読まなければ状態が理解できないUIにはしない。

---

## 4.2 成果物を見る時間を最大化する

中央画面の最大面積をArtifactへ与える。

設定・履歴・Evidence・内部GraphはArtifact表示領域を恒常的に圧迫しない。

---

## 4.3 Creative Loopを一続きの体験にする

生成、評価、比較、改善、履歴を別々のツールとして扱わない。

```text
Brief
 ↓
Generate
 ↓
Artifact
 ↓
Evaluate
 ↓
Compare
 ↓
Refine
 ↓
Next Iteration
 ↓
Outcome
```

を一つのTask体験として扱う。

---

## 4.4 バックグラウンド状態変化で操作を奪わない

Iterationが進んでも、

- 表示中Artifact
- Zoom
- Compare状態
- Scroll位置
- Keyboard focus
- 開いているEvidence

を勝手に変更しない。

新しい状態は通知するが、ユーザーの現在作業を奪わない。

---

# 5. Core UX Principles

## 5.1 Artifact-first

中央Surfaceの主役は常にCreative Artifactとする。

```text
Image
Video
Audio
3D
Document
その他将来Artifact
```

Artifact種別に応じてRendererを切り替える。

---

## 5.2 Task-centric

トップレベルの作業単位はCreative Taskである。

```text
CreativeTask
├ Goal
├ Accepted Brief
├ Constraints
├ Iterations
├ Candidates
├ Evaluations
├ Evidence
├ Refinements
├ Lineage
└ Formal Outcome
```

ModelやProviderはTask実行の詳細であり、トップレベルNavigationにはしない。

---

## 5.3 Evidence-first

「良い結果」というラベルだけを見せない。

Evaluationから、

```text
Evaluation
 ↓
Decision
 ↓
Score
 ↓
Evidence
 ↓
Refinement reason
```

へ追跡できること。

---

## 5.4 Progressive Disclosure

第一階層：

```text
Artifact
Evaluation
Iteration
Outcome
```

第二階層：

```text
Evidence
Refinement
Lineage
Accepted Brief
```

第三階層：

```text
DMLG
GraphRevision
ExecutionPlan
Run
Provenance
HarnessVersion
Diagnostics
```

とする。

---

## 5.5 Honest State

推測値を表示しない。

Canonicalな根拠がない場合、

```text
75%
残り2分
ほぼ完了
```

等を生成しない。

Progress percentage、ETA等は正式な根拠が存在する場合だけ表示する。

---

# 6. User Mental Model

ユーザーが理解する概念を次に限定する。

| Concept | 意味 |
|---|---|
| Creative Task | 一つの制作目的 |
| Brief | Goal、Direction、References、Recipe等 |
| Artifact | 生成された画像・動画等 |
| Candidate | Task内のArtifact候補 |
| Iteration | 一回のGenerate-Evaluate-Refine周期 |
| Evaluation | Artifactへの評価 |
| Evidence | Evaluationの根拠 |
| Refinement | 次Iterationへ変更した理由・内容 |
| Flow | Taskがどう実行されたか |
| Outcome | COSが保存した正式結果 |

GraphRevisionやExecutionPlanは高度情報として扱う。

---

# 7. State Model

単一の「状態」へ丸めず、次を独立軸として扱う。

```text
Task Status
ASSIGNED
ACTIVE
COMPLETED
CANCELLED
```

```text
Inner Phase
Prepare
Generate
Evaluate
Select
Refine
Finalize
...
```

```text
Iteration
0
1
2
...
```

```text
Artifact Availability
Available
Pending
Missing
Forbidden
Unsupported
```

```text
Connection
Live
Stale
Offline
Error
```

```text
Formal Outcome
COMPLETED
PARTIAL
FAILED
CANCELLED
```

これらをUI側で勝手に一つのenumへ変換しない。

---

# 8. Candidate Meaning

次の4状態を明確に分離する。

```text
Viewed
現在ユーザーが見ているCandidate

Compare Selected
比較対象として選択したCandidate

Best-so-far
COS Runtimeが現在保持している候補

Formal Selected
Outcomeへ記録された正式成果
```

`Viewed`を変更してもCOSの正式選択には影響しない。

---

# 9. Application Information Architecture

```text
COS Desktop
│
├─ New
│   └─ Creative Brief
│
├─ Active
│   └─ Creative Task Workbench
│       ├─ Artifact Surface
│       ├─ Compare Surface
│       ├─ Flow Surface
│       ├─ Creative Loop Dock
│       └─ Inspector
│
├─ History
│   ├─ Active
│   └─ All
│
└─ Settings
    ├─ Runtime
    ├─ Storage
    ├─ Providers
    ├─ Appearance
    └─ Keyboard
```

管理画面を増やしすぎない。

---

# 10. Main Application Shell

基準Desktop構成：

```text
┌─────────────────────────────────────────────────────────────────────────────┐
│ COS   Task / Project     ACTIVE · REFINE · I3          Live       ■ Stop   │
├──────────────┬──────────────────────────────────────┬───────────────────────┤
│ TASK RAIL    │                                      │ INSPECTOR             │
│              │           WORK SURFACE               │                       │
│ + New        │                                      │ Goal                  │
│              │                                      │ Evaluation            │
│ Active       │             ARTIFACT                 │ Evidence              │
│ > Task A     │                                      │ Refinement            │
│   Task B     │         Image / Video                │ Outcome               │
│              │                                      │                       │
│ History      │                                      │ Accepted Brief   ▸    │
│              │                                      │ Lineage          ▸    │
│              │                                      │ Execution        ▸    │
│              │                                      │ Diagnostics      ▸    │
├──────────────┴──────────────────────────────────────┴───────────────────────┤
│ CREATIVE LOOP                                                               │
│ I0 [A][B][C] ── I1 [D][E] ── I2 [F][G][H] ── I3 [I][J] ── Outcome          │
├─────────────────────────────────────────────────────────────────────────────┤
│ Artifact   Compare   Flow                               Runtime / Status    │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

# 11. Task Rail

左Railの役割はNavigationだけとする。

表示内容：

```text
New
Active
  Task A
  Task B
History
```

Task行には最低限、

```text
Task title
Task status
Iteration
Attention indicator
```

を表示する。

大量Taskではvirtualized listとする。

---

# 12. Creative Brief

新規Task作成時は中央Surface全体をBriefへ利用する。

```text
Goal

Creative Direction

References
[image] [image] [video]

Recipe

Constraints

Advanced ▸

                             Start
```

意味上の順序：

```text
Goal
「何を達成するか」

Direction
「どう表現したいか」

References
「何を参照するか」

Recipe
「どの制作能力を使うか」

Advanced
「必要なtask-local control」
```

内部schema名をそのままUI labelには使用しない。

---

# 13. Accepted Brief

Task受付後のBriefはImmutableとして扱う。

メインSurfaceからは退き、

```text
Inspector
 └ Accepted Brief
```

へ移動する。

Artifact表示を優先しつつ、1操作で確認可能とする。

再利用時のみ新規Briefへコピーする。

元Taskは変更しない。

---

# 14. Artifact Work Surface

Task開始後の中央Surfaceの標準表示。

画像の場合：

```text
┌───────────────────────────────────────┐
│                                       │
│                                       │
│              ARTIFACT                 │
│                                       │
│                                       │
│                              100%     │
└───────────────────────────────────────┘
```

提供する基本操作：

- Zoom
- Pan
- Fit
- 1:1
- Fullscreen
- Open original
- Copy
- Export
- Candidate navigation

動画：

- Native playback
- Seek
- Pause
- Frame step
- Volume
- Fullscreen

将来Artifact種別をRenderer abstractionで追加する。

---

# 15. Creative Loop Dock

COS固有の中心UXとする。

```text
Iteration 0
[A] [B] [C]
      │
      ▼
Iteration 1
[D] [E]
      │
      ▼
Iteration 2
[F] [G] [H]
          │
          ▼
Iteration 3
[I] [J]
      │
      ▼
Formal Outcome
```

Dockは横方向にvirtualizeする。

Candidate thumbnailには、

- Iteration
- Evaluation state
- Formal marker
- Availability
- Current view
- Compare selection

を必要最小限表示する。

Candidate数が増えても全画像をdecodeしない。

---

# 16. Comparison UX

2候補を選択すると同じWork SurfaceがCompareへ切り替わる。

```text
┌────────────────────────┬────────────────────────┐
│ Candidate A            │ Candidate B            │
│                        │                        │
│     Artifact A         │      Artifact B        │
│                        │                        │
├────────────────────────┼────────────────────────┤
│ Evaluation A           │ Evaluation B           │
│ Evidence               │ Evidence               │
└────────────────────────┴────────────────────────┘
```

提供候補：

- synchronized zoom
- synchronized pan
- synchronized video seek
- swap A/B
- change A
- change B
- close and restore previous view

Comparisonは一時表示状態であり、COS canonical stateにはしない。

---

# 17. Inspector

右側Inspectorは「設定」ではなく、Artifactがなぜ現在の位置付けになったかを説明する。

標準構成：

```text
Goal

Evaluation
  Decision
  Score
  Confidence

Evidence
  Summary

Refinement
  Iteration 2 → 3
  Reason

Outcome
  Formal status

Accepted Brief ▸
Lineage ▸
Execution ▸
Diagnostics ▸
```

内部IDやdigestはDiagnosticsへ置く。

---

# 18. Intervention Strip

通常時は表示しない。

ユーザー介入が必要な場合のみWork Surface上部へ出す。

例：

```text
Evaluation unavailable
[Inspect]

Reference artifact missing
[Inspect]

Connection stale
[Reconnect]

Stop requested
[Details]

Formal outcome available
[View]
```

複数問題がある場合は優先度の高い1件を表示し、残り件数から一覧へ移動できるようにする。

---

# 19. Flow Surface

FlowはCreative Task内部を観察するためのSurfaceである。

第一階層：

```text
Goal
 ↓
Generate
 ↓
Evaluate
 ↓
Select
 ↓
Refine
 ↓
Outcome
```

第二階層：

```text
Goal
├ Context
├ Capability
├ Generation
├ Evaluation
├ Feedback
└ Refinement
```

第三階層：

```text
GraphRevision
ExecutionPlan
Run
Evaluation
GraphPatch
Provenance
HarnessVersion
```

DMLGは通常操作を阻害しない。

---

# 20. DMLG UX

初期製品ではDMLGをinspection-firstとする。

主要用途：

```text
現在どのGraphRevisionか
何が変化したか
なぜ変更されたか
どのPlanが生成されたか
どのRunが実行されたか
どのArtifactへ繋がったか
```

GraphRevision差分例：

```text
Revision 12 → 13

PARAM
cfg_scale
4.5 → 5.0

GRAPH
Evaluator B added

PLAN
Recompiled

Reason
composition evidence
```

自由配線Editorは初期必須機能としない。

---

# 21. Command Palette

Command Paletteを第一級操作Surfaceとする。

例：

```text
New creative task
Open active task
Open candidate
Compare selected
Show evaluation
Show evidence
Show flow
Show provenance
Stop task
Reuse task
Toggle inspector
Fullscreen artifact
```

Keyboardのみで主要操作を完了できること。

---

# 22. Conceptual Architecture

最上位構造：

```text
COS Desktop
    │
    ▼
Desktop Application Core
    │
    ▼
Versioned Desktop Protocol
    │
    ▼
COS Daemon
    │
    ▼
COS Domain Core
    │
    ▼
Providers / Workers
```

UIとCOS runtimeの意味論を分離する。

---

# 23. Process Architecture

2プロセスを基本構成とする。

```text
Process 1
COS Desktop
GPUI

Process 2
cosd
Rust Headless Runtime
```

```text
┌──────────────────────┐
│ GPUI Desktop         │
└──────────┬───────────┘
           │
      Typed IPC
           │
┌──────────▼───────────┐
│ cosd                 │
│                      │
│ Command Router       │
│ Projection Engine    │
│ Subscription         │
│ Media Service        │
│ Graph Service        │
│ Persistence          │
└──────────┬───────────┘
           │
┌──────────▼───────────┐
│ COS Domain Core      │
└──────────────────────┘
```

UI終了後も実行中Taskを継続できること。

---

# 24. Process Separation Requirements

Desktop crash:

```text
Task continues
```

Desktop restart:

```text
Reconnect
 ↓
Snapshot restore
 ↓
Continue observation
```

Desktop update:

```text
cosd continues if protocol compatible
```

これにより、

- background execution
- crash recovery
- UI hot restart
- CLI client
- future mobile viewer
- remote client
- multiple window support

への拡張を可能にする。

---

# 25. Rust Workspace Architecture

推奨構成：

```text
crates/

desktop-gpui/
    shell
    task_rail
    work_surface
    artifact
    compare
    flow
    inspector
    loop_dock
    command_palette

ui-model/
    application_view
    task_view
    artifact_view
    evaluation_view
    graph_view
    local_interaction

app-core/
    navigation
    commands
    preferences
    capabilities
    desktop_lifecycle

desktop-protocol/
    command
    snapshot
    event
    capability
    version

cosd/
    command_router
    projection
    subscriptions
    recovery
    graph_service
    media_service

artifact-service/
    metadata
    thumbnails
    previews
    cache
    streaming

provider-adapters/
    generation
    evaluator
    external_runtime
```

既存COS domain cratesは意味の唯一所有者として下位に置く。

---

# 26. Dependency Rule

```text
desktop-gpui
      ↓
ui-model
      ↓
app-core
      ↓
desktop-protocol
```

GPUIから直接、

- Database
- COS Rust semantic type
- Provider
- Graph kernel
- Artifact CAS

を操作しない。

---

# 27. UI State Ownership

UIが所有してよいもの：

```text
selected candidate
compare pair
zoom
pan
scroll
panel width
disclosure state
draft
window geometry
temporary filters
```

UIが所有してはいけないもの：

```text
Task status
Formal Outcome
Evaluation result
Winner
GraphRevision
ExecutionPlan
Artifact identity
Stop completion
```

後者はすべてcosd projectionから取得する。

---

# 28. Command Model

UIからruntimeへの変更要求は明示Commandとする。

```text
CreateTask
StopTask
ReuseTask
RefreshTask
RequestArtifact
RequestComparisonData
...
```

Commandには、

```text
command_id
target identity
expected revision
payload
```

を必要に応じて持たせる。

再送による二重実行を防止する。

---

# 29. Projection Model

cosdはDesktop向けProjectionを提供する。

```text
TaskSnapshot
CandidateSnapshot
ArtifactSnapshot
EvaluationSnapshot
IterationSnapshot
OutcomeSnapshot
GraphSnapshot
```

Desktopはdomain objectそのものを直接読む必要を持たない。

---

# 30. Observation Protocol

基本方式：

```text
Initial Snapshot
      ↓
Delta Stream
      ↓
Delta Stream
      ↓
...
```

各projectionはrevisionを持つ。

```text
snapshot revision = 1042

delta = 1043
delta = 1044
delta = 1045
```

切断時：

```text
Reconnect
 ↓
send last_seen_revision
 ↓
resume delta

or

full snapshot
```

とする。

Desktop用途では定期PollingよりSubscriptionを標準とする。

---

# 31. Optimistic Update Policy

Canonical stateを楽観更新しない。

Stop操作：

```text
User presses Stop

UI:
Stop requested

cosd:
canonical update

UI:
Task state changed
```

即座に`CANCELLED`へ変更しない。

楽観更新可能なのはPresentation stateのみ。

---

# 32. Media Architecture

Control Planeへmedia bytesを流さない。

Projection：

```text
artifact_digest
media_type
dimensions
availability
preview_handle
metadata
```

Data Plane：

```text
Artifact Service
 ↓
thumbnail
preview
original
 ↓
background decode
 ↓
GPU texture
```

IPC JSONへのbase64埋込は禁止する。

---

# 33. Media Loading Policy

Candidate Dock：

```text
thumbnail only
```

中央Artifact：

```text
preview
 ↓
progressive high resolution
```

Fullscreen：

```text
original / high resolution
```

Compare：

```text
2 preview textures
```

必要なArtifactだけdecodeする。

高解像度表示でも入力・選択・比較操作をdecode/upload待ちで停止させない。thumbnail / preview / originalの役割を分け、8K/16K級画像と複数Artifactの比較は可視領域・必要な解像度を優先して提示する。1:1表示は物理pixel基準とし、DPI変更で意味を変えない。色profileの扱いと未対応能力は明示し、色の正確さやvideo同期を未検証のまま保証しない。

具体的なcache・tile・thread・resource設計は[Rendering / Performance / Visual Architecture](RENDERING_PERFORMANCE_VISUAL_ARCHITECTURE.md)へ分離する。技術的な責務・可視領域中心の描画・媒体表現の分離は[ADR-0007](../decisions/ADR-0007-RENDERING-PERFORMANCE-VISUAL-ARCHITECTURE.md)でAcceptedとするが、API・色処理・video連携・性能数値の未検証部分は引き続きProposedとする。設計の確定は実装・性能・媒体対応の確認を意味しない。

---

# 34. Cache Architecture

```text
Artifact Cache

Metadata Cache
Thumbnail Cache
Decoded CPU Cache
GPU Texture Cache
```

各cacheを独立budget管理する。

GPU Texture CacheはLRUを基本とする。

生成RuntimeのVRAMをUIが不必要に圧迫しない。

---

# 35. Virtualization

次をvirtualizeする。

- Candidate Dock
- History
- Task list
- Evidence list
- Logs
- Provenance list
- large Graph
- Diagnostics events

数千件存在しても表示領域周辺だけをmaterializeする。

---

# 36. GPUI Rendering Policy

UI threadの責務：

```text
Input
Layout
Paint
Focus
small state update
```

Background execution：

```text
IPC
serialization
image decode
video decode
thumbnail generation
graph layout
search
filesystem
database
network
```

UI threadをblockしない。

---

# 37. Frame Performance Target

設計目標：

```text
60 Hz
16.67 ms/frame以内

120 Hz target
8.33 ms/frame以内
```

通常interactionでは120Hz-readyを目標とする。

大量データの存在によって入力latencyを悪化させない。

---

# 38. Performance Budgets

初期設計目標：

| Metric | Target |
|---|---:|
| Warm first usable window | ≤ 500 ms |
| Input feedback | next frame |
| UI frame p95 | < 8.33 ms on target hardware |
| Scroll frame p95 | < 8.33 ms |
| Task switch cached | < 50 ms |
| Inspector open | < 16 ms |
| Candidate selection cached | < 16 ms |
| IPC command dispatch overhead | < 5 ms local |
| UI idle CPU | near 0% |
| Long-running task | no UI polling loop |

これらは製品SLAではなく設計・計測targetとする。技術的なcalibration status・予算候補・gate承認は[Rendering / Performance / Visual Architecture](RENDERING_PERFORMANCE_VISUAL_ARCHITECTURE.md) §10が所有する。例として`Warm first usable window ≤ 500 ms`は現時点でstretch investigationでありapproved gateではない（2026-10-08時点のRelease計測はwarm window p95 ≤1.5 s候補、n=30）。本節のproduct-level目標を変更せずに技術的な閾値を調整する場合はcompanionを更新する。

---

# 39. Resource Policy

UIのresource使用量をTask実行量から切り離す。

生成Artifactが1000件存在しても、

```text
1000 full images
```

をmemoryへloadしない。

基本原則：

```text
metadata many
thumbnail visible subset
preview 1–2
original on demand
```

---

# 40. Reliability Architecture

必須Failure scenario：

```text
Desktop crash
cosd crash
provider crash
worker crash
IPC disconnect
disk full
corrupt cache
missing artifact
runtime restart
version mismatch
partial update
```

Task stateとDesktop connection stateを別々に扱う。

Desktop disconnectをTask failureへ変換しない。

---

# 41. Recovery

Desktop再起動：

```text
Launch
 ↓
Connect cosd
 ↓
Protocol negotiation
 ↓
Task snapshot
 ↓
Restore local presentation state
```

cosd再起動：

```text
Load canonical state
 ↓
Recover unfinished task
 ↓
Rebuild projection
 ↓
Reconnect clients
```

presentation cacheはcanonical truthにしない。

---

# 42. Local-first

基本動作はlocal-firstとする。

```text
Desktop
 │
 local IPC
 │
cosd
 │
local CAS / DB / worker
```

Cloud接続なしでも、

- Task history
- Artifact browsing
- Evidence
- DMLG
- local generation capability

が使用可能な構成を許容する。

---

# 43. Future Remote Architecture

Protocol境界を維持することで将来、

```text
Desktop
 ↓
secure remote transport
 ↓
remote cosd
```

へ拡張可能とする。

DesktopのUXとCOS semanticsは変更しない。

---

# 44. Capability Model

機能の有無をbackend名で判定しない。

```rust
struct CreativeCapabilities {
    image_generation: bool,
    video_generation: bool,
    audio_generation: bool,
    references: bool,
    comparison: bool,
    param_refinement: bool,
    graph_refinement: bool,
    publication: bool,
}
```

UIはCapability projectionに基づいて操作を表示する。

---

# 45. Artifact Extensibility

Artifact種別ごとに、

```text
ArtifactRenderer
ArtifactInspector
ArtifactComparer
ArtifactExporter
```

という能力境界を設ける。

例：

```text
Image
Video
Audio
3D
Document
```

を同じWork Surfaceへ追加可能にする。

---

# 46. Evaluation Extensibility

Evaluation UIは特定Evaluatorへ依存しない。

```text
EvaluationProjection

decision
axes[]
confidence?
evidence_count
authority
```

のような汎用projectionを受ける。

Evaluator固有dataは詳細Disclosureへ置く。

---

# 47. Visual Design

方向性：

```text
High-density
Neutral
Static
Fast
Clear hierarchy
Artifact dominant
```

UIはArtifactより目立たない。

美しさと応答性を同時に満たす。英数字・日本語が混在する緻密な文字、sharpなicon、明確なbaseline、可読性を保つ情報密度、Artifact中心の視覚階層、一貫したfocus/selectionを設計要件とする。100% / 125% / 150% / 175% / 200% DPIで文字・境界・hit target・popupが破綻しないことを検証する。装飾によるblur・常時animation・視覚的noiseで品質を代替しない。

Typography / geometry / semantic color / depth / motionの具体値は[技術companion](RENDERING_PERFORMANCE_VISUAL_ARCHITECTURE.md)で検証するProposed案とし、本書のProduct/UX authorityを置き換えない。

避ける：

```text
AI purple
gradient
glow
excess cards
excess borders
constant animations
decorative agent avatars
```

---

# 48. Color

色は状態の意味に使用する。

```text
Running
Passed
Warning
Failed
Pending
Selected
Formal
```

色だけには依存しない。

Text / Icon / Shapeを併用する。

---

# 49. Animation

使用用途：

- Candidate arrival
- Evaluation completed
- Iteration transition
- Outcome committed
- Error appearance
- Panel transition

常時animationは使用しない。

Reduced Motion設定を尊重する。

---

# 50. Keyboard-first

主要操作はKeyboardで完結可能にする。

最低限：

```text
Command Palette
Task switch
Candidate next / previous
Compare selection
Artifact zoom
Inspector toggle
Flow open
Evidence open
Stop
Fullscreen
```

Focus locationを常に可視化する。

---

# 51. Accessibility

必須品質項目：

- Keyboard navigation
- visible focus
- semantic labels
- state not represented only by color
- scalable text
- high contrast
- reduced motion
- screen reader metadata where supported
- logical focus order
- dialogs with focus trapping / restoration

GPUI componentの独自実装時もAccessibilityを設計対象とする。

---

# 52. Desktop Integration

Desktop製品として次を第一級機能とする。

```text
Drag & Drop
Clipboard
Native file picker
Fullscreen
Multiple monitor
High DPI
IME
Native notifications
Deep links
Window restoration
Single-instance handling
Crash diagnostics
File association where useful
```

---

# 53. Window Strategy

MVP：

```text
Single primary window
+
Fullscreen Artifact
```

将来：

```text
Detached Artifact viewer
Detached Compare
Detached Flow
```

Core stateはWindowへ所有させない。

複数Windowは同じprojectionを観察する。

---

# 54. Local Preferences

Desktop local preferenceとして保存可能：

```text
window geometry
panel width
theme
keyboard bindings
last opened task
recent locations
explicitly saved Brief presets
```

保存しない：

```text
canonical Task state
Formal Outcome
Evaluation truth
credential plaintext
automatic prompt history
```

---

# 55. Security

Trust boundaries：

```text
Desktop UI
Desktop Core
IPC
cosd
Provider adapter
External runtime
```

実行操作はcosd側で再検証する。

UIボタンのdisableだけをsecurity boundaryにしない。

---

# 56. Credentials

CredentialsはOS credential storeまたはprovider-owned secure storeを使用する。

Plaintext configuration fileへの保存を標準にしない。

Credential値を、

- logs
- diagnostics
- provenance
- crash reports

へ含めない。

---

# 57. Artifact Security

外部Artifactは、

```text
identity
media type
availability
verification
```

を確認してから表示する。

Artifact metadataをcommandとして実行しない。

---

# 58. Observability

Diagnosticsには、

```text
Desktop version
Protocol version
cosd version
Task ID
Projection revision
Connection state
Last update
Memory cache usage
GPU texture cache usage
Active subscriptions
```

を表示可能にする。

通常UXへは露出しない。

---

# 59. Logging

ログを分類する。

```text
Application log
Runtime log
Provider log
Task event
Diagnostic trace
```

すべてを1つの巨大Terminalへ流さない。

Log levelとretentionを制御する。

---

# 60. Distribution

Windowsを第一対象とした場合：

```text
Signed installer
+
portable archive
```

を提供する。

一般利用者はinstallerを標準とする。

---

# 61. Auto Update

Desktop updaterは、

```text
check
download
verify signature
stage
restart
rollback
```

を持つ。

cosdとDesktopのprotocol compatibilityを確認してから切り替える。

---

# 62. Versioning

最低限、

```text
desktop_version
cosd_version
protocol_version
schema_version
```

を分離する。

Desktopとcosdのversion完全一致を必須にしない。

Protocol compatibility rangeを持つ。

---

# 63. Quality Strategy

テストを意味ごとに分ける。

```text
Domain tests
Protocol tests
Projection tests
ui-model tests
GPUI component tests
Golden Workflow tests
Performance tests
Recovery tests
Accessibility tests
Packaging tests
```

UI screenshotだけで意味論を検証しない。

---

# 64. Golden Creative Workflow

MVPの主要受入Workflow：

```text
New Task
 ↓
Brief入力
 ↓
Start
 ↓
Candidate生成
 ↓
Artifact確認
 ↓
Evaluation確認
 ↓
次Iteration
 ↓
Candidate比較
 ↓
Evidence確認
 ↓
Flow確認
 ↓
Formal Outcome
 ↓
Historyから再Open
```

途中Failure：

```text
Generation failure
Evaluation failure
Missing media
Connection loss
Stop request
Desktop restart
```

を含める。

---

# 65. UX Validation

GPUI本実装前に操作可能Prototypeで以下を確認する。

```text
5秒でTask状態が理解できるか
Artifactが自然に主役になるか
Candidate Loopを理解できるか
ViewedとFormal selectionを誤認しないか
EvaluationからEvidenceへ自然に進めるか
Compareへ迷わず到達できるか
Flowを必要な時だけ使えるか
問題発生時の介入方法が分かるか
```

---

# 66. Performance Validation

最低限計測する。

```text
startup
frame time
input latency
scroll latency
candidate switching
large history
large candidate set
large graph
high-resolution image
video playback
memory growth
GPU memory growth
idle CPU
IPC throughput
reconnect
```

「native」または主観的な速度だけで品質を判定しない。即時の操作応答、安定したframe pacing、低いidle CPU/GPU、メモリの上限、描画品質を同じscenarioで評価する。p50 / p95 / p99、dropの定義、popupを開いた静止状態と閉じた後、60Hz / 120Hz、実際のhardware・DPI・Debug/Release条件を分けて記録する。計測不能はNOT_MEASUREDとし、推定値やfixture結果を実測・Runtime成功として扱わない。

[Rendering / Performance / Visual Architecture](RENDERING_PERFORMANCE_VISUAL_ARCHITECTURE.md)に予算候補・観測設計・段階的移行を、[Tern Binary Rendering Analysis](../development/TERN_BINARY_RENDERING_ANALYSIS.md)に実体・Release COS・reference・一時的な同一GPUI pinの計測証拠と限界を置く。[ADR-0007](../decisions/ADR-0007-RENDERING-PERFORMANCE-VISUAL-ARCHITECTURE.md)は技術アーキテクチャと現行pin維持を確定する。数値budgetと将来のrenderer/pin変更は、必要な比較計測なしにAcceptedへ昇格しない。

---

# 67. Development Sequence

実装順：

```text
1. Product semantics freeze
2. Information Architecture
3. Golden Creative Workflow
4. Low-fi wireframe
5. Visual directions
6. Interactive GPUI prototype
7. UX validation
8. Design System
9. ui-model
10. desktop-gpui shell
11. Artifact Surface
12. Loop Dock
13. Inspector
14. Compare
15. Flow
16. desktop-protocol
17. cosd
18. Media service
19. COS integration
20. Recovery
21. Performance hardening
22. Accessibility
23. Packaging / update
24. Production acceptance
```

---

# 68. MVP Scope

MVPに含める。

```text
New Task
Creative Brief
Task Rail
Artifact Surface
Image / Video
Candidate Dock
Evaluation
Evidence
Compare
Outcome
History
Stop
Flow read-only
DMLG inspection
Command Palette
Crash/reconnect recovery
Signed desktop package
```

後回し：

```text
DMLG free editing
multi-user collaboration
cloud orchestration
third-party UI plugins
large distributed runtime scheduler
3D editor
full video editor
```

---

# 69. Design System Boundary

Design SystemはGPUI widget collectionだけにしない。

次を含む。

```text
Typography
Spacing
Color semantics
Focus
Motion
Density
Iconography
Status grammar
Empty state
Error state
Loading state
Inspector pattern
Work Surface pattern
Dock pattern
Command pattern
```

COS Desktop全体の情報言語として定義する。

---

# 70. Architectural Invariants

以下を恒久的な設計原則とする。

### A. Artifact-first
成果物がUIの中心。

### B. Task-centric
TaskがNavigationと状態の単位。

### C. Evidence-first
判断理由を追跡可能。

### D. Canonical truth outside UI
UIはCOS意味論のownerにならない。

### E. Progressive Disclosure
内部複雑性は必要時のみ表示。

### F. Process-separated
Desktopとruntimeを分離。

### G. Main-thread non-blocking
重い処理を描画threadで実行しない。

### H. Capability-driven
Provider名ではなく能力でUIを構成。

### I. Local-first
ネットワークなしでも可能な範囲を最大化。

### J. Replaceable boundaries
GPUI・Provider・Runtime変更を境界内へ閉じ込める。

### K. Recoverable
Desktop終了・切断・runtime restartでTaskを失わない。

### L. Explainable
Artifact → Evaluation → Evidence → Refinement → Outcomeを逆引き可能。

---

# 71. Final Target Architecture

```text
                         COS DESKTOP
┌────────────────────────────────────────────────────────────────────┐
│ GPUI                                                               │
│                                                                    │
│ Context Bar                                                        │
│ Task Rail ───── Work Surface ───── Inspector                       │
│                     │                                              │
│            ┌────────┼────────┐                                     │
│            │        │        │                                     │
│         Artifact  Compare   Flow                                   │
│                                                                    │
│ Creative Loop Dock                                                 │
│ Command Palette                                                    │
└────────────────────┬───────────────────────────────────────────────┘
                     │
                  ui-model
                     │
                  app-core
                     │
              desktop-protocol
                     │
═════════════════════╪══════════════════════ Process Boundary
                     │
┌────────────────────▼───────────────────────────────────────────────┐
│ COSD                                                               │
│                                                                    │
│ Command Router                                                     │
│ Projection Engine                                                  │
│ Subscription / Recovery                                            │
│ Media Service                                                      │
│ Graph / Lineage Service                                            │
│ Capability Registry                                                │
│ Persistence                                                        │
└────────────────────┬───────────────────────────────────────────────┘
                     │
┌────────────────────▼───────────────────────────────────────────────┐
│ COS DOMAIN                                                         │
│                                                                    │
│ Contract                                                           │
│ Versioned DMLG                                                     │
│ Immutable Execution Plan                                           │
│ Worker Runtime                                                     │
│ Artifact / CAS                                                     │
│ Evaluation                                                         │
│ Evidence                                                           │
│ Refinement                                                         │
│ Provenance                                                         │
│ Knowledge                                                          │
│ Formal Outcome                                                     │
└────────────────────┬───────────────────────────────────────────────┘
                     │
             Provider / Worker Layer
           ┌─────────┼───────────┐
           ▼         ▼           ▼
        Generate   Evaluate   External Runtime
```

---

# 72. Product Identity

COS Desktopは、

> **Creative Artifactを生成するアプリ**

だけではない。

また、

> **Workflow Graphを編集するアプリ**

でもない。

製品の本質は、

> **Creative Taskに対して生成・評価・改善・比較・Evidence・Outcomeまでを一つの高速なNative Workbenchで扱い、Creative Optimizationの過程そのものを観察・理解・制御できるDesktop Environment**

である。

そのため、UIの中心はArtifactとCreative Loopとし、DMLGやExecutionPlanはその背後にある強力な実行・説明基盤として配置する。

これをCOS Desktopの基本設計とする。