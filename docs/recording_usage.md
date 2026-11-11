# 手持ちの記録を開く・入力設定

[README](../README.md)のビルド・起動手順を済ませてから、以下のコマンドをリポジトリのルートで実行してください。

## カメラ・Scene・Topicの値を一緒に見る

```sh
cargo run --release -p viewer-native -- \
  --mcap /path/to/recording.mcap --layout monitor --no-platform-geometry
```

ファイルや分割MCAPのディレクトリを、Nativeのウィンドウへドロップして開くこともできます。
分割入力はディレクトリ直下の`.mcap`を1本のlog-time軸として扱います。

**画像トピック名をコードに合わせる必要はありません。**
`Image` / `CompressedImage`のschemaとCDR encodingから候補を抽出し、Cameraのプルダウンで切り替えられます。
Nativeの複数パネルはそれぞれ独立して選択できます。
`--camera-topic TOPIC`は初期カメラの希望値で、固定パネルの設定を一括置換する引数ではありません。
[選択・フォーカス・プレビューの動作](camera_topic_selection.md)を参照してください。

同梱の車体形状はTurtleBot3 Waffle Piです。ほかの機体では上記のように非表示にするか、
`--platform-geometry FILE.json`で実際の形状を指定します。Sceneは **1.0 = 1 metre** を維持します。

## 点群のトピックや座標系を合わせる

```sh
cargo run --release -p viewer-native -- \
  --mcap /path/to/recording.mcap --layout scene \
  --point-cloud-topic /points --fixed-frame lidar --no-platform-geometry
```

`/points`と`lidar`は、記録内の実際のトピック名・frame名に置き換えてください。
表示先frameが入力frameと異なる場合は、それらを結ぶTFが必要です。
SceneではAuto / Solid / RGB・RGBA / Intensityを選び、強度範囲も変更できます。
Webではファイルを開く前にSceneの設定でPoint cloud topicとWorld frameを指定します。
[PointCloud2の対応形式と上限](point_cloud2.md)を参照してください。

## Topicの中身とグラフを調べる

CameraやSceneが不要な記録には、`--layout explore`が使えます。
Topicsから対象を選ぶと、現在のMCAP log time以前の最新メッセージをInspectorで確認できます。
配列はページ単位で表示し、Nativeではnumeric fieldの「+」から概要Plotを追加できます。
InspectorやPlotで対象を選んでも、Cameraなどのcontinuous playback対象は増えません。

ほかに`health`（到着周期・Diagnostics）、`inspection`（Inspector・速度/ヨーレート）があります。
引数なしの既定レイアウトは`standard`です。`standard` / `navigation`などはデモ用のPath・Odometry等を要求するため、
手持ちの記録の入口には、利用可能なScene入力を使う`monitor`か、query中心の`explore`を勧めます。

## Path・TF・Camera overlayを合わせる

| 入力 | 既定値・変更方法 |
| --- | --- |
| Camera | schemaから検出し、パネルで選択 |
| LaserScan / PointCloud2 | `/scan`。Nativeは`--point-cloud-topic`、WebはScene設定で変更 |
| 表示先frame | `odom`。Nativeは`--fixed-frame`、WebはWorld frameで変更 |
| Path | `/planning/path` |
| Odometry | `/odom` |
| 動的TF / 静的TF | `/tf` / `/tf_static` |

Path・Odometry・PointCloud・TFのトピック名は、[workspace bindings](../config/workspace_bindings.json)と同じ形式のJSONを実行時に指定できます。再ビルドは不要です。

```sh
cargo run -p viewer-native --release -- \
  --mcap recording.mcap --layout monitor --workspace-bindings my_robot.json
# ROS 2 Liveでも起動時に指定できます（購読の実行中切り替えは行いません）。
cargo run -p viewer-native --release --features ros2-live -- \
  --live --workspace-bindings my_robot.json
```

```json
{
  "pathTopic": "/planning/path",
  "odometryTopic": "/odom",
  "pointCloudTopic": "/scan",
  "dynamicTfTopic": "/tf",
  "staticTfTopic": "/tf_static"
}
```

Nativeの優先順位は、明示的な`--point-cloud-topic` > 外部JSON > 同梱デフォルトです。
外部JSONは5項目すべて指定してください。未知キー・欠落項目・空文字などの不備は起動エラーになります。

Webでは **Open MCAPの隣のWorkspace bindings JSON** で先にファイルを選択し、valid表示を確認してから記録を開きます。
Browser LocalとRecording Serverの両方で利用でき、JSONはブラウザ内で処理されます。サーバーへのアップロードや追加APIは不要です。
Scene設定のPoint cloud topic欄は、**空欄ならJSONの値**を使い、入力した場合だけJSONを上書きします。
設定未指定時は従来のデフォルトを使います。エラー中・読み込み中は新しい記録を開けません。
訂正したJSONを選ぶか、Use bundled bindingsで明示的に同梱設定へ戻してください。PointCloud欄の手入力は引き続き優先され、空欄にするとJSON／同梱値へ戻ります。
設定は次に開く記録に適用され、開いているセッションには反映しません。Camera切り替えやSeekでも、そのセッションの設定を維持します。
JSONはUTF-8・16 KiB以下、トピック名は1〜1024バイトで空白・制御文字を含めないでください。
実際のtopic/schemaの解決は既存の必須・任意入力ルールに従い、自動推定は行いません。[Native／Web Local／Remoteの検証結果](verification/workspace-bindings/README.md)を参照してください。
レイアウトの配置も同梱presetで、外部JSONの実行時load/saveやGUIでの配置編集は未対応です。

Camera overlayにはPath・TF・実カメラに対応したcalibrationが必要です。
同梱のcalibrationはデモ値で、別カメラの実測値としては使えません。
Nativeは`--camera-calibration FILE.json`、WebはScene設定のCamera calibration JSONから読み込みます。
[物理geometry・座標系・ground frame](physical_geometry.md)に設定例をまとめています。

## 対応範囲と制約

| 項目 | Native | Browser Local | Recording Server → Web |
| --- | --- | --- | --- |
| 索引付きMCAP | 単一ファイル・分割ディレクトリ | 単一ファイル | 設定した単一ファイル |
| MCAP圧縮 | 非圧縮・Zstd・LZ4 | 非圧縮・Zstd・LZ4 | 非圧縮・Zstd |
| Camera / Scene / BEV / Inspector | 対応 | 対応 | 対応 |
| 数値Plot・Topic Health・Diagnostics | 対応 | 未対応 | 未対応 |
| Preview / bookmarks | 対応 | 未対応 | 未対応 |
| ROS 2 Live | 対応 | 未対応 | 未対応 |

- **索引が必要です。** Summary・Chunk Index・選択streamのMessage Indexがない場合はエラーを表示します。
  巨大ファイルを黙って全走査することはありません。rosbag2 fastwriteなどは[明示的な変換](fastwrite.md)を行ってください。
- CameraはJPEG / PNGの`CompressedImage`と、`rgb8 / bgr8 / rgba8 / bgra8 / mono8`のRaw `Image`に対応します。
  16bit・深度画像・compressedDepth・H.264/H.265は未対応です。
- Inspectorとnumeric field PlotはROS 2 plain CDRと完全なros2msg依存定義を使います。
  schemaが不足・未対応の場合はメタデータと理由を表示します。
- PointCloud2は記録入力に対応します。Liveの点群入力はLaserScanです。
- Nativeの画像Previewは任意の追加sidecarです。なくても再生・exact Seekは使えます。
  **旧schema 1のsidecarは現在のpreview-builderで再生成が必要です。** 分割記録全体のPreview生成は未対応です。
- 大きなChunkや高解像度画像は、展開時間・一時メモリ・Seek遅延に影響します。入力経路ごとに読み込み・保持・query上限があります。
  すべての記録で一定のFPSやメモリ量を保証するものではありません。

対応差分の詳細は[Native / Web一覧](native_web_parity.md)、分割MCAPの制約は[入力互換性](issue/015-split-mcap-and-web-lz4/README.md)を参照してください。

