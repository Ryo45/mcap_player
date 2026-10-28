#!/usr/bin/env bash
set -euo pipefail
cd /workspace/mcap_player
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy -p viewer-native --all-targets --features scene-extension --locked -- -D warnings
cargo test --workspace --locked
CARGO_TARGET_DIR="$PWD/examples/path-boxes/target" cargo build --manifest-path examples/path-boxes/Cargo.toml --target wasm32-unknown-unknown --release --locked
CARGO_TARGET_DIR="$PWD/examples/detection-boxes/target" cargo build --manifest-path examples/detection-boxes/Cargo.toml --target wasm32-unknown-unknown --release --locked
cargo test -p viewer-extension --locked independent_rust_guest -- --ignored
cargo test -p viewer-extension --locked actual_detection_guest -- --ignored
cargo test -p viewer-native --features scene-extension --locked
mkdir -p target/scene-extension-independent/staggered
if [ ! -f target/scene-extension-independent/staggered/recording.mcap ]; then
    cargo run -p ros-fixture --example scene_extension_fixture --locked -- target/scene-extension-independent/staggered/recording.mcap
fi
cargo test -p viewer-native --features scene-extension --locked actual_wasm_indexed_seek -- --ignored
mkdir -p target/detection3d/recording
if [ ! -f target/detection3d/recording/recording.mcap ]; then
    cargo run -p ros-fixture --example detection3d_fixture --locked -- target/detection3d/recording/recording.mcap
fi
cargo test -p viewer-native --features scene-extension --locked actual_detection3d_recording -- --ignored
cargo check -p viewer-core --no-default-features --locked
cargo check -p viewer-core -p viewer-extension -p viewer-mcap -p viewer-renderer -p bev-renderer -p scene-renderer -p viewer-web --locked --target wasm32-unknown-unknown
(cd apps/viewer-web && trunk build --release)
cargo test -p viewer-native --locked --features ros2-live
