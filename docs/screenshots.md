# READMEのスクリーンショット

READMEと[レイアウトギャラリー](layout_gallery.md)の5枚は、現行のNative / Web releaseビルドの実画面です。
2026-10-27に`18787dc`の実装で撮り直し、Cameraのトピック選択プルダウンも表示しています。
同梱のGazebo由来データに、決定的な計画Pathと完全なCamera / LaserScan schemaを加えた撮影用コピーを使います。

## サンプルを準備する

リポジトリのルートで実行します。下の出力先が既にある場合は別の空ディレクトリを使ってください。
`readme_fixture`は既存ファイルを上書きしません。

```sh
mkdir -p /tmp/mcap-readme-demo
cargo run --release -p ros-fixture --example readme_fixture -- /tmp/mcap-readme-demo/recording.mcap
cargo run --release -p preview-builder -- /tmp/mcap-readme-demo/recording.mcap
cargo run --release -p viewer-native -- \
  --mcap /tmp/mcap-readme-demo/recording.mcap \
  --layout monitor --inspect-topic /scan
```

`readme_fixture`は`tests/fixtures/camera-jpeg/camera_7_5s.mcap`を読み、Camera / LaserScanの
root-only ros2msg schemaにHeader / Timeの依存定義を補います。
message payload、log time、publish time、sequenceは保持し、MCAP indexはwriterで再構築します。
この補完は同梱fixtureの既知の2型のみが対象で、任意ROS schemaの自動解決ではありません。

Previewはこのコピーのsource fingerprintに対応して別途生成します。
現在のschema 2で生成するため、古いsidecarを再利用しません。Webの撮影にsidecarは不要です。

## 共通の状態

- 解像度はNative client area / browser viewportとも **1920 × 1080 pixels**。
- MCAP log timeの末尾`1768724930204523401 ns`（開始から約5.018秒）で停止。
- Native / Webのmonitor画面では`/scan`をInspectorで表示。sequence 43、360点。
- SceneはChase、距離6 m、点群蓄積off。表示先frameは`odom`。
- 車体はTurtleBot3 Waffle Piの物理bounds、gridは1 m。Pathの元のxyzを使用。
- Cameraはフロント画像を選択し、overlayは30 visible points。校正値はデモ値と画面内にも表示。
- アプリのパネル・操作部をそのまま表示。合成・パネル非表示・表示用の物理scale補正は行わない。

Pathは生成された15 mのサンプルです。複数Camera画像は同一元映像の色違いで、異なる実視点ではありません。
実ロボットの走行検証や実測calibrationの結果として扱わないでください。

## Native

[monitor画面](images/monitor-workspace.png)は上のコマンドで再現できます。
記録を最後まで再生し、Inspectorの取得完了を待ちます。fieldの折りたたみ状態は既定のままです。

ギャラリーの`cameras` / `navigation` / `inspection`も同じ記録を使用します。
`--layout`を変更し、`inspection`では`--inspect-topic /scan`を指定します。
速度・ヨーレートのoverview取得が完了してから撮影します。

Nativeの撮影はLinux / NVIDIA RTX 5070 Ti / Vulkanの通常ウィンドウです。
Desktopや別アプリを含めず、client areaをPNGで保存しています。

## ブラウザ

[READMEの起動手順](../README.md#ブラウザで試す)でWeb版を開きます。
ホストにTrunk等が揃っている場合は次のコマンドでも起動できます。

```sh
(cd apps/viewer-web && trunk serve --release)
```

1. viewportを1920 × 1080 pixelsにする。
2. **Open MCAP**で上の`recording.mcap`を選ぶ。
3. timelineを末尾へSeekし、停止状態にする。
4. Topicsから`/scan`を選び、Inspectorの取得が完了するまで待つ。
5. `3D Scene` / `Chase`、`Accumulate points` off、Cameraの`Image topic`をフロント画像にする。
6. Remote Recording Serverを閉じた状態で、ページ領域をPNG保存する。

撮影にはLinux / Chrome 143 / NVIDIA Vulkanのhardware GPUを使用しました。
ブラウザのtoolbarやDesktopを含みません。Web固有の操作部とCamera thumbnailsはそのまま表示しています。

[撮影記録](images/readme-capture.json)に、入力とNative binaryのSHA-256、viewport、browser/GPU情報を保存しています。
