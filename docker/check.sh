#!/usr/bin/env bash
set -euo pipefail
cd /workspace/mcap_player
python3 -m unittest discover -s tools/repository-storage -p 'test_*.py'
python3 tools/repository-storage/storage.py check
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
mkdir -p target/workspace-bindings
if [ ! -f target/workspace-bindings/recording.mcap ]; then
    cargo run -p ros-fixture --example workspace_bindings_fixture --locked -- target/workspace-bindings/recording.mcap
fi
cargo test -p viewer-native --locked actual_workspace_bindings_recording -- --ignored
cargo check -p viewer-core -p viewer-mcap -p viewer-renderer -p bev-renderer -p scene-renderer -p viewer-web --locked --target wasm32-unknown-unknown
(cd apps/viewer-web && trunk build --release)
cargo test -p viewer-native --locked --features ros2-live
