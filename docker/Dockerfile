FROM ros:jazzy-ros-base-noble

ARG RUST_VERSION=1.91.1
ARG TRUNK_VERSION=0.21.14
ARG DEV_UID=1000
ARG DEV_GID=1000

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential ca-certificates curl git pkg-config clang-18 lld-18 libclang-18-dev \
    libssl-dev libudev-dev libxkbcommon-dev libwayland-dev libx11-dev libxi-dev \
    libxrandr-dev libxcursor-dev libvulkan-dev mesa-vulkan-drivers \
    ros-jazzy-sensor-msgs ros-jazzy-nav-msgs ros-jazzy-tf2-msgs \
    ros-jazzy-rosidl-typesupport-introspection-c \
    && rm -rf /var/lib/apt/lists/*

RUN test "$DEV_UID" -gt 0 && test "$DEV_GID" -gt 0 \
    && if id ubuntu >/dev/null 2>&1 && test "$(id -u ubuntu)" = "$DEV_UID"; then userdel -r ubuntu; fi \
    && if ! getent group "$DEV_GID" >/dev/null; then groupadd --gid "$DEV_GID" dev; fi \
    && useradd --uid "$DEV_UID" --gid "$DEV_GID" --create-home --shell /bin/bash dev \
    && mkdir -p /home/dev/.cargo/registry /home/dev/.cargo/git /home/dev/target /home/dev/.cache \
    && chown -R "$DEV_UID:$DEV_GID" /home/dev

ENV PATH="/home/dev/.cargo/bin:${PATH}" \
    CARGO_TARGET_DIR=/home/dev/target \
    CC_wasm32_unknown_unknown=clang-18 \
    CXX_wasm32_unknown_unknown=clang++-18 \
    CARGO_BUILD_JOBS=2 \
    RUST_BACKTRACE=1

USER dev
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/rustup-init.sh \
    && sh /tmp/rustup-init.sh -y --profile minimal --default-toolchain "$RUST_VERSION" \
    && rustup component add rustfmt clippy \
    && rustup target add wasm32-unknown-unknown \
    && cargo install --locked --version "$TRUNK_VERSION" trunk \
    && rm /tmp/rustup-init.sh

USER root
# rclrs vendor bindings also link its example/test message interface libraries.
RUN apt-get update && apt-get install -y --no-install-recommends \
    ros-jazzy-example-interfaces ros-jazzy-test-msgs \
    && rm -rf /var/lib/apt/lists/*
COPY docker/entrypoint.sh /usr/local/bin/mcap-dev-entrypoint
COPY docker/check.sh /usr/local/bin/mcap-dev-check
RUN chmod +x /usr/local/bin/mcap-dev-entrypoint /usr/local/bin/mcap-dev-check
USER dev
WORKDIR /workspace/mcap_player
ENTRYPOINT ["/usr/local/bin/mcap-dev-entrypoint"]
CMD ["bash"]
