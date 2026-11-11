# ROS 2 Live入力

通常のMCAP再生にはROS 2は不要です。このページはROS 2へ直接接続するNative利用者向けです。

ROS 2 Jazzyの環境で`ros2-live`を有効にします。画像形式は起動時に指定します。

```sh
source /opt/ros/jazzy/setup.bash
cargo run --release -p viewer-native --features ros2-live -- \
  --live --layout standard --camera-topic /camera/front/image/compressed

# Raw Imageの場合
cargo run --release -p viewer-native --features ros2-live -- \
  --live --layout standard --live-raw-camera-topic /camera/image_raw
```

Raw topicは複数指定でき、CompressedImageと混在できます。既定QoSはbest effort / volatileで、`--reliable`も指定できます。
プルダウンの切り替えは購読済み画像の範囲です。ROSグラフからの追加発見・動的な購読変更、LiveのGeneric Inspectorは未対応です。
[Live入力の検証](verification/2026-10-21-camera-formats/raw-live.md)と[fixtureツール](../tools/ros-fixture/README.md)を参照してください。

Path・TF等のトピック名は起動時の`--workspace-bindings FILE.json`でも指定できます。
設定JSONと入力の優先順位は[記録・入力設定](recording_usage.md)を参照してください。

```sh
cargo run --release --features ros2-live -- \
  --live --workspace-bindings my_robot.json
```
