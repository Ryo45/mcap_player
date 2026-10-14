# Development container

The development image uses the official ROS 2 Jazzy `ros-base-noble` image (Ubuntu 24.04),
Rust 1.91.1, clang/lld 18, the `wasm32-unknown-unknown` target, rustfmt/clippy and trunk 0.21.14. CMake/NASM build the statically linked SIMD libjpeg-turbo used by Native.
ROS setup is sourced by the entrypoint, so the `ros2-live` feature can be compiled and tested.
The image builds tools only; source code is mounted at runtime. The build context excludes the
repository, credentials and recordings. The container runs with a non-root UID/GID, and Cargo
registry/git/target caches live in Docker volumes independently of the host toolchain.

```sh
# Set these when your host UID/GID differ from 1000 (Linux).
export MCAP_DEV_UID=$(id -u)
export MCAP_DEV_GID=$(id -g)
docker compose build dev
docker compose run --rm dev

# Run all available repository gates.
docker compose run --rm dev mcap-dev-check

# Individual checks are useful while implementing a change.
docker compose run --rm dev cargo test --workspace --locked
docker compose run --rm dev cargo test -p viewer-native --features ros2-live --locked
docker compose run --rm dev bash -lc 'cd apps/viewer-web && trunk build --release'
docker compose run --rm --service-ports dev bash -lc 'cd apps/viewer-web && trunk serve --address 0.0.0.0'
```

The default service is for builds/tests and the Web development server. Native GUI/GPU and live
ROS networking require explicit host integration (display socket, GPU device, network); these are
not enabled by the default service. Ports 8080/8081 bind to loopback when `--service-ports` is used.
The container does not mount the Docker socket. Source changes are visible on the host; output
files have the configured UID/GID. Changing those IDs requires rebuilding the image; pre-existing
cache volumes retain their original ownership.

Rust/trunk versions are pinned. The ROS image tag and apt package updates track Jazzy/Noble fixes;
the image is not claimed to be bit-for-bit reproducible. Use the same image build for CI and local
verification, and rebuild deliberately when changing tool versions.
