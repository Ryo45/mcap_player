# MCAP JPEG Camera + BEV MVP

Native and browser viewers share `McapPlayback<B>`, including the MCAP catalog,
playback clock, pipeline bindings and `DomainState` for Camera, BEV, Telemetry,
PointCloud and TF. Native supplies an mmap while Web supplies a `Vec<u8>`.
The camera wall is catalog-driven: clicking a thumbnail selects that camera in
the focus panel, and adding another `CompressedImage` topic requires no new
renderer path. Raw
`sensor_msgs/msg/Image`, H.264, remote URLs and web live input are not
supported.

Native playback displays the JPEG camera beside a GPU-rendered BEV with a
metric grid and fixed ego footprint. The BEV target follows panel size without
re-uploading scene layers.

Native also includes a perspective 3D view below the Camera/BEV row. It renders
a world grid, an odometry-driven ego wireframe and the planned path using a
depth-tested offscreen target. Real `/scan` points are shown only in this 3D
view. At acquisition time, the viewer resolves `base_scan -> odom` through
measurement-time-indexed `/tf_static` and `/tf`. `DomainState` retains the raw
scan, while the stateful scene snapshot builder transforms each new scan once
and caches the resulting world coordinates. A scan with missing TF remains
available for a retry when TF arrives. Later ego-pose updates never transform
historical points.
`Accumulate scans` switches between the latest scan and bounded,
odometry-anchored scan history; seek and file reload clear that history.
The 3D camera defaults to a vehicle-following rear/right chase view. The view
selector also provides a mouse-orbiting free view and a forward vehicle-eye
view. Hover the 3D panel and use the mouse wheel to zoom in chase/free mode;
left-drag orbits in free mode, and double-click resets the camera. Camera-only
changes update the view uniform without re-uploading scene or point buffers.

The sample fixture contains seven JPEG camera topics. Native shows them as a
camera wall; click a thumbnail to select the focused view. Additional
`sensor_msgs/msg/CompressedImage` topics are discovered from the MCAP catalog
and follow the same path automatically.

The planned path is projected onto every camera image. The path is interpreted
in its ROS `base_link` frame, while each `CompressedImage.header.frame_id`
selects its `base_link -> camera optical frame` transform from `/tf_static`.
Pinhole intrinsics and `plumb_bob` distortion coefficients are loaded from
`config/camera_calibration.json`. The bundled coefficients are zero (ideal
pinhole), but non-zero coefficients use the same projection path. Native can
select another file with `--camera-calibration FILE`; Web embeds the same
default file in its production bundle.

For the Camera + fixed dummy path sample, run:

```bash
cargo run -p viewer-native -- \
  --mcap tests/fixtures/camera-jpeg/camera_7_5s.mcap \
  --camera-topic /camera/front/image/compressed \
  --camera-calibration config/camera_calibration.json
```

The BEV path is read from `/planning/path` as `nav_msgs/msg/Path`. The telemetry
panel reads real `/odom` messages and shows position, heading, speed and yaw
rate at the playback cursor.

## Native playback

The bundled fixture is used when no arguments are provided:

```bash
cargo run -p viewer-native
```

The canonical explicit command is:

```bash
cargo run -p viewer-native -- \
  --mcap tests/fixtures/camera-jpeg/camera_7_5s.mcap \
  --camera-topic /camera/front/image/compressed
```

An `.mcap` file can also be dropped onto the running window. Resize changes
only GPU sampling/layout; it does not decode or CPU-resize the JPEG again.

## Browser playback

Install Trunk and the Rust WASM target, then serve the web package:

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk
cd apps/viewer-web
trunk serve --release
```

Open the page and choose
`tests/fixtures/camera-jpeg/camera_7_5s.mcap` to play
the camera, dummy planned path, real odometry, scan and TF streams together.
The browser camera panel matches Native: it shows a focused frame and a
catalog-driven thumbnail row; click a thumbnail to change focus.
The browser reads the complete local file, which is intentional for this
small-file MVP.

## ROS 2 live

Source ROS Jazzy and build the isolated feature:

```bash
source /opt/ros/jazzy/setup.bash
cargo run -p viewer-native --features ros2-live -- \
  --live --camera-topic /camera/front/image/compressed
```

Add `--reliable` to replace the default sensor-data best-effort/volatile QoS.
The ROS executor runs on its own thread. Its callback records Unix arrival time
before introspection, reconstructs a CDR payload, and writes to a capacity-one
latest mailbox; domain state and GPU writes remain on the application thread.

See [tools/ros-fixture/README.md](tools/ros-fixture/README.md) for synthetic and
TurtleBot3 smoke procedures.

## Required checks

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check -p viewer-core -p viewer-renderer -p bev-renderer -p scene-renderer --target wasm32-unknown-unknown
cd apps/viewer-web && trunk build --release
source /opt/ros/jazzy/setup.bash
cargo test -p viewer-native --features ros2-live
```
