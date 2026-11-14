# ビルドと起動

最短の利用手順は[README](../README.md#nativeで試す)を参照してください。
通常のMCAP再生にDocker・ROS 2は不要です。コマンドはリポジトリのルートで実行します。

## Nativeのビルド依存

Rust **1.91.1**でビルド・検証しています。通常の記録再生にROS 2のインストールは不要です。
NativeのビルドにはC/C++ツールチェーン、CMake、NASM、pkg-configが必要です。

Ubuntu 24.04のビルド依存パッケージ例です。

```sh
sudo apt-get update
sudo apt-get install -y build-essential cmake nasm pkg-config \
  libudev-dev libxkbcommon-dev libwayland-dev libx11-dev libxi-dev \
  libxrandr-dev libxcursor-dev libvulkan-dev
```

## Nativeのビルド・起動

```sh
cargo build --release
./target/release/viewer-native --layout monitor
# ビルドと起動を一度に行う場合
cargo run --release -- --layout monitor
```

`cargo build`の出力は`target/debug/viewer-native`、releaseは`target/release/viewer-native`です。
初回ビルドには時間がかかります。性能を評価する際はreleaseを使用してください。
同梱データ・設定を相対パスで参照するため、起動にもリポジトリのルートを使います。
GUIの実行にはウィンドウシステムと対応するGPUドライバーが必要です。

同梱の約5秒の記録を再生します。Inspectorまで紹介画像と同じ状態で試すには、
依存schemaを補完した[撮影用デモ](screenshots.md)を生成してください。
任意の計測成果物は取得せずにビルドできます。[成果物の保存・取得](repository-storage/README.md)と
[既存成果物のLFS移行・旧commit SHAの対応](repository-storage/migration-2026-10-31.md)も参照できます。

## ビルド対象と全体チェック

ルートの既定対象はNativeです。`cargo build` / `cargo run` / `cargo test`はNativeを対象にします。

```sh
cargo build --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo build --release -p recording-server
```

`cargo build --workspace`はブラウザで配信するHTML / JS / Wasm一式を作るコマンドではありません。
Webアプリの配信物にはTrunkを使います。
Web・ROS Live・optional extensionsを含む全検証は[Docker開発環境](development_container.md)で実行できます。
既定のDockerサービスはbuild / test / Web用です。Native GUI/GPUやLive networkingのホスト接続は別設定です。

## Webをホストで起動する

[公開Web版](https://ryo45.github.io/mcap_player/)はビルドなしで使えます。以下はローカルで動かす場合の手順です。
GitHub Pages向けの配信設定は[公開手順](github_pages.md)を参照してください。

ホスト側にツールが揃っている場合は、次の手順でも起動できます。

```sh
rustup target add wasm32-unknown-unknown
# clang-18を用意したLinux環境の例
export CC_wasm32_unknown_unknown=clang-18
export CXX_wasm32_unknown_unknown=clang++-18
cargo install trunk --locked --version 0.21.14
(cd apps/viewer-web && trunk serve --release)
```

WebはNativeと共通のeguiパネルを使用し、Topics / Inspector・Scene / BEV・Camera、速度・ヨーレートPlotとレイアウトJSONに対応します。WebGPU対応ブラウザが必要です。
任意の数値field Plot、Preview、bookmarks、Health / DiagnosticsはNativeのみです。

## DockerでWebを起動する

```sh
docker compose build dev
docker compose run --rm --service-ports dev bash -lc \
  'cd apps/viewer-web && trunk serve --release --address 0.0.0.0'
```

`http://localhost:8080`へ接続します。[Docker開発環境](development_container.md)にツールのバージョンと全体検証の手順があります。
