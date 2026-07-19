# Camera fixture and ROS smoke tools

The checked-in canonical fixture is deterministic, contains 30 320x240 JPEG
frames over 2.9 seconds, and intentionally gives every frame a different
measurement and arrival timestamp.

Regenerate it without ROS:

```bash
cargo run -p ros-fixture -- generate
```

Convert the first five seconds of a raw-image MCAP to the JPEG topic. This
converter is pure Rust and does not require a sourced ROS environment:

```bash
cargo run -p ros-fixture --release -- convert \
  mcap/rosbag2_2026_01_18-17_28_44/rosbag2_2026_01_18-17_28_44_0.mcap \
  mcap/rosbag2_2026_01_18-17_28_44/camera_bev_telemetry_scan_tf_5s.mcap \
  5
cargo run -p ros-fixture --release -- verify \
  mcap/rosbag2_2026_01_18-17_28_44/camera_bev_telemetry_scan_tf_5s.mcap
```

The converter adds `/planning/path` as a deterministic `nav_msgs/msg/Path`
sample alongside the JPEG camera. It is intentionally only a planned line;
LaserScan is not added to the 2D BEV. Real `/odom` and `/scan` messages from
the same arrival-time window are preserved for telemetry and the 3D view.
The latest `/tf_static` before the camera window plus `/tf` and `/tf_static`
inside the window are also preserved. Verification decodes the transform tree
and reports whether `base_scan -> base_footprint` resolves.

The Python converter remains useful when working directly with a rosbag2
directory in a sourced ROS Jazzy shell:

```bash
python3 tools/ros-fixture/camera_fixture.py convert \
  --bag mcap/rosbag2_2026_01_18-17_28_44 \
  --input-topic /camera/image_raw \
  --output tests/fixtures/camera-jpeg/camera_front_3s.mcap
```

Run the bounded live smoke test in two terminals:

```bash
source /opt/ros/jazzy/setup.bash
python3 tools/ros-fixture/camera_fixture.py synthetic
```

```bash
source /opt/ros/jazzy/setup.bash
cargo run -p viewer-native --features ros2-live -- --live
```

For TurtleBot3 Gazebo, launch a `burger_cam` world using the launch package
installed in the ROS environment, then bridge only the raw camera:

```bash
source /opt/ros/jazzy/setup.bash
export TURTLEBOT3_MODEL=burger_cam
ros2 launch turtlebot3_gazebo turtlebot3_world.launch.py
```

```bash
source /opt/ros/jazzy/setup.bash
python3 tools/ros-fixture/camera_fixture.py bridge \
  --input-topic /camera/image_raw \
  --output-topic /camera/front/image/compressed
```

The MVP does not subscribe to `/odom`, `/scan` or TF during this smoke test.
