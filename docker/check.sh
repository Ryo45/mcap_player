#!/usr/bin/env bash
set -euo pipefail
cd /workspace/mcap_player
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy -p viewer-native --all-targets --features scene-extension,plugin-panels --locked -- -D warnings
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
cargo fmt --manifest-path examples/robot-status-panel/Cargo.toml -- --check
mkdir -p target/plugin-panel
CARGO_TARGET_DIR="$PWD/examples/robot-status-panel/target" cargo build --manifest-path examples/robot-status-panel/Cargo.toml --target wasm32-unknown-unknown --release --locked --features alternate
cp examples/robot-status-panel/target/wasm32-unknown-unknown/release/robot_status_panel.wasm target/plugin-panel/alternate-guest.wasm
CARGO_TARGET_DIR="$PWD/examples/robot-status-panel/target" cargo build --manifest-path examples/robot-status-panel/Cargo.toml --target wasm32-unknown-unknown --release --locked
cargo test -p viewer-extension --locked actual_robot_status -- --ignored
for mode in recording alternate malformed; do
    if [ ! -f "target/plugin-panel/$mode.mcap" ]; then
        cargo run -p ros-fixture --example robot_status_fixture --locked -- "target/plugin-panel/$mode.mcap" "$mode"
    fi
done
cargo fmt --manifest-path examples/robot-monitor-panel/Cargo.toml -- --check
CARGO_TARGET_DIR="$PWD/examples/robot-monitor-panel/target" cargo build --manifest-path examples/robot-monitor-panel/Cargo.toml --target wasm32-unknown-unknown --release --locked
mkdir -p target/plugin-multi-input
for mode in recording alternate malformed oversized missing-status missing-tracks; do
    if [ ! -f "target/plugin-multi-input/$mode.mcap" ]; then
        cargo run -p ros-fixture --example robot_monitor_fixture --locked -- "target/plugin-multi-input/$mode.mcap" "$mode"
    fi
done
cargo test -p viewer-native --features scene-extension,plugin-panels --locked
cargo test -p viewer-native --features scene-extension,plugin-panels --locked actual_robot_ -- --ignored
cargo check -p viewer-core --no-default-features --locked
cargo check -p viewer-core -p viewer-extension -p viewer-mcap -p viewer-renderer -p bev-renderer -p scene-renderer -p viewer-web --locked --target wasm32-unknown-unknown
(cd apps/viewer-web && trunk build --release)
cargo test -p viewer-native --locked --features ros2-live
