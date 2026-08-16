# 自動運転ログ／ライブ可視化プレイヤー
# Implementation Design & MVP Development Plan

> **Historical document.** This 2026-07-19 MVP plan describes the removed Domain/Pipeline
> architecture. It is not current implementation guidance. See
> `docs/message_centric_architecture.md` for the source of truth.

> **Implemented MVP scope (2026-07-19):** `crates/` と `apps/` のworkspaceを
> 新baselineとし、Native/Webの単一JPEG `CompressedImage` camera、Webの固定grid/ego
> BEV、Native ROS live cameraを対象とする。`sensor_msgs/msg/Image` rawはアプリ本体では
> 非対応であり、fixture/実環境入力では `tools/ros-fixture` のoffline converterまたは
> bridgeでJPEGへ変換する。ROS callbackではdynamic messageを共通Pipelineへ渡すためCDRを
> 再構成し、その追加copyを `copied_bytes` として計測する。ROS依存は
> `viewer-native` の `ros2-live` featureだけに閉じ込める。

- **Version:** 0.3
- **Date:** 2026-07-19
- **Status:** Implementation-ready draft
- **Primary audience:** コーディングエージェント、実装担当者、アーキテクチャレビュー担当者
- **Purpose:** 合意済みの設計方針を、段階的に実装できる具体的な作業計画へ落とす

---

## 0. この文書の使い方

この文書は、単なる将来構想ではなく、MVPを実装するための作業契約である。

実装時は次の優先順位に従う。

1. **各Phaseで端から端まで動く一本を維持する。**
2. **合意済みの境界を壊さない。**
3. **未確認の性能問題に対する先行最適化をしない。**
4. **必要性が確認されていない汎用化をしない。**
5. **Native first, Web earlyとする。**
6. **Phaseごとの完了条件を満たしてから横へ広げる。**

本書中の表記は以下の意味を持つ。

| 表記 | 意味 |
| --- | --- |
| **MUST** | MVP実装で必ず守る。 |
| **SHOULD** | 特別な理由がなければ守る。 |
| **MAY** | 必要性が確認された場合のみ導入してよい。 |
| **DO NOT** | 初期実装では行わない。 |
| **要議論** | 該当Phaseへ着手する前に決める。MVP開始を止める論点ではない。 |

---

# 1. エグゼクティブサマリ

本システムは、ROS 2 liveデータおよびMCAPログを、Rust、wgpu、eguiで低レイテンシに可視化するプレイヤーである。

最終的な初期製品像は、単一ウィンドウ内に以下を持つ。

- Main 3D Scene
- 2D BEV
- 11台のCamera Wall
- Telemetry
- Driver-control HUD
- Autonomy status
- Timeline / playback controls

ただし実装は、最初から全機能を同時に作らない。本システムはカメラベース自動運転の可視化を主目的とするため、最初の縦切りはLiDARではなくJPEGカメラ1台とする。まずNativeでMCAPからJPEGカメラ1台を表示し、その直後にWebでも同じ経路を成立させる。続いてBEV、11台Camera Wall、Telemetryを育てる。Main 3DとLiDARは重要だが、主に事後の状況確認用途として後続フェーズで追加する。

設計の中心は以下である。

```text
Source-specific input
        |
        v
    RawMessage
        |
        v
 StreamPipeline
        |
        v
   DomainUpdate
        |
        +-------------------+-------------------+------------------+
        |                   |                   |                  |
        v                   v                   v                  v
 Main Scene Store       BEV State          Camera State      Telemetry State
        |                   |                   |                  |
        v                   v                   v                  v
MainSceneSnapshot       BevFrame          Camera frame       Telemetry snapshot
        |                   |                   |                  |
        v                   v                   v                  v
 Main 3D Renderer       BEV Renderer       Camera texture       egui UI
```

全画面を一つの同期済みGlobal Snapshotとして扱わない。表示領域ごとに時刻の意味、同期ポリシー、更新契機を分離する。


## 1.1 Product priority

実装・性能・レビューの優先順位は次のとおりとする。

1. Camera playback path
2. 11-camera wallとfocused camera
3. BEV: ego / future path / detected objects / occupancy grid
4. Telemetry / driver-control HUD / autonomy status
5. Main 3D
6. LiDAR

LiDARは不要ではないが、カメラベース自動運転の主要判断材料ではなく、事後の空間状況確認を補助するデータと位置付ける。したがって、Camera経路を犠牲にしてLiDARの汎用化や最適化を先行してはならない。

---

# 2. Goals / Non-goals

## 2.1 Goals

MVPおよび初期製品は以下を目標とする。

- NativeアプリケーションでローカルMCAPを再生できる。
- Browser/WASMでローカルMCAPを開き、少なくともBEVとJPEGカメラ1台を表示できる。
- 将来的にROS 2 live Sourceを追加できる。
- 11カメラ、ego pose、future path、detected objects、occupancy grid、基本Telemetryを扱える構造にする。
- 5 LiDARを事後確認用のMain 3Dデータとして追加できる構造にする。
- `header.stamp`に相当するmeasurement timeと、プレイヤーが受信したarrival timeを区別する。
- Main 3Dはmeasurement timeで同期する。
- BEV、Camera、Telemetryは現在状態として独立更新する。
- RendererからROS、MCAP、CDR、topic名を分離する。
- BEVを再利用可能な高性能wgpu crateとして実装する。
- ウィンドウresize時にカメラ画像をCPUで再resizeしない。
- データ更新ごとにGPU buffer／textureを再生成しない。
- 処理が遅れた場合に古い表示を延々と消化せず、最新状態へ追従できる余地を持つ。
- ジュニア実装者でもデータ経路を追える、明示的な型とmodule構造にする。

## 2.2 Non-goals

初期実装では以下を行わない。

- グラフ描画エンジン
- 外部グラフツールとのIPC
- 汎用データ配信daemon
- ECS
- 汎用plugin API
- scriptable renderer
- 自由なdock layout
- 任意ROS messageを自動表示する汎用viewer
- 完全決定論的な動画export
- seek地点の完全な状態復元
- 11カメラのWeb性能最適化
- H.264対応をJPEG表示経路より先に実装すること
- すべてのSourceを一つの美しいtraitへ無理に統合すること

---

# 3. アーキテクチャ原則

## 3.1 MVP first

困ることが実測で確認される前に、複雑な機構を導入しない。

初期値の例:

- decode lookahead: `0`
- MCAP seek: コールドseek
- Stream binding: コード固定
- Raw payload: `Vec<u8>`
- Pipeline Factory: 明示的な`match`
- BEV layer: 固定実装
- Camera: JPEG 1台から開始

## 3.2 境界は早く、抽象化は遅く

以下の境界は最初から維持する。

```text
Source -> RawMessage
RawMessage -> StreamPipeline -> DomainUpdate
Domain state -> Presentation model
Presentation model -> Renderer
```

一方、次は必要性が確認されるまで導入しない。

- Sourceの巨大な共通trait
- Pipeline builder Registry
- Renderer plugin trait
- Generic event bus
- `Box<dyn Any>`ベースのStore
- ECS

## 3.3 Native first, Web early

最初のWalking SkeletonはNativeのJPEGカメラ1台で作る。ただし、共有crateはPhase 0からWASM targetでcompile可能に保つ。

Nativeのカメラ経路が通った直後に、Webでも以下を通す。

- ローカルMCAP選択
- Playback
- `sensor_msgs/msg/CompressedImage`相当のJPEGカメラ1台
- 最小BEV

Web対応を最後の移植作業にしない。

## 3.4 表示領域ごとの同期

```text
Main 3D       measurement timeで同期
BEV           arrival timeで各レイヤーの最新を採用
Camera        arrival timeでカメラごとの最新frameを採用
Telemetry     arrival timeで現在値を更新
```

同じウィンドウ内に表示されていても、更新ドメインは別である。

---

# 4. 初期画面構成

```text
+------------------------------------------------------------------------+
| Source | Connection | AUTONOMY STATUS | Delay / Errors                |
+-------------------------------------------+----------------------------+
|                                           | Camera Wall                |
|                                           | fixed 3 x 4 tiles          |
|               Main 3D Scene               | click one to focus         |
|                                           +----------------------------+
|                                           | Telemetry                  |
+----------------------------+--------------+----------------------------+
| 2D BEV                     | Driver-control HUD                        |
+----------------------------+-------------------------------------------+
| Play | Pause | Speed | Timeline | Current time                         |
+------------------------------------------------------------------------+
```

MVPの各段階では、必ずしも全領域を同時に完成させない。

Web Walking Skeletonは以下でよい。

```text
+-----------------------------------------------------------+
| Open MCAP | Play | Pause | Timeline                       |
+----------------------------+------------------------------+
|                            |                              |
|            BEV             |       JPEG Camera 1          |
|                            |                              |
+----------------------------+------------------------------+
```

---

# 5. Platform Strategy

## 5.1 共有するもの

以下はNative/Webで共有する。

- 時刻型
- `RawMessage`
- `StreamDescriptor`
- `StreamBinding`
- `StreamPipeline`
- `PipelineFactory`
- `PipelineSet`
- `DomainUpdate`
- Domain state
- `MainSceneSnapshot`
- `BevFrame`
- Snapshot／Frame Builder
- BEV renderer
- Main 3D rendererの大部分
- Camera texture renderer
- egui widget compositionの大部分

## 5.2 Platform固有とするもの

### Native

- winit event loop
- native wgpu surface生成
- filesystem／mmap
- ローカルMCAP reader
- ROS 2 live Source
- native JPEG／H.264 decoder

### Web

- browser canvas
- Wasm entry point
- File／Blob入力
- HTTP Range reader（後続）
- browser async execution
- Web JPEG／H.264 decoder

## 5.3 依存方向

```text
viewer-core          platform independent
bev-renderer         platform independent wgpu code
viewer-renderer      platform independent wgpu code
viewer-ui            mostly platform independent

viewer-native        depends on shared crates
viewer-web           depends on shared crates
```

共有crateへ以下を入れない。

- `std::fs::File`
- mmap
- rclrs
- blocking I/O前提のSource API
- window handle
- Native codec handle
- 不要な`Send + Sync`制約

---

# 6. 推奨Workspace構成

MVP開始時はcrateを増やしすぎない。

```text
workspace/
  Cargo.toml

  crates/
    viewer-core/
    viewer-renderer/
    bev-renderer/
    viewer-ui/

  apps/
    viewer-native/
    viewer-web/
```

Sourceとdecodeを最初から独立crateにする必要はない。最初は`viewer-core`または各app内部moduleで開始し、Native/Webの重複が明確になってから切り出してよい。

## 6.1 `viewer-core`

```text
src/
  lib.rs
  time.rs
  ids.rs
  raw_message.rs

  playback/
    mod.rs
    clock.rs
    engine.rs

  pipeline/
    mod.rs
    descriptor.rs
    binding.rs
    factory.rs
    set.rs
    compressed_image.rs
    ego_pose.rs
    point_cloud.rs

  domain/
    mod.rs
    update.rs
    scene.rs
    camera.rs
    telemetry.rs
    transform.rs

  state/
    mod.rs
    main_scene.rs
    bev.rs
    camera.rs
    telemetry.rs

  presentation/
    mod.rs
    main_scene.rs
    camera.rs
    telemetry.rs
```

## 6.2 `bev-renderer`

```text
src/
  lib.rs
  frame.rs
  renderer.rs
  view.rs
  style.rs

  gpu/
    mod.rs
    output_target.rs
    buffers.rs
    uniforms.rs
    pipelines.rs

  layers/
    mod.rs
    ego.rs
    trajectory.rs
    objects.rs
    occupancy.rs

  shaders/
    occupancy.wgsl
    segment.wgsl
    box.wgsl
```

## 6.3 `viewer-renderer`

```text
src/
  lib.rs
  main_3d.rs
  camera_texture.rs
  gpu_context.rs
```

Rendererはwindowやsurfaceを所有しない。`Device`、`Queue`、target format等を外から受け取る。

## 6.4 `viewer-ui`

```text
src/
  lib.rs
  status_bar.rs
  timeline.rs
  telemetry.rs
  camera_wall.rs
  driver_control_hud.rs
  layout.rs
```

## 6.5 `viewer-native`

```text
src/
  main.rs
  app.rs
  source/
    mcap.rs
    ros2.rs          # 後続
  decode/
    jpeg.rs
    h264.rs          # 後続
```

## 6.6 `viewer-web`

```text
src/
  lib.rs
  app.rs
  source/
    blob_mcap.rs
    http_mcap.rs     # 後続
  decode/
    jpeg.rs
    h264.rs          # 後続
```

---

# 7. 時刻モデル

## 7.1 型

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct MeasurementTime(pub i64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ArrivalTime(pub i64);

pub struct SampleMeta {
    pub stream_id: StreamId,
    pub measurement_time: Option<MeasurementTime>,
    pub arrival_time: ArrivalTime,
    pub source_sequence: Option<u64>,
    pub frame_id: Option<FrameId>,
}
```

同じnanosecond値でも、measurement timeとarrival timeは別のnewtypeとする。

## 7.2 Measurement time

- 原則としてROS messageの`header.stamp`。
- センサー間同期に使う。
- 計測時点のego pose／TFを求めるために使う。
- Main 3DのScene表示時刻に使う。
- headerが存在しない場合は`None`。
- arrival timeへの暗黙fallbackは行わない。

## 7.3 Arrival time

### ROS live

プレイヤーのSource callbackがメッセージを受け取った直後に現在時刻を打刻する。

```text
ROS callback entry
    -> arrival timeを即時打刻
    -> queue
    -> decode
```

Decode完了時刻やapplication threadへ届いた時刻をarrival timeにしない。

### MCAP replay

MCAPに記録されたreceived timeを使用する。ファイルから読み出した実PC時刻は使用しない。

## 7.4 Processing time

キュー待ち、decode、GPU upload、render等の性能計測には`std::time::Instant`相当を別途使う。データ同期には使用しない。

---

# 8. Playback Model

Playbackは次の二つの境界で考える。

```text
1. どこまでデータを持ってくるか
   fetch/decode horizon

2. 今どこまで表示へ公開するか
   playback cursor
```

```text
MCAP timeline
    ----------------------|-------------|------------------>
                          ^             ^
                    playback cursor   fetch horizon
```

## 8.1 PlaybackClock

```rust
pub struct PlaybackClock {
    cursor: ArrivalTime,
    speed: f64,
    playing: bool,
}
```

責務:

- play / pause
- speed
- wall timeからcursorを進める
- seek時にcursorを変更する

担当しないこと:

- MCAP reader
- decode
- Store更新
- drop policy
- seek時の状態復元

## 8.2 McapPlaybackSource

初期公開APIは高レベルに保つ。

```rust
pub struct McapPlaybackSource {
    // reader and internal lookahead
}

impl McapPlaybackSource {
    pub fn read_until(
        &mut self,
        end: ArrivalTime,
        output: &mut Vec<RawMessage>,
    ) -> Result<ReadStatus, SourceError>;

    pub fn seek(
        &mut self,
        target: ArrivalTime,
    ) -> Result<(), SourceError>;
}
```

`peek`／`pop`はSource内部の実装詳細であり、公開APIにしない。

## 8.3 MVP playback loop

MVPではdecode lookaheadを0にする。

```rust
let cursor = playback_clock.cursor();
let fetch_until = cursor; // MVP: lookahead = 0

source.read_until(fetch_until, &mut raw_messages)?;

for raw in raw_messages.drain(..) {
    pipelines.decode(raw, &mut domain_updates)?;
}

for update in domain_updates.drain(..) {
    state.apply(update, &mut dirty_state);
}
```

## 8.4 Decode staging

Decode遅延が問題になった場合のみ有効化する。

```text
PlaybackClock
   -> cursor + lookaheadまでRawMessageを取得
   -> 先行decode
   -> Decoded staging
   -> arrival_time <= cursorになったものだけrelease
```

```rust
pub struct DecodedEnvelope {
    pub generation: SourceGeneration,
    pub arrival_time: ArrivalTime,
    pub updates: Vec<DomainUpdate>,
}
```

最初から複雑なworker poolやpriority queueを作らない。APIはlookaheadを後から追加できる形に留める。

## 8.5 Cold seek

初期版のseekは状態復元を行わない。

```text
seek
  -> generationを更新
  -> pending job/stagingを無効化
  -> dynamic stateをclear
  -> dynamic renderer availabilityをclear
  -> source.seek(target)
  -> clock.seek(target)
  -> target以降の新しいデータで再構築
```

seek後、更新頻度の低いデータがしばらく表示されなくても許容する。

古い値を表示し続けず、以下を明示する。

- `WaitingForData`
- `WaitingForCameraFrame`
- `No occupancy data after seek`

Static map、vehicle dimensions、camera calibration、fixed extrinsics等はdynamic stateと分離し、seek後も保持してよい。

---

# 9. Source Boundary

ROS liveとMCAP playbackを無理に同一traitへ統合しない。

```text
ROS live callback ----------------------+
                                        |
MCAP PlaybackClock -> read_until() -----+--> RawMessage
```

共通化は`RawMessage`以降で行う。

## 9.1 RawMessage

MVPでは所有権最適化を先行しない。

```rust
pub struct RawMessage {
    pub stream_id: StreamId,
    pub arrival_time: ArrivalTime,
    pub payload: Vec<u8>,
}
```

将来必要なら`bytes::Bytes`、mmap slice、shared bufferへ変更する。

Sourceの責務:

- stream catalogを提供する。
- arrival timeを確定する。
- raw payloadを出力する。
- MCAPでは`read_until()`と`seek()`を提供する。

Sourceが担当しないこと:

- header.stamp抽出
- semantic role判定
- TF解決
- Scene同期
- Renderer更新

---

# 10. Stream Pipeline

## 10.1 公開契約

```rust
pub trait StreamPipeline {
    fn decode(
        &mut self,
        message: RawMessage,
        output: &mut Vec<DomainUpdate>,
    ) -> Result<(), DecodeError>;
}
```

MVPでは不要な`Send + Sync`制約を付けない。Worker化するPhaseで必要になったら追加する。

## 10.2 Pipeline Factory

Source open時にstream専用Pipelineを構築する。

```rust
pub struct StreamDescriptor {
    pub stream_id: StreamId,
    pub topic: Arc<str>,
    pub schema_name: Arc<str>,
    pub message_encoding: MessageEncoding,
    pub schema: Option<Vec<u8>>,
}
```

```rust
pub enum StreamBinding {
    EgoPose,
    DetectedObjects,
    PredictedPath,
    OccupancyGrid,
    PointCloud { sensor_id: SensorId },
    Camera { camera_id: CameraId },
    Telemetry { mapping: TelemetryMappingId },
    Transform,
    Ignore,
}
```

```rust
impl PipelineFactory {
    pub fn build(
        &self,
        descriptor: &StreamDescriptor,
        binding: &StreamBinding,
    ) -> Result<Box<dyn StreamPipeline>, PipelineBuildError>;
}
```

Factoryの目的:

- schema、encoding、bindingの組み合わせを初期化時に検証する。
- stream固有のfield layoutやcamera ID等をPipelineへ閉じ込める。
- 実行時の分岐を`stream_id -> pipeline`だけにする。
- scratch bufferや解析済みschemaをstreamごとに保持する。

初期版ではPipeline Registryを作らない。明示的な`match`でよい。

## 10.3 PipelineSet

```rust
pub struct PipelineSet {
    pipelines: HashMap<StreamId, Box<dyn StreamPipeline>>,
}
```

実行中にtopic文字列やschema名を再判定しない。

## 10.4 二段階処理

概念上は以下の二段階である。

```text
Raw bytes
    -> wire decode
型付きmessage相当
    -> semantic adapter
DomainUpdate
```

ただし、全Pipelineをgenericな共通structへ無理に押し込まない。

- 単純なPipelineは専用structでよい。
- Wire decodeとsemantic conversionが分離しやすい場合のみ内部で分ける。
- 外からclosureを注入するピュアな設計にはこだわらない。

Pipelineが保持してよいもの:

- schema解析結果
- CDR layout
- point cloud field offset
- camera ID／sensor ID
- scratch buffer
- decode counter

Pipelineが保持してはいけないもの:

- 他streamの最新値
- ego pose history
- TF history全体
- playback cursor
- freshness
- Renderer state

---

# 11. DomainUpdate

```rust
pub enum DomainUpdate {
    Scene(SceneUpdate),
    Camera(CameraUpdate),
    Telemetry(TelemetryUpdate),
    Transform(TransformUpdate),
}

pub struct Sample<T> {
    pub meta: SampleMeta,
    pub value: T,
}
```

`DomainUpdate`は単なるdecoded messageではなく、アプリケーション上の意味が付いた中間状態である。

## 11.1 SceneUpdate

```rust
pub enum SceneUpdate {
    EgoPose(Sample<EgoPose>),
    DetectedObjects(Sample<Arc<DetectedObjects>>),
    PredictedPath(Sample<Arc<PredictedPath>>),
    OccupancyGrid(Sample<Arc<OccupancyGrid>>),
    PointCloud {
        stream_id: StreamId,
        sample: Sample<Arc<PointCloudFrame>>,
    },
}
```

## 11.2 CameraUpdate

JPEG MVPでは以下を基本とする。

```rust
pub enum ImageEncoding {
    Jpeg,
}

pub struct EncodedCameraFrame {
    pub camera_id: CameraId,
    pub meta: SampleMeta,
    pub encoding: ImageEncoding,
    pub data: Vec<u8>,
}

pub enum CameraUpdate {
    EncodedImage(EncodedCameraFrame),
}
```

## 11.3 TelemetryUpdate

```rust
pub enum TelemetryUpdate {
    Values(TelemetryBatch),
    AutonomyState(Sample<AutonomyState>),
    DriverControl(Sample<DriverControlState>),
}
```

RevisionはStoreが値を採用した時点で発行する。FreshnessはStore／Builder／UI policyが判断する。

---

# 12. Domain Stateと同期ドメイン

## 12.1 Main 3D

### 時刻

Scene表示時刻は、最新commit済みego poseのmeasurement timeとする。

```text
scene_time = latest ego pose measurement_time
```

### 選択

- objects: `latest_before(scene_time)`
- future path: `latest_before(scene_time)`
- occupancy: `latest_before(scene_time)`
- point cloud: streamごとに`latest_before(scene_time)`
- scene timeより未来の値: 保持するが描画しない

### Buffer

```rust
pub struct MainSceneStore {
    pub ego_poses: PoseHistory,
    pub objects: MeasurementPair<Arc<DetectedObjects>>,
    pub predicted_paths: MeasurementPair<Arc<PredictedPath>>,
    pub occupancy: MeasurementPair<Arc<OccupancyGrid>>,
    pub point_clouds: HashMap<StreamId, MeasurementPair<Arc<PointCloudFrame>>>,
}
```

Ego pose以外は原則2件だけ保持する。

2件は、最新値がscene timeより未来でも一つ前を選べる最小構成である。

Ego poseだけは時間幅で保持する。初期候補は1秒だが、値は実測後に調整する。

### Pose interpolation

各センサー時刻に対するego poseのみ内挿する。

- 並進: linear interpolation
- 2D yaw: shortest-angle interpolation
- 3D rotation: quaternion slerp
- 外挿: 行わない

センサーデータそのものは内挿しない。

### Update trigger

Main 3D Snapshotはego pose更新後に生成する。

複数ego poseが一度に処理された場合、すべての中間Snapshotを作らず最新poseへ追従する。

## 12.2 BEV

BEVは常にego-centeredの現在状態表示とする。

- objects/path/gridをレイヤーごとに最新1件保持する。
- arrival timeで最新を採用する。
- 入力がego-localならego pose更新はトリガーにしない。
- map/odom/sensor frameからego-localへ変換する必要がある場合のみmeasurement time時点のpose／TFを使う。
- 過去データの蓄積表示は行わない。

```rust
pub struct BevState {
    pub objects: LatestArrived<Arc<DetectedObjects>>,
    pub future_path: LatestArrived<Arc<PredictedPath>>,
    pub occupancy: LatestArrived<Arc<OccupancyGrid>>,
}
```

## 12.3 Camera

- cameraごとに最新decode済みframe 1件を保持する。
- arrival timeで採用する。
- Main 3D／ego poseを待たない。
- decode完了順が逆転した場合、古いarrival timeのframeで表示を巻き戻さない。

## 12.4 Telemetry/HUD

- 項目ごとに最新1件を保持する。
- arrival timeで更新する。
- stale判定もarrival time基準とする。
- Main 3D上にHUDとして重ねても、データ更新はTelemetry domainに従う。

---

# 13. MainSceneSnapshot

```rust
pub struct MainSceneSnapshot {
    pub sequence: SnapshotSequence,
    pub display_time: MeasurementTime,
    pub ego: Option<SnapshotItem<Arc<EgoState>>>,
    pub objects: Option<SnapshotItem<Arc<DetectedObjects>>>,
    pub predicted_path: Option<SnapshotItem<Arc<PredictedPath>>>,
    pub occupancy: Option<SnapshotItem<Arc<OccupancyGrid>>>,
    pub point_clouds: Arc<[PointCloudSceneItem]>,
}
```

```rust
pub struct SnapshotItem<T> {
    pub source_time: MeasurementTime,
    pub revision: Revision,
    pub freshness: Freshness,
    pub value: T,
}
```

原則:

- 全画面状態ではなくMain 3D専用契約である。
- 大容量値は`Arc`で共有する。
- 点群全体をSnapshot作成ごとにCPU座標変換しない。
- sensor frameからSceneへのtransformを保持し、GPU側で適用する。
- Rendererはfreshness thresholdを決定しない。

---

# 14. BevFrame

`BevFrame`は`MainSceneSnapshot`と分離する。

```rust
pub struct BevFrame {
    pub sequence: FrameSequence,
    pub ego: BevEgo,
    pub objects: Option<Versioned<Arc<[BevObject]>>>,
    pub future_paths: Option<Versioned<Arc<[BevPath]>>>,
    pub occupancy: Option<Versioned<Arc<BevOccupancyGrid>>>,
}
```

```rust
pub struct Versioned<T> {
    pub measurement_time: Option<MeasurementTime>,
    pub arrival_time: ArrivalTime,
    pub revision: Revision,
    pub value: T,
}
```

`BevFrame`の契約:

- BEV描画に必要な情報だけを持つ。
- GPU backendには依存しない。
- 全入力は同じ2D座標契約へ変換済み。
- 単位はmeter。
- +Xは前方、+Yは左。
- yaw正方向は反時計回り。
- object reference pointを明記する。
- GPU vertex、instance、texture formatを公開しない。

変換経路:

```text
Domain/BEV state
    -> BevFrameBuilder
    -> BevFrame
    -> BevRenderer::sync()
    -> GPU resources
```

`BevFrameBuilder`はrevisionを見て変化していない配列を再変換しない。

---

# 15. BEV Renderer

## 15.1 責務

- ego
- future path
- detected objects
- occupancy grid

をwgpuでoffscreen textureへ描画する。

担当しないこと:

- ROS／MCAP
- CDR decode
- TF探索
- 時刻同期
- egui layout
- Source control

## 15.2 API

```rust
pub struct BevRenderer {
    // private GPU resources
}

impl BevRenderer {
    pub fn new(
        device: &wgpu::Device,
        target_format: wgpu::TextureFormat,
        initial_extent: RenderExtent,
    ) -> Result<Self, BevRendererError>;

    pub fn resize(
        &mut self,
        device: &wgpu::Device,
        extent: RenderExtent,
    );

    pub fn sync(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &BevFrame,
    ) -> Result<(), BevRendererError>;

    pub fn render(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &BevView,
        style: &BevStyle,
    ) -> Result<(), BevRendererError>;

    pub fn output_view(&self) -> &wgpu::TextureView;
}
```

`sync()`と`render()`を分離する。

- データ更新時だけ`sync()`でGPU uploadする。
- 視点変更／再描画時は既存resourceで`render()`する。

## 15.3 GPU表現

### Occupancy grid

- 1 cell 1 byteのR8系texture
- CPUでRGBA化しない
- fragment shaderで色変換
- 1枚のquadとして描画

### Detected objects

- 固定box mesh
- instance buffer
- 物体ごとに個別vertex bufferを作らない

### Ego

- object pipelineのmeshを再利用可能
- style／draw callは分離してよい

### Future path

- segment instance
- vertex shaderで太さを持つquadへ展開
- platform依存のline widthへ依存しない

## 15.4 Resize

ウィンドウ／パネルresize時:

- input dataは変更しない
- layer bufferは変更しない
- offscreen targetを必要時のみ再生成する
- view uniformを更新する

## 15.5 Layer abstraction

初期版で公開plugin traitを作らない。

```rust
pub struct BevRenderer {
    occupancy: OccupancyLayer,
    trajectory: TrajectoryLayer,
    objects: ObjectLayer,
    ego: EgoLayer,
}
```

共通性が実装から確認された後でprivate traitを抽出する。

---

# 16. Camera Architecture

## 16.1 初期対象

実データには以下が存在する。

- `CompressedImage`相当のJPEG
- H.264

Cameraは本製品の最優先データ経路である。最初はJPEG 1ストリームで、MCAPからdecode、GPU texture、表示までをNative/Webの両方で成立させる。H.264は同じCamera state／texture表示経路を再利用する後続マイルストーンとする。

## 16.2 Pipelineとdecoderの境界

```text
RawMessage
    -> CompressedImagePipeline
       header.stamp / format / JPEG payloadを抽出
    -> EncodedCameraFrame
    -> Platform JPEG decoder
    -> DecodedCameraFrame
    -> CameraTextureSlot
    -> Camera panel
```

`CompressedImagePipeline`はJPEG decodeを行わない。

## 16.3 JPEG input

```rust
pub struct EncodedCameraFrame {
    pub camera_id: CameraId,
    pub meta: SampleMeta,
    pub encoding: ImageEncoding,
    pub data: Vec<u8>,
}
```

`format`文字列の表記揺れはPipelineで正規化する。

## 16.4 Decoded frame

MVPではCPU RGBA bufferでよい。

```rust
pub struct DecodedCameraFrame {
    pub camera_id: CameraId,
    pub measurement_time: Option<MeasurementTime>,
    pub arrival_time: ArrivalTime,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub stride: u32,
}
```

Zero-copyやplatform frame handleは性能問題が確認された後に検討する。

## 16.5 Latest frame policy

- decode前pending JPEGはcameraごとに最新へcoalesceしてよい。
- decode済みframe採用時はarrival timeを比較する。
- 古いdecode結果で画面を巻き戻さない。

## 16.6 GPU texture

```rust
pub struct CameraTextureSlot {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    extent: wgpu::Extent3d,
    uploaded_revision: Revision,
}
```

MUST:

- 同解像度ならtextureを再生成しない。
- frame更新時は既存textureへuploadする。
- パネルresize時にJPEGを再decodeしない。
- パネルresize時にCPU画像resizeを行わない。
- GPU samplerでタイル矩形へ縮小／拡大する。

## 16.7 Camera Wall

製品初期版では11台を固定3x4配置する。

- 位置は固定し、選択でタイル順を入れ替えない。
- クリックした1台をCamera Wall内部で拡大する。
- 通常品質とfocused品質の切替は、visibilityとは別commandにする。
- 11台Web対応はMVP外。

## 16.8 H.264

JPEG表示パス成立後に追加する。

再利用するもの:

- `CameraUpdate`
- `CameraState`
- `CameraTextureSlot`
- Camera panel
- arrival time採用policy

追加するもの:

- access unit解析
- codec configuration
- decoder state
- keyframe handling
- seek後decoder reset
- dependent frameのdrop policy

---

# 17. UIとVisibility

表示切り替えの意味はdraw ON/OFFに限定する。

```rust
pub enum UiCommand {
    SetLayerVisible(LayerId, bool),
    SelectFocusedCamera(Option<CameraId>),
    Seek(ArrivalTime),
    SetPlaying(bool),
    SetPlaybackSpeed(f64),
}
```

Visibility変更で以下を暗黙に行わない。

- subscription解除
- decode停止
- buffer破棄
- Sourceのstream停止

性能問題が確認された場合のみ、別概念として追加する。

```rust
pub enum ResourcePolicy {
    KeepWarm,
    Throttle,
    Suspend,
}
```

## 17.1 Telemetry

固定行として表示する。

- speed
- acceleration
- yaw rate
- gear
- autonomy state
- steering
- accel
- brake
- source age

Telemetry current valuesとEvent Logを混ぜない。

## 17.2 Autonomy state

単純なboolにしない。

```rust
pub enum AutonomyState {
    Manual,
    Standby,
    Engaging,
    Active,
    Disengaging,
    Emergency,
    Fault,
    Unknown,
    Stale,
}
```

UI側で複数topicを組み合わせて判定しない。Semantic adapter／aggregator側で意味を確定する。

---

# 18. Application StateとUpdate Loop

## 18.1 最小所有構造

```rust
pub struct ViewerApp {
    playback: PlaybackEngine,
    pipelines: PipelineSet,

    state: DomainState,
    presenters: PresentationBuilders,

    renderer: AppRenderer,
    dirty: DirtyState,

    raw_messages: Vec<RawMessage>,
    domain_updates: Vec<DomainUpdate>,
}
```

```rust
pub struct DomainState {
    pub main_scene: MainSceneStore,
    pub bev: BevState,
    pub cameras: CameraState,
    pub telemetry: TelemetryState,
}
```

```rust
pub struct DirtyState {
    pub main_scene: bool,
    pub bev: bool,
    pub cameras: CameraDirtySet,
    pub telemetry: bool,
    pub ui: bool,
}
```

## 18.2 Update

```rust
fn update(&mut self, wall_dt: Duration) -> Result<(), AppError> {
    self.playback.clock.advance(wall_dt);

    let cursor = self.playback.clock.cursor();
    let fetch_until = cursor + self.playback.decode_lookahead;

    self.playback.source.read_until(
        fetch_until,
        &mut self.raw_messages,
    )?;

    for raw in self.raw_messages.drain(..) {
        self.pipelines.decode(raw, &mut self.domain_updates)?;
    }

    // MVPではlookahead=0なので即適用。
    for update in self.domain_updates.drain(..) {
        self.state.apply(update, &mut self.dirty);
    }

    if self.dirty.main_scene {
        self.presenters.main_scene.rebuild(&self.state.main_scene)?;
    }

    if self.dirty.bev {
        self.presenters.bev.rebuild(&self.state.bev)?;
    }

    self.renderer.sync_changed(
        &self.presenters,
        &self.state,
        &mut self.dirty,
    )?;

    Ok(())
}
```

## 18.3 Render

```rust
fn render(&mut self) -> Result<(), RenderError> {
    self.renderer.render()
}
```

State／GPU resource更新と、視点変更による再描画を分ける。

---

# 19. Threading and Queues

MVPでは複雑なthreadingを導入しない。

初期:

- MCAP read: application側から同期的に呼ぶ
- generic decode: application threadでも可
- JPEG decode: platform実装に応じて非同期でもよい
- GPU resource: render/application thread
- State mutation: application thread

後から追加可能:

```text
Source/IO
  -> bounded raw queue
  -> per-stream decode workers
  -> decoded staging
  -> application thread
```

worker化する場合も以下を守る。

- generationの異なる結果をStoreへ適用しない。
- 同じstreamのstateful Pipeline処理順を守る。
- camera decodeがMain 3DやTelemetryを止めない。
- queueを無制限にしない。

---

# 20. Error Handling / Diagnostics

## 20.1 Pipeline build error

- 必須stream: open失敗
- optional stream: 無効化して警告

## 20.2 Message decode error

- アプリ全体を停止しない。
- messageを破棄する。
- stream別counterを増やす。
- ログをrate limitする。
- 不正payloadでpanicしない。

## 20.3 Availability

```rust
pub enum DataAvailability {
    WaitingForData,
    Available,
    Stale,
    Error,
}
```

seek後やデータ欠落時に古い値を正しそうに見せない。

## 20.4 最低限のmetrics

- MCAP read時間
- messages read per frame
- Pipeline decode時間／件数
- JPEG decode時間
- Store apply時間
- Snapshot／BevFrame build時間
- GPU upload bytes／回数
- BEV render時間
- Main 3D render時間
- Camera upload時間
- playback cursorと実表示arrival timeの差
- dropped／coalesced message数

初期はログ出力や簡単なdebug panelでよい。

---

# 21. Performance Principles

MUST:

- データ更新ごとにGPU buffer／textureを再生成しない。
- Snapshot生成ごとに大容量配列をコピーしない。
- 点群全体をCPUで毎Snapshot座標変換しない。
- occupancyをCPUでRGBA化しない。
- カメラ表示サイズに合わせたCPU resizeをしない。
- 古いframe backlogを無制限に蓄積しない。
- seek generation変更後の古い結果を捨てる。

SHOULD:

- revision不変ならGPU uploadを省略する。
- object/pathはinstance bufferを使用する。
- camera pending JPEGは最新へcoalesce可能にする。
- buffer capacityを再利用する。

DO NOT:

- 最初からzero-copy抽象化を作る。
- 最初からGPU上のpoint cloud decodeへ進む。
- 最初から11台Web cameraを最適化する。
- 実測前に複雑なworker schedulerを作る。

---

# 22. MVP Definition

## 22.1 Native MVP

Native MVPは以下を満たす。

1. ローカルMCAPを開ける。
2. PlaybackClockでplay/pause/speedを制御できる。
3. `read_until(cursor)`でarrival time到達済みデータだけを処理する。
4. `CompressedImage`相当のJPEGカメラ1台が以下を通る。

```text
MCAP -> RawMessage -> CompressedImagePipeline
     -> EncodedCameraFrame -> JPEG Decoder
     -> DecodedCameraFrame -> CameraState
     -> CameraTextureSlot -> Camera panel
```

5. measurement timeとarrival timeを保持できる。
6. topic文字列判定、ROS型、JPEG container decodeがRendererに残っていない。
7. resize時にCPU画像resizeを行わない。
8. 同解像度frameでGPU textureを再生成しない。
9. cold seekを実行でき、seek後は次のJPEGまでWaitingForDataを表示する。
10. 共有crateがWASM targetでcompileできる。

## 22.2 Web MVP

Web MVPは以下を満たす。

1. ブラウザでローカルMCAPを選択できる。
2. playback／pauseを実行できる。
3. `CompressedImage`相当のJPEGカメラ1台を表示できる。
4. JPEG経路がNativeと同じ`RawMessage`、Pipeline、DomainUpdate、Camera stateを使用する。
5. 最小BEVをwgpuでリアルタイム描画できる。
6. seek後にcamera stateをclearし、次のJPEGから復帰できる。
7. resizeしてもJPEG再decode／CPU resizeをしない。
8. 同解像度frameでGPU textureを再生成しない。

## 22.3 Initial Product Completion

MVP後の初期製品完成条件は、優先順に以下とする。

- Native Camera Wall 11台
- focused camera拡大
- BEV: ego/path/objects/occupancy
- Telemetry固定行
- driver-control HUD
- autonomy status
- ROS 2 live Source
- Main 3Dのego pose measurement-time同期
- 5 LiDAR対応
- cold seek
- basic diagnostics

---

# 23. Development Phases

各Phaseは必ず実行可能な状態で終了する。

## Phase 0: Workspaceと共有境界

### 目的

Native実装を始めながら、Web対応を壊さない骨格を作る。

### 実装

- workspace作成
- `viewer-core`
- `viewer-renderer`
- `bev-renderer`
- `viewer-ui`
- `viewer-native`
- `viewer-web`
- `MeasurementTime`
- `ArrivalTime`
- `RawMessage`
- IDs
- shared crateのWASM check

### 完了条件

```bash
cargo check --workspace
cargo check -p viewer-core --target wasm32-unknown-unknown
cargo check -p bev-renderer --target wasm32-unknown-unknown
cargo check -p viewer-renderer --target wasm32-unknown-unknown
```

が通る。

### 実装しない

- ROS live
- Camera decode
- H.264
- IPC
- Plugin

---

## Phase 1: Native JPEG Camera Walking Skeleton

### 目的

最優先データであるカメラについて、ローカルMCAPからGPU表示までの経路を端から端まで通す。

### 経路

```text
McapPlaybackSource
  -> RawMessage
  -> CompressedImagePipeline
  -> DomainUpdate::Camera::EncodedImage
  -> JPEG Decoder
  -> DecodedCameraFrame
  -> CameraState
  -> CameraTextureSlot
  -> Camera panel
```

### 実装順

1. `MeasurementTime`、`ArrivalTime`、`StreamId`、`CameraId`を導入する。
2. 既存`LoadedPacket`相当を`RawMessage`へ置換する。
3. `PlaybackClock`を現在のPlayer stateから分離する。
4. `McapPlaybackSource::read_until(cursor)`を作る。
5. `StreamPipeline`、`PipelineFactory`、`PipelineSet`を作る。
6. `CompressedImagePipeline`でheader、format、JPEG payloadを抽出する。
7. `DomainUpdate::Camera::EncodedImage`を作る。
8. JPEGをRGBAへdecodeする最小実装を追加する。
9. `CameraState`へ最新frameを反映する。
10. `CameraTextureSlot`を作り、同解像度ではtextureを再利用する。
11. egui内の単一Camera panelへtextureを表示する。
12. cold seek時にcamera stateをclearする。

### 完了条件

- NativeでMCAPを開き、JPEGカメラ1台を再生できる。
- play/pause/speedが動く。
- measurement timeとMCAP arrival timeが保持される。
- RendererがMCAP、topic名、CDR、CompressedImage wire layoutを知らない。
- ウィンドウresizeでJPEG再decode／CPU resizeを行わない。
- 同解像度frameでGPU textureを再作成しない。
- malformed payloadでアプリがpanicしない。
- cold seek後、次のJPEGから表示が復帰する。

### テスト

- `read_until()`境界値
- unknown stream
- CompressedImage format正規化
- malformed JPEG/container
- seek generation
- texture reuse decision
- 古いdecode結果の非採用

---

## Phase 2: Web JPEG Camera Walking Skeleton + Minimal BEV

### 目的

Webで最優先のCamera表示パスを早期に成立させ、同時にwgpu offscreen BEV経路も確認する。

### 経路A: JPEG Camera

```text
Local MCAP File/Blob
  -> RawMessage
  -> CompressedImagePipeline
  -> EncodedCameraFrame
  -> Web-compatible JPEG Decoder
  -> DecodedCameraFrame
  -> CameraTextureSlot
  -> Camera panel
```

### 経路B: Minimal BEV

```text
Local MCAP File/Blob
  -> RawMessage / temporary fixture
  -> BevState
  -> BevFrameBuilder
  -> BevRenderer
  -> offscreen texture
  -> egui panel
```

最初のBEVは背景、grid、固定egoだけでよい。利用可能ならfuture pathを1本追加する。

### 完了条件

- browserでMCAPを選択できる。
- play/pauseできる。
- JPEGカメラ1台を表示できる。
- Nativeと同じPipeline／DomainUpdate／Camera stateを使う。
- 最小BEVを表示できる。
- panel resizeでCPU resizeしない。
- 同解像度でtextureを再作成しない。
- cold seek後、次のJPEGで復帰する。

### 実装しない

- H.264
- HTTP Range
- 11 camera Web
- Web ROS live
- browser固有zero-copy最適化

---

## Phase 3: BEV Full MVP

### 目的

カメラ表示を補完する、低遅延な自車中心2D状況表示を完成させる。

### 実装順

1. Ego
2. Future path
3. Detected objects
4. Occupancy grid

### 完了条件

- `BevFrame`が`MainSceneSnapshot`から独立している。
- 各layerはarrival timeで最新状態を更新する。
- 座標変換が必要な場合のみmeasurement timeを使う。
- revision不変layerでuploadしない。
- objectsはinstance rendering。
- pathはsegment instance。
- occupancyはR8 texture。
- representative logでリアルタイム再生可能。

---

## Phase 4: Native Camera Wall + Telemetry / HUD

### Camera

1. JPEG 1台の既存経路を11台へ拡張する。
2. 11台固定3x4 layoutを作る。
3. focused cameraをCamera Wall内で拡大する。
4. cameraごとのtimestamp、age、decode errorを表示する。
5. decode／upload metricsを追加する。

### Telemetry / HUD

- speed
- accel
- brake
- steering
- yaw rate
- gear
- autonomy state

### 完了条件

- Camera WallがBEVやUIを止めない。
- 各cameraは独立更新する。
- focused camera切替がvisibilityと分離している。
- Telemetry current valuesが固定位置で更新される。
- driver-control HUDとautonomy statusが表示される。
- 11台で性能不足がある場合、数値として記録され、Phase 7へ課題化される。

---

## Phase 5: Main 3D、Ego Pose同期、LiDAR

### 目的

事後の空間状況確認のため、measurement-time同期されたMain 3DとLiDARを追加する。

### 実装

- `EgoPosePipeline`
- `PoseHistory`
- `MeasurementPair<T>`
- `MainSceneSnapshotBuilder`
- pose interpolation
- `data_to_scene` transform
- ego pose future boundary
- 既存Livox decodeの`LivoxPointCloudPipeline`移設
- point cloud GPU bufferの永続化

### 完了条件

- Main 3D scene timeがego pose measurement timeで進む。
- future sensor dataを描画しない。
- sensor measurement timeでposeを内挿する。
- pose不足時にpanicしない。
- RendererがLivox wire layoutを知らない。
- 同容量の点群更新でGPU bufferを再作成しない。
- 複数pose batchで中間Snapshotを大量生成しない。

---

## Phase 6: ROS 2 Live

### 目的

MCAPとは別のSource経路から同じ`RawMessage`以降を再利用する。

### 実装

- ROS callbackでarrival time即時打刻
- stream catalog／binding
- Pipeline reuse
- bounded delivery
- connection diagnostics

### 完了条件

- ROS liveとMCAPが同じPipeline／Domain state／Rendererを使う。
- ROS liveへPlaybackClockを無理に通さない。
- Camera／Telemetry／BEVが独立更新する。
- Camera live pathを最初に成立させ、その後にMain 3D／LiDARを接続する。

---

## Phase 7: Measure and Optimize / H.264

以下を実測し、問題が確認されたものだけ対策する。

優先候補:

- 11-camera JPEG decode budget
- decode lookahead
- decoded staging
- worker pool
- per-camera latest mailbox
- JPEG coalescing
- camera preview stream
- H.264 Native/Web decoder
- browser／native hardware decode
- Web 11-camera feasibility

二次候補:

- point cloud buffer growth strategy
- packed point cloud／GPU decode
- HTTP Range MCAP

# 24. Test Strategy

## 24.1 Unit tests

- 時刻newtype
- `MeasurementPair::latest_before`
- `LatestArrived`採用順
- pose interpolation
- shortest-angle interpolation
- Pipeline build validation
- CompressedImage format normalization
- malformed payload rejection
- `BevFrameBuilder`座標変換
- cold seek clear
- generation mismatch discard

## 24.2 Golden tests

座標系のバグを防ぐため、以下をgolden test化する。

- ego原点
- +X前方、+Y左
- yaw正負
- oriented box四隅
- occupancy origin／resolution
- Main 3DとBEVで同じobjectが一致すること

## 24.3 Integration tests

小さいMCAP fixtureを用意する。

最低限:

- JPEG camera
- future path
- objects
- occupancy
- telemetry
- ego pose
- LiDAR

検証:

- play/pause
- speed
- cold seek
- waiting states
- Native/Web共通decode

## 24.4 GPU tests / benchmarks

- BEV revision不変sync
- occupancy upload
- object count scaling
- path segment scaling
- resize
- camera texture reuse

CIでGPU testが難しい場合、resource decision部分をunit testし、実GPU benchmarkは専用環境で実行する。

---

# 25. Coding Agent Instructions

コーディングエージェントは以下を守ること。

Camera経路を最優先する。LiDAR移行や点群最適化を、JPEG Camera Walking Skeletonより先に実装してはならない。

## 25.1 作業単位

- 一度に一つのPhaseまたは一つの縦切りだけ実装する。
- 既存動作を維持したまま置換する。
- 大規模な一括rewriteを避ける。
- 各変更後にbuild／test／実行確認を行う。

## 25.2 設計変更

以下を行う前に、Design Docへ理由を記録しレビューを求める。

- 新しい公開trait
- 新しい共有crate
- Source共通化の拡大
- ECS導入
- generic event bus
- plugin registry
- raw payloadの複雑な所有権抽象化
- GPU resource model変更
- 時刻意味の変更

## 25.3 依存追加

- 依存追加は最小限にする。
- Native専用依存を共有crateへ入れない。
- Wasm非対応依存を共有crateへ入れる場合は理由を明記する。
- JPEG decoderはNative/Webで別依存でもよい。

## 25.4 コード品質

- エラーを`unwrap()`で握りつぶさない。
- malformed inputでpanicしない。
- 公開型には役割を示すdoc commentを付ける。
- measurement timeとarrival timeを同じ型へ戻さない。
- Rendererへtopic名、schema名、ROS型名を持ち込まない。
- performance-sensitive allocationにはコメントとmetricを付ける。

## 25.5 完了報告

各作業の完了報告には以下を含める。

1. 実装したデータ経路
2. 変更したmodule／公開API
3. build／test結果
4. Native／Webの確認状況
5. 新しいallocation／copy
6. 未解決の問題
7. 次に進むべき最小作業

---

# 26. First Coding Task

最初のコーディングエージェントへの指示は以下とする。

## Task: Native JPEG Camera Walking Skeleton

### Goal

現在のMCAP再生コードへ、次のCamera縦切りを追加する。

```text
McapPlaybackSource
  -> RawMessage
  -> CompressedImagePipeline
  -> DomainUpdate::Camera::EncodedImage
  -> JPEG Decoder
  -> DecodedCameraFrame
  -> CameraState
  -> CameraTextureSlot
  -> egui Camera panel
```

### Required changes

1. `MeasurementTime`、`ArrivalTime`、`StreamId`、`CameraId`を導入する。
2. 既存`LoadedPacket`相当を`RawMessage`へ置換または適合させる。
3. `PlaybackClock`を現在のPlayer stateから分離する。
4. `McapPlaybackSource::read_until(cursor)`を作る。
5. `StreamPipeline` traitを作る。
6. `PipelineFactory`を明示的matchで作る。
7. `PipelineSet`でstream ID dispatchする。
8. 対象camera topic用の`CompressedImagePipeline`を実装する。
9. messageからheader.stamp、format、JPEG bytesを抽出する。
10. `DomainUpdate::Camera::EncodedImage`を作る。
11. JPEGをRGBAへdecodeする。
12. cameraごとに最新arrival timeのframeだけを採用する`CameraState`を作る。
13. `CameraTextureSlot`を追加し、同解像度ではtextureを再利用する。
14. egui panelへ1カメラを表示する。
15. cold seek時にdynamic camera stateをclearする。

### Constraints

- LiDARの移行を先に行わない。
- ROS liveを実装しない。
- Web UIをまだ実装しない。
- H.264を実装しない。
- Pipeline Registryを作らない。
- Source共通traitを作らない。
- decode worker poolを作らない。
- JPEGのzero-copy最適化をしない。
- 画像をpanelサイズへCPU resizeしない。

### Acceptance criteria

- ローカルMCAPを再生し、指定JPEG cameraを表示できる。
- play/pause/speedが維持される。
- header.stampがmeasurement timeとして保持される。
- MCAP記録時刻がarrival timeとして保持される。
- RendererがMCAP、topic名、CompressedImage wire layoutを知らない。
- unknown streamを安全に無視または報告できる。
- malformed payloadでアプリがpanicしない。
- 同解像度のframe更新でGPU textureを再作成しない。
- resizeでJPEG再decode／CPU resizeを行わない。
- cold seek後、次のJPEGから表示が復帰する。
- `cargo test --workspace`が通る。
- 共有crateが`wasm32-unknown-unknown`でcheckできる。

---

# 27. Open Questions

以下はMVP開始を止めない。該当Phaseで判断する。

| 項目 | 判断時期 | 論点 |
| --- | --- | --- |
| MCAP received time field | Phase 1 | 記録系がreceived timeをどのMCAP fieldへ格納しているか確認する。 |
| Pose history duration | Phase 5 | 初期1秒で十分か、実ログのarrival/decode遅延を見る。 |
| TF model | Phase 5以降 | fixed extrinsicから始め、dynamic TF storeをいつ導入するか。 |
| Web MCAP reader | Phase 2 | 小さいファイル全体readで始めるか、最初からBlob sliceを使うか。 |
| Web JPEG decoder | Phase 2 | 使用API／crateとCPU copy経路。 |
| Main 3D Web対応 | Phase 5以降 | Web Walking Skeleton後の優先順位。 |
| 11 camera decode budget | Phase 4/7 | 解像度、fps、preview stream、hardware decode。 |
| H.264 | Phase 7 | Native/Web decoder、access unit、keyframe、seek。 |
| Point cloud format | Phase 5/7 | canonical CPU vertexかpacked GPU decodeか。 |
| Decode staging depth | Phase 7 | 0から開始し、decode latency実測で決める。 |
| Worker count | Phase 7 | workloadとCPU profileで決める。 |
| HTTP Range | Phase 7 | Remote MCAPの実要件確認後。 |

---

# 28. Architecture Decision Summary

1. 初期版は単一プロセス・単一ウィンドウとする。
2. グラフ／IPCは延期する。
3. 表示領域ごとに同期ドメインを分ける。
4. Measurement timeとArrival timeを分ける。
5. Main 3Dはego pose measurement timeで駆動する。
6. Main Sceneデータは限定buffer、BEV／Camera／Telemetryは最新状態を持つ。
7. BEVは独立wgpu offscreen renderer crateとする。
8. `MainSceneSnapshot`と`BevFrame`を分ける。
9. StreamPipelineはSource open時にPipelineFactoryで構築する。
10. 初期版でPipeline Registryを作らない。
11. 表示切り替えはdraw ON/OFFに限定する。
12. 画像／BEV resizeはGPU中心に行う。
13. GPU resourceを永続化し、revision差分で更新する。
14. ECS、汎用plugin、自由dock UIを導入しない。
15. Playbackはfetch horizonとdisplay cursorに分ける。
16. MVPではlookahead 0、cold seekとする。
17. Native first, Web earlyとする。
18. Native/Web MVPでJPEGカメラ1台を最優先で通し、Webではminimal BEVも通す。
19. LiDARはCamera／BEV／Telemetry経路の後に、事後確認用Main 3Dとして追加する。
20. H.264はJPEG表示パス成立後に追加する。

---

# 29. Review Checklist

実装開始前の最終確認:

- [ ] 現在のprototypeで使用するMCAPのreceived time fieldを把握している。
- [ ] 最初に対応するJPEG camera topic／schemaが分かっている。
- [ ] そのMCAPでCompressedImageのformat、header.stamp、受信時刻の格納先を確認できる。
- [ ] 後続で対応するLiDAR topic／schemaが分かっている、またはPhase 5までに確認する担当が決まっている。
- [ ] Native/Web共有crateのWASM checkをCIまたはローカルで実行できる。
- [ ] cold seek後に表示欠落を許容することが関係者に共有されている。
- [ ] H.264、11 camera Web、IPC、graphがMVP外であることが共有されている。
- [ ] Rendererからdecodeを除去する移行順が理解されている。
- [ ] GPU resource再利用を検証するmetricまたはdebug counterを用意する。

Phase完了レビュー:

- [ ] 縦切りが実行可能か。
- [ ] 境界を越えた依存が増えていないか。
- [ ] Native専用コードが共有crateへ混入していないか。
- [ ] time semanticsが暗黙に変わっていないか。
- [ ] 新しいallocation/copyを把握しているか。
- [ ] 次の最小Phaseに進める状態か。

---

# 30. 最終方針

このプロジェクトは、完成形のアーキテクチャを一度に作るのではなく、カメラ価値を中心に以下の順で育てる。

```text
NativeでJPEG camera 1本を通す
    -> WebでもJPEG camera 1本 + minimal BEVを通す
        -> BEVを完成させる
            -> Native Camera Wall 11台 + Telemetry/HUDへ広げる
                -> Main 3D / ego同期 / LiDARを追加する
                    -> ROS liveを追加する
                        -> 実測してH.264・staging・11台性能を最適化する
```

最初の実装で守るべき最重要点は、機能数ではない。

```text
Source -> RawMessage -> Pipeline -> DomainUpdate
      -> State -> Presentation model -> Renderer
```

この経路を一本通し、以後の機能を同じ境界へ追加できる状態を作ることがMVPの中心である。
