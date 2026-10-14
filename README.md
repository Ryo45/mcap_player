# MCAP Player

ROS 2 の MCAP を再生し、カメラ映像・車両状態・周辺情報をまとめて確認する Rust 製ビューアです。Native、ブラウザ、Recording Server 経由のリモート再生、ROS 2 live 入力に対応します。

ロボットの「何が見えていて、どこへ進もうとしていて、そのときtopicに何が入っていたか」を
一つの画面で追うための監視・解析ツールです。記録を再生・停止・seekしながら、映像と空間表示を
確認し、気になるtopicの値を同じ再生位置で調べられます。

**Native**

![Nativeのmonitor画面。左にTopic一覧とLaserScanのInspector、右上に点群と計画軌跡の3D Scene、右下に軌跡を重畳したフロントCamera。](docs/images/monitor-workspace.png)

*同梱のシミュレーション記録をNativeで再生し、`/scan`をInspectした実画面です。
水色はLaserScan、黄色はScene上の計画軌跡、緑色はCameraへの軌跡投影です。
画面上部には速度・再生状態・TF／Cameraの状態を表示しています。*

**ブラウザ（Chrome／WebGPU）**

![ChromeのWeb版。Nativeと同じ録画・再生位置で、左にLaserScanのInspector、右上に3D Scene、右下に軌跡overlay付きフロントCameraを表示。](docs/images/monitor-workspace-web.png)

*同じサンプルをブラウザで開いた実画面です。Nativeと同じく、左にTopics／Inspector、右上にScene、
右下にCameraを配置し、記録の末尾で停止して`/scan`を表示しています。
ローカルMCAPを選択するだけで、ブラウザでも点群・計画軌跡・Camera overlay・メッセージの値を確認できます。*

topicを調べても、CameraやSceneの再生対象は変わりません。大きな記録は必要なtopicと範囲を読み、
配列はページ単位で、Plotは点数を制限した概要として表示します。
まず試すには[監視・解析用の画面](#監視解析用の画面)、撮影に使ったサンプルの準備は
[スクリーンショットの再現手順](docs/screenshots.md)を参照してください。

## 主な機能

- 複数の JPEG カメラ（`sensor_msgs/msg/CompressedImage`）を一覧・フォーカス表示
- カメラ画像上に計画経路を投影
- BEV にグリッド、車両位置、計画経路を表示
- 3D ビューに車両姿勢、軌跡、LaserScan を表示
- `/odom` の位置、向き、速度、ヨーレートを再生位置に同期して表示
- `/tf` と `/tf_static` による座標変換

## 性能検証

必要なデータだけを読み込み・decode・保持する設計が有効になる条件と限界を、
圧縮方式・Chunk構成・選択トピック・記録長・Seekの条件を変えて測っています。
[計測手順と指標の定義](docs/performance/README.md)に再現用コマンドと生データをまとめています。
Native連続読み込みとindexed queryではI/O削減の効き方が異なるため、分けて評価します。

## Native で再生

引数なしの場合は同梱サンプルを再生します。

```bash
cargo run -p viewer-native
```

ファイルとカメラトピックを指定する場合:

```bash
cargo run -p viewer-native -- \
  --mcap tests/fixtures/camera-jpeg/camera_7_5s.mcap \
  --camera-topic /camera/front/image/compressed
```

起動中のウィンドウへ `.mcap` ファイルをドロップして開くこともできます。カメラキャリブレーションを変更する場合は `--camera-calibration FILE` を指定してください。

`standard` / `showcase` / `explore` / `monitor` のLayout JSONは0.1では同梱された内部presetです。外部向けの保存形式や
backward compatibilityは保証しません。`schemaVersion` / `configVersion` は同梱presetの開発時検証と
unsupported panelの可視化に使い、外部Layoutのload/save/migration policyは公開時に別途定義します。

## ブラウザで再生

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk
cd apps/viewer-web
trunk serve --release
```

表示されたページでローカルの MCAP ファイルを選択します。ファイル全体は読み込まず、再生に必要な範囲だけを取得します。
左に Topic 一覧・メッセージ Inspector、右上に 3D Scene、右下にカメラを表示します。
Topic を選ぶと、現在の MCAP log time 以前の最新メッセージを自動取得します（最大250 ms間隔）。
「Follow cursor」で自動追従を切り替え、「Refresh message」で手動更新できます。
ROS 2 CDR のフィールドは完全な ros2msg dependency 定義があれば展開できます。カメラのない録画も開けます。
3D は Chase / Free / Vehicle eye、点群蓄積に対応し、BEV 表示にも切り替えられます。
Native / Web の対応状況と今後の開発順は[差分一覧](docs/native_web_parity.md)を参照してください。

## 物理geometryとTurtleBot3

Scene内の距離は **1.0 = 1 metre** です。車体・Path・Scanに個別の倍率を掛けません。
同梱データ向けの車体はTurtleBot3 Waffle PiのURDF collision形状の包絡で、
約 **0.283 × 0.306 × 0.141 m**。1 m gridと同じ単位で表示します。
Pathはframe付き3D座標とzを保持し、TFでScene／BEV／Cameraへ変換します。

```sh
cargo run -p viewer-native -- --layout monitor \
  --platform-geometry config/platform_geometry/turtlebot3_waffle_pi.json \
  --ground-frame base_footprint
```

geometry JSONは別のロボットや自動車の非対称なboundsにも置き換えられます。
Nativeは`--no-platform-geometry`、WebはSceneの設定から車体を非表示にできます。
Ground frameを指定しなければPathの元のzを使います。Camera calibrationは別設定で、
同梱値がデモ値であることをoverlay状態に表示します。
寸法の出所・座標系・検証・現在の制約は[physical geometry](docs/physical_geometry.md)を参照してください。

## Recording Server

LAN 上の MCAP をブラウザから再生する場合に使用します。

```bash
cp config/recording-server.toml.example config/recording-server.toml
# config/recording-server.toml に MCAP のパスと許可する Origin を設定
cargo run -p recording-server -- \
  --config config/recording-server.toml
```

ブラウザでサーバー URL（既定値: `http://localhost:8081`）を入力し、Remote Playback を開きます。設定の詳細は [Filesystem Recording Server](docs/filesystem_recording_server.md) を参照してください。

## ROS 2 live 入力

ROS 2 Jazzy の環境を読み込み、`ros2-live` 機能を有効にします。

```bash
source /opt/ros/jazzy/setup.bash
cargo run -p viewer-native --features ros2-live -- \
  --live --camera-topic /camera/front/image/compressed
```

既定の QoS は best effort / volatile です。Reliable にする場合は `--reliable` を追加してください。動作確認用データの生成手順は [ROS fixture](tools/ros-fixture/README.md) を参照してください。

## 対応データと制約

- カメラ: JPEG の `sensor_msgs/msg/CompressedImage`
- 計画経路: `/planning/path` の `nav_msgs/msg/Path`
- 車両状態: `/odom`
- 点群表示: `LaserScan`
- カメラ内部パラメータ: `config/camera_calibration.json`
- 未対応: Raw `sensor_msgs/msg/Image`、H.264、ブラウザでの live 入力

## 監視・解析用の画面

Nativeに`monitor` presetを追加しています。

`--inspect-topic /scan`を追加すると、起動時からそのtopicをInspectorに表示します。
Inspector対象は画面のTopic一覧で変更できます。このオプションは記録入力専用です。

```sh
cargo run -p viewer-native -- --mcap PATH --layout monitor
# ROS 2 Jazzy環境でリアルタイム監視
cargo run -p viewer-native --features ros2-live -- --live --layout monitor
```

- 左30%: 検索できるTopic一覧、下に選択topicのGeneric Inspector。クリックで対象を切り替えます。
- 右70%の上半分: 点群・車両・計画軌跡を重畳した3D Scene。Chase / Free / Vehicle eyeを切り替えられます。
- 右下半分: フロントCamera 1台＋計画軌跡overlay。
- Scene上部: 再生状態／時刻、速度、yaw rate、Camera／overlay状態、TF診断を常時表示。

左上の「Source / diagnostics」で詳細診断、Inspector下の「Schema / numeric plots」でschemaと
field Plotにアクセスできます。通常画面の追加sidebarとInspectorウィンドウは表示しません。
現在値はcontinuous featureの共有状態から表示し、速度overviewの全記録読み込みは行いません。
点群・TF・Odometry・Pathは存在する場合に使用します。フロントCameraは
`/camera/front/image/compressed`、他のtopic bindingは`config/workspace_bindings.json`を使用します。
点群入力は現在`LaserScan`です。軌跡overlayにはPath、TF、Camera calibrationが必要で、欠落時は状態を表示します。
Generic Inspectorは記録のcursor query専用で、Liveの任意topic値閲覧は今後の対応です。
このpresetはNativeのみです。

## Generic Inspector

Nativeの「Topics」でtopicを開き、**Inspect at cursor**を選ぶと、再生cursor `T`でのlatest message
`<= T`を表示します。log time／publish time／sequence／ROS header stampと、schemaに従ったfield treeを
閲覧できます。nested messageは折りたたみ、配列／sequenceは64要素ずつページ表示します。
対象topicの選択でplayback selectionは増えません。messageとindex/chunkを再利用するため、近接した
cursor移動でwhole-log scanや同じchunkの再展開を繰り返しません。
ROS 2 plain CDR＋完全なros2msg dependency定義に対応し、未対応schemaはmetadataと理由を表示します。
payload上限16 MiB、最大8 topicです。詳細は[architecture](docs/message_centric_architecture.md)を参照してください。

## Topic Tree / numeric field Plot

Nativeの左側「Topics」はcatalog/schema metadataだけで構築します。topicを開き、numeric fieldの
「+」から任意fieldのbounded overview Plotを追加できます（最大8本、各4,000点）。Cameraなどを
含まないrecordingは`cargo run -p viewer-native -- --mcap PATH --layout explore`で開けます。同じstreamの
loading中fieldはまとめて読み、source切替後もnumeric workerは同時に1つです。追加・削除で
continuous playbackのstream selectionは変わりません。ROS 2 CDR + ros2msgのnested numeric scalarに
対応し、元schema definitionも表示します。Web Local/Remoteにも同じschema bytesを保持しますが、
field Plot UIは現在Nativeのみです。現在の設計と対応範囲は
[message-centric architecture](docs/message_centric_architecture.md)を参照してください。

## Native recording I/O

NativeのMCAP読み込みはbounded positional readを使います。元ファイルの変更・truncateでも、
読み込み済みpayloadのRust参照は無効化されません。検知した変更はエラーとして扱います。
詳細とAPIの時間境界は[現在のarchitecture](docs/message_centric_architecture.md)を参照してください。

Nativeカメラは表示の物理pixel数に合わせてlibjpeg-turboのIDCT縮小を使い、2本のbounded workerでJPEGをdecodeします。
overlayは元画像の座標で計算します。[設計と制限](docs/native_jpeg.md)、[FHD表示を基準にした4K記録の比較計測](docs/performance/2026-10-14-native-jpeg/summary.md)を参照してください。

## 開発環境

[Docker開発コンテナ](docs/development_container.md)にRust 1.91.1・clang・WASM・trunk・ROS 2 Jazzyをまとめています。
NativeのJPEGデコードにはSIMDを有効にしたlibjpeg-turboを静的リンクします。ホストでビルドする場合はC/C++ツールチェーン、CMake、NASMが必要です（Ubuntu: `sudo apt install build-essential cmake nasm pkg-config`）。Dockerには同梱しています。

`docker compose build dev`で準備し、`docker compose run --rm dev mcap-dev-check`で検証できます。

## 開発時のチェック

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check -p viewer-core -p viewer-mcap -p viewer-renderer -p bev-renderer -p scene-renderer --target wasm32-unknown-unknown
cd apps/viewer-web && trunk build --release
source /opt/ros/jazzy/setup.bash
cargo test -p viewer-native --features ros2-live
```

Native の `--layout health` は Topic Health（MCAP log time の到着・周期の事実）と
Diagnostics（producer が報告した DiagnosticArray の level）を別 Panel で表示します。
デフォルトは直近10秒、Diagnostics topic は `/diagnostics` です。
詳細と設定は [Native recording health panels](docs/native_recording_health.md) を参照してください。

Rustの責務別配置と命名方針は[module structure](docs/module_structure.md)を参照してください。

Webのカメラ表示は2本のWeb WorkerでJPEGをデコードし、表示サイズに合わせたImageBitmapをCanvasへ渡します。
[設計・制限と検証](docs/web_jpeg.md)、[4K JPEGのFHD基準比較](docs/performance/2026-10-15-web-jpeg/README.md)を参照してください。
