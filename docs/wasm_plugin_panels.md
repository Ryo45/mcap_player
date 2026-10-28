# Wasm PluginPanel PoC

Native can load an independently built Core Wasm module as an additional Workspace
panel. The `robot-status-panel` example reads a **custom ROS type known only through
the recording's `ros2msg` schema**, selects numeric/string/bool fields, and declares
current values and history plots. No ROS installation is needed for playback.
This is a deliberately small experimental protocol, not a stable plugin framework.

## Build and run

From the repository root, with the normal Native build dependencies installed:

```sh
rustup target add wasm32-unknown-unknown
cargo build --manifest-path examples/robot-status-panel/Cargo.toml \
  --target wasm32-unknown-unknown --release --locked
mkdir -p target/plugin-panel
cargo run -p ros-fixture --example robot_status_fixture --locked -- \
  target/plugin-panel/recording.mcap
cargo run -p viewer-native --release --features plugin-panels -- \
  --mcap target/plugin-panel/recording.mcap --layout explore \
  --plugin-panel examples/robot-status-panel/panel.json
```

The fixture command creates a new file and refuses to overwrite an existing one.
It augments the bundled recording with 45 **synthetic** `my_robot_msgs/msg/RobotStatus`
messages: battery, temperature, state, sequence, enabled and an unused 4 KiB array.
The host contains no corresponding Rust message struct or hardcoded decoder.
The sequence exceeds JavaScript's exact double integer range; its decimal string
crosses the ABI without rounding. History plot values are intentionally doubles.

A manifest contains a module path relative to itself and opaque JSON `settings`.
The sample reads `settings.topic` (default `/robot/status`). A Workspace panel node
uses `panel_type: "plugin"`, `config_version: 1`, `config.manifest: "path/to/panel.json"`.
The CLI adds a right-hand split to the chosen bundled layout. Up to four instances
are allowed. Native's existing layout model owns their IDs and positions; this PoC
does not add a layout editor or persistence UI. CLI manifest paths are resolved
relative to the process working directory, as are existing config paths.

Change only the guest, rebuild it, and click **Reload plugin**. For example:

```sh
cargo build --manifest-path examples/robot-status-panel/Cargo.toml \
  --target wasm32-unknown-unknown --release --locked --features alternate
```

The heading changes to “alternate guest”; the running host does not need rebuilding.
Reload requires the same input definition; changed topics/fields/history requirements
require restarting the viewer, still without rebuilding the host. Invalid plugins
become a placeholder/error in their own panel. Scene extensions remain independent;
`--features scene-extension,plugin-panels` enables both experiments.

## Data flow and ownership

```text
Workspace manifest / independent Wasm.define(settings)
    -> PanelDefinition (fixed topic, selected scalar fields, numeric history fields)
    -> ViewerSession's bounded Query worker
       -> existing McapSource indexed readers / latest-before / numeric overview
       -> cached RosMessageSchema skip plans (no full-message materialization)
    -> owned PanelUpdate snapshot (generation, exact cursor, values, reduced histories)
    -> independent Wasm.update(JSON)
    -> validated PanelOutput (declarative UI + optional user-driven HostCommand)
    -> Workspace PluginPanel -> egui / egui_plot -> existing renderer
```

The panel is a **Query consumer**, not a continuous FeatureRuntime consumer.
Its fixed input demand is configured when opening the session. It does not change
SessionPlan's playback streams, build another MCAP reader/transport implementation,
or route Plot/Inspector through FeatureRuntime. Reads fork the existing indexed
source and use its bounded caches, chunk handling and cancellation mechanisms.

The host compiles selected scalar skip plans once per source/requirements generation.
Numeric histories use `NumericOverviewPlan`, sharing the existing numeric query's
visitor/reducers while reusing its compiled schema/extractors across windows. Latest
value extraction is reused while the predecessor log time stays unchanged. No raw
CDR payload backing is retained by the panel snapshot. String, bool, exact signed/
unsigned integer and finite float values are supported; unrelated fields are skipped.
This projection is explicitly **selected fields**, not a lossless full ROS message.

There is one persistent worker with one active job, one pending job and one completion
slot for all panels. During playback, latest queries target 100 ms buckets (at most
10 Hz in wall time, including fast playback); history windows end on whole log-time
seconds, refreshed at most 1 Hz in wall time. Paused queries target
the exact cursor, with `[start, cursor + 1 ns)` history. Reduced history is at most
60 seconds, four fields and 256 reduced points per field. No complete decoded history is stored. Work is bounded in time
range and retained memory, but a very dense window can still require substantial
chunk I/O/decompression/field extraction. There is no new overview sidecar/cache.

Forward playback can display a completed slightly older query rather than starving
while waiting for a moving cursor. Snapshots never contain a predecessor newer than
their requested cursor. Seek invalidates snapshots only **after successful playback
commit**. Generation cancellation rejects old in-flight work; the panel discards its
output/events and creates a fresh guest instance before displaying a new generation.
Source replacement transfers the worker and advances the generation. Failed queries
return empty values/histories and an explicit status. A failed seek preserves the
previous playback and panel state.

Only query revisions or UI events execute the guest. UI output and plot point buffers
are reused between updates; ordinary repaint does not decode, run Wasm, rebuild plot
points or add custom GPU resources. The normal egui painting/tessellation still runs.
With no configured panels no worker, extra query, decoder or Wasm execution starts.
Default builds do not include the optional Native plugin runtime.

## Protocol and isolation

`viewer-extension::panel` contains portable, serde-encoded models:

- `PanelDefinition`: API version, identity/title, topic, selected fields, history budget.
- `PanelUpdate`: generation/revision, exact timestamp strings, status, scalar map,
  reduced numeric series, errors, settings, click events.
- `PanelOutput`: Text, Value, Row, Column, Plot (references host series), Button.
- `HostCommand`: TogglePlayback or exact-time Seek, accepted only with a user event.

Panel Core Wasm exports `panel_api_version() -> i32` (1), `init()`,
`input_ptr() -> i32`, `output_ptr() -> i32`, `define(input_len) -> output_len`,
`update(input_len) -> output_len`, and linear `memory`. Input/output are UTF-8 JSON
bytes copied between independent memories. Rust struct layout and traits are not
part of the ABI. `define` receives settings; `update` receives the full update model.
WASI/imports/start functions are forbidden. Reset creates a fresh instance and
calls define again, checking the definition is unchanged. Dropping the instance
releases its resources; there is no external resource lifecycle to notify yet.

Native uses the existing **wasmi** dependency, with shared private sandbox mechanics
for Scene and Panel. Panel protocol/versioning is separate from Scene ABI 3.
No additional third-party dependency was required. Limits: module 2 MiB, manifest
8 KiB, VM memory 16 MiB, 5 million fuel per invocation, stack/recursion limits,
input 128 KiB/output 64 KiB, UI 128 nodes/depth 8, individual strings 2 KiB, latest CDR
message 2 MiB. Output schemas, plots, IDs and commands are validated. Traps/fuel
exhaustion/invalid JSON clear the panel output and latch an error until reset/reload;
playback is not rolled back or modified. One click can yield at most one host command.
The guest cannot request arbitrary IO, subscriptions, GPU operations or process exit.

Core Wasm + JSON is easy to inspect and does not require a Rust-specific memory ABI,
WIT tooling or WASI. It incurs bounded copies/parsing costs. Browser should instantiate
the module through JavaScript `WebAssembly.instantiate` as a **separate instance**, not
run wasmi inside the viewer Wasm. The data models compile for wasm32; Browser loading,
Worker execution/cancellation and time-budget enforcement are **not implemented**.
TypeScript/Python will need language-specific adapters and, for an embedded language
runtime, different memory/import budgets; this slice does not establish those runtimes.
Fuel prevents an unbounded Native invocation, but it still runs on the UI thread;
a pathological guest can consume its bounded budget and delay a frame. Worker-based
execution and profiling remain follow-up work. Memory/VM limits do not make this an
OS security boundary or a complete untrusted plugin distribution system.

## Scope and next steps

Only indexed Native file recordings, one stream/schema per input topic, plain CDR1
and `ros2msg` definitions, selected dot-path scalar fields and numeric histories are
implemented. Unknown schemas/arrays as selected values/unsupported encodings produce
an explicit panel error; no whole-file scan fallback is introduced. Distinct panels
currently own separate projections and may repeat extraction/query work for identical
requests. Cross-panel demand deduplication is deferred until multiple real consumers
justify it; history fields within one panel already share chunk reads.

Input acquisition is owned by Query; selected-field decoding is a host projection.
A future raw-CDR input representation can share acquisition/lifecycle while changing
the projection/ABI. It must have separate size/retained-backing limits and will not
replace lightweight built-in codecs. There is no raw-CDR guest mode, dynamic schema
value tree, TypeScript/Python SDK, Custom Panel 3D UI, UI editor, generic message bus,
or dynamic subscription broker in this PoC. Plot nodes currently reference host-query series; guest-generated/derived XY curves
are not implemented. Plot style/layout properties and a stable
public API should follow actual panel use cases rather than precede them.

The next useful step is a second custom message/panel exercising shared inputs,
followed by measured update/serialization budgets and off-UI-thread guest execution.

## Verification

[Recorded Native screenshots, tests and measured costs](verification/plugin-panel/README.md).

Build the sample and generate the recording above, plus two lifecycle fixtures:

```sh
cargo run -p ros-fixture --example robot_status_fixture --locked -- target/plugin-panel/alternate.mcap alternate
cargo run -p ros-fixture --example robot_status_fixture --locked -- target/plugin-panel/malformed.mcap malformed
cargo test -p viewer-extension --locked
cargo test -p viewer-extension actual_robot_status --locked -- --ignored
cargo test -p viewer-native --features scene-extension,plugin-panels --locked
cargo test -p viewer-native --features scene-extension,plugin-panels actual_robot_status --locked -- --ignored
```

Tests cover selected scalar extraction (including exact integers, bounded strings,
large skipped arrays and malformed input), real independently built Wasm, VM reset,
output limits/traps/fuel/commands, latest-before/history, no-predecessor, Seek generation,
Source replacement with an in-flight request, missing topics and decode errors.
The Session integration asserts no added playback routes and unchanged committed
cursor during Query work. `docker/check.sh` builds guests/fixtures and runs these
gates together with existing Native/Web/Scene/ROS-live checks.
