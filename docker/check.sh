#!/usr/bin/env bash
set -euo pipefail
cd /workspace/mcap_player
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo check -p viewer-core -p viewer-renderer -p bev-renderer -p scene-renderer -p viewer-web --locked --target wasm32-unknown-unknown
(cd apps/viewer-web && trunk build --release)
cargo test -p viewer-native --locked --features ros2-live
