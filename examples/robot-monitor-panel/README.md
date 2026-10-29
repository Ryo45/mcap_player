# Robot / perception monitor (Panel API 2)

An independently compiled Wasm panel combining **two custom ROS topics**:

- `status`: `/robot/status` — selected battery/state values and battery/temperature history.
- `tracks`: `/perception/tracks` — a variable number of objects and variable stage timings.

`panel.json` settings `statusTopic` / `tracksTopic` change the topics without rebuilding
the host. The guest assembles Text, Value, Row, Column, Plot and Button nodes; it has
no CDR decoder, MCAP reader, egui dependency or GPU access. It displays at most eight
objects and explicitly distinguishes an empty object array from a missing/error input.

The second message's schema (embedded in the generated MCAP) is conceptually:

```text
# my_robot_msgs/msg/TrackedObjects
uint32 frame
my_robot_msgs/TrackedObject[] objects
float32[] timings_ms

# my_robot_msgs/msg/TrackedObject
uint64 id
string label
float64 confidence
float64 distance
```

The host projects `objects[].id`, `objects[].label`, `objects[].confidence`,
`objects[].distance` and `timings_ms[]`. Each projected array retains its order and
length; the guest combines object columns by index. IDs stay exact above 2^53.
Each input has an independent predecessor log time: this is **not a synchronized join**.

From the repository root:

```sh
rustup target add wasm32-unknown-unknown
cargo build --manifest-path examples/robot-monitor-panel/Cargo.toml \
  --target wasm32-unknown-unknown --release --locked
mkdir -p target/plugin-panel target/plugin-multi-input
# Skip commands for fixture files that already exist; generators refuse overwrite.
cargo run -p ros-fixture --example robot_status_fixture --locked -- target/plugin-panel/recording.mcap
cargo run -p ros-fixture --example robot_monitor_fixture --locked -- target/plugin-multi-input/recording.mcap
cargo run -p viewer-native --release --features plugin-panels -- \
  --mcap target/plugin-multi-input/recording.mcap --layout scene \
  --plugin-panel examples/robot-monitor-panel/panel.json
```

`--layout explore` instead selects no continuous Scene/camera streams. The panel's
Query inputs are identical in either layout. The fixture adds 15 slower perception
messages to the 45 status messages in the five-second bundled recording. Object
counts cycle 0, 1, 2, 3 and stage timings cycle 1, 2, 3. Both are synthetic test data.
The generator also supports `alternate`, `malformed`, `oversized`, `missing-status`,
`missing-tracks` as a second argument. All files are indexed MCAPs.

[Host architecture and bounds](../../docs/wasm_plugin_panels.md) ·
[Verification and screenshots](../../docs/verification/plugin-panel-arrays/README.md).
