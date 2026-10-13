# READMEのスクリーンショット

READMEの[monitor画面](images/monitor-workspace.png)はNative Viewerの実ウィンドウを撮影しています。
同梱のGazebo由来の小さなシミュレーション記録を使用し、Camera・LaserScan・Odometry・TFと
決定的な計画Pathを同時表示しています。計画Pathはサンプル用の生成データです。

## サンプルを準備する

```sh
mkdir -p /tmp/mcap-readme-demo
cargo run -p ros-fixture --example readme_fixture -- /tmp/mcap-readme-demo/recording.mcap
cargo run -p preview-builder -- /tmp/mcap-readme-demo/recording.mcap
cargo run -p viewer-native -- \
  --mcap /tmp/mcap-readme-demo/recording.mcap \
  --layout monitor --inspect-topic /scan
```

`readme_fixture`は既存の`tests/fixtures/camera-jpeg/camera_7_5s.mcap`を読み、Camera／LaserScanの
root-only ros2msg schemaにHeader／Timeの依存定義を補った撮影用コピーを作ります。
message payload、log time、publish time、sequenceは保持し、MCAP indexはwriterで再構築します。
出力が既に存在する場合は上書きしません。再実行時は別の空ディレクトリを使用してください。
Previewはこのコピーのsource fingerprintに対応して別途生成します。
この補完は同梱fixtureの既知の2型のみが対象で、任意ROS schemaの自動解決ではありません。

## 撮影した状態

- Native `monitor` preset、1600 × 1100 pixels。
- 記録の末尾で停止し、`/scan`のlatest messageを表示。
- SceneはChase、scan accumulationはoff。
- Cameraはフロント1台。PathとCamera calibration／TFによるoverlayを表示。
- Desktopや別アプリを含めず、Viewerのclient areaのみをPNGで保存。

READMEの画像はfieldを折りたたんだ状態です。Inspectorではheaderを展開し、下へscrollして
`ranges`などの配列を64要素ずつ閲覧できます。

## ブラウザ版を撮影する

READMEの[ブラウザ画面](images/monitor-workspace-web.png)は、同じ
`/tmp/mcap-readme-demo/recording.mcap`をChromeのWeb版で開いた実画面です。

```sh
cd apps/viewer-web
trunk serve --release
```

1. Chromeで表示されたURLを開き、viewportを1600 × 1100 pixelsにする。
2. `Open MCAP`から撮影用の`recording.mcap`を選択する。Web版ではPreview sidecarは不要。
3. timelineを末尾までseekし、停止状態にする。
4. Topicsから`/scan`を選び、Inspectorの自動取得が完了するまで待つ。
5. `3D Scene`、`Chase`、`Accumulate scans` off、フロントCameraを選ぶ。
6. Inspectorのfieldは折りたたみ、Remote Recording Serverも閉じた状態でページ領域をPNG保存する。

撮影時はNativeと同じcursor `1768724930204523401 ns`、Scan sequence 43、360点、
Camera overlayの可視点30を確認した。パネル配置と再生状態を揃え、Web固有の操作部と
camera thumbnailsはそのまま表示している。

撮影にはheadless ChromeとソフトウェアWebGPUを使用した。画像はページviewportの実キャプチャで、
ブラウザのtoolbarやDesktopを含まない。撮影用にパネルを隠したり合成したりしていない。
