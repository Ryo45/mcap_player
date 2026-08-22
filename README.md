# MCAP Player

ROS 2 の MCAP を再生し、カメラ映像・車両状態・周辺情報をまとめて確認する Rust 製ビューアです。Native、ブラウザ、Recording Server 経由のリモート再生、ROS 2 live 入力に対応します。

## 主な機能

- 複数の JPEG カメラ（`sensor_msgs/msg/CompressedImage`）を一覧・フォーカス表示
- カメラ画像上に計画経路を投影
- BEV にグリッド、車両位置、計画経路を表示
- 3D ビューに車両姿勢、軌跡、LaserScan を表示
- `/odom` の位置、向き、速度、ヨーレートを再生位置に同期して表示
- `/tf` と `/tf_static` による座標変換

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

## ブラウザで再生

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk
cd apps/viewer-web
trunk serve --release
```

表示されたページでローカルの MCAP ファイルを選択します。ファイル全体は読み込まず、再生に必要な範囲だけを取得します。

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

## 開発時のチェック

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check -p viewer-core -p viewer-renderer -p bev-renderer -p scene-renderer --target wasm32-unknown-unknown
cd apps/viewer-web && trunk build --release
source /opt/ros/jazzy/setup.bash
cargo test -p viewer-native --features ros2-live
```
