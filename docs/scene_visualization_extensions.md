# Scene Visualization Extension — independent input and Box contract

One optional Native Scene extension runs an independently built Wasm guest.
The current example consumes `/perception/objects` as **Path positions**, independently
of the Scene's `/planning/path`, and returns oriented Boxes with Object IDs.
**Custom ROS types, full PoseStamped fields, real detector/track IDs and Browser
extension loading are not implemented.** This fixture isolates input routing from
adding another decoder; it does not claim detection-message compatibility.

## Architecture and decisions

```text
panel requirements + startup extension descriptor (topic, schema, projection)
    -> PlaybackRequirements (fixed named Path input)
    -> SessionPlan (union of required streams; unique restore inputs)
    -> indexed MCAP forward/LatestBefore reads
    -> FeatureRuntime
         displayed PathController: existing ordered behavior
         additional PathControllers: latest-only per source batch before decode
    -> committed, separate PathState per StreamId (one decoder/materialization)
    -> Native presentation selects the descriptor's resolved input StreamId
    -> WasmExtension / native VisualizationExtension
    -> validated SceneOverlay { lines, boxes }, in the input frame
    -> acquisition-time rigid TF placement to the configured fixed frame
    -> independent overlay_revision
    -> existing SceneRenderer (Box -> 12 edges only when overlay changes)
```

Previously the extension was forced to share the displayed Path binding. That
proved execution/rendering but not independent input selection. We now resolve
named Path demands at session initialization without changing WorkspaceBindings.
`SessionPlan::path_stream_for_topic` resolves the extension input; the Native
workspace stores its StreamId on source configuration. Camera route reconfiguration
retains the same extension requirement. There is no running subscription broker.

Same-stream consumers borrow the built-in PathController if selected, or one
additional controller otherwise. Duplicate topic demands do not add physical
streams, restore reads or decodes. Additional inputs are capped at 32 explicit
Path topics, not a registry of arbitrary message types. Disabled extensions add
no topic demand, extra controller, CDR decode or VM execution.

For independent latest-value inputs, FeatureRuntime marks routed serialized
messages as consumed but decodes only the greatest log time in a source batch;
equal log times choose the final message in the source's deterministic batch
order. No pending RawMessage/chunk backing survives the call. Dropped messages
increment `dropped`, not `decoded` or decode errors. Malformed latest input follows
existing forward tolerance: error count increases and the prior valid state remains.
The displayed Path's semantics remain unchanged. TF retains history as before.

Both Path roles use the existing common lightweight `decode_path` and the same
empty restore-candidate mechanism. Independent inputs receive exact LatestBefore
restoration even when their publication times differ from the displayed Path.
Malformed restore of either input fails the entire candidate without changing
cursor or either authoritative feature state. Missing/wrong-schema required topics
fail source planning; absent predecessors produce empty feature/overlay state.
Queries, Plot, Health, Preview and pixel decoding remain separate.

The host owns selection, CDR, metric coordinates, TF, bounds, GPU and error display.
The guest owns object identity, dimensions, orientation and coloring. The Rust
trait accepts a borrowed PathFrame and returns the same SceneOverlay without a VM.
`path_positions_v1` is explicitly a partial projection: positions, frame and times;
per-pose headers/orientations are not available. The 1024-point limit is checked
at the VM boundary; the shared CDR decoder still materializes the full latest
Path under existing source/CDR limits, without a separate early point-count cap. Future host typed decoders should
preserve ROS meaning instead of pretending PathFrame is a full ROS Path.

## Box and object lifetime

`viewer-core::SceneBox` contains:

- `SceneObjectId(u64)`: stable within one extension, shared ID namespace with lines.
- `center`: metric XYZ in the input frame.
- `orientation`: unit ROS x/y/z/w quaternion.
- `size`: full positive extent along the box's local X/Y/Z axes, in metres.
- `color`: opaque linear RGB.

`SceneOverlay` is a complete replacement snapshot. Reusing an ID updates that
object; IDs omitted from the next snapshot disappear. No decoded or object history
is retained, and no persistent scene graph/diff protocol is implied. Duplicate IDs
(including across Box/Line kinds) fail validation. Canonical ID ordering means
output reordering alone does not cause a new revision/GPU upload. Picking, labels,
class/track metadata and host-side per-object diffs are deferred.

Host placement resolves a rigid transform once per snapshot. It transforms centres
and composes orientations, preserving sizes and IDs; the pure RigidTransform API
reuses existing TF math. Renderer expansion uses the Box corners. There is no
per-platform scale correction or plugin-defined GPU resource.

Once successfully placed, a snapshot freezes at measurement-time TF, not frame
locked. Later TF eviction/updates do not move/remove it. Missing TF retries when
TF changes without rerunning the guest. A new input revision or target frame causes
placement to be reconsidered. Seek/source transitions drop old output and create a
fresh guest instance **after successful playback commit**. Guest failures clear
its overlay and disable it until reset, leaving playback and other features usable.
No asynchronous guest results exist in this implementation.

## Runtime and ABI version 2

Native uses optional [wasmi 2.0](https://docs.rs/wasmi/2.0.0/wasmi/) (Rust >=1.86),
a fuel-metered interpreter instead of a JIT distribution. Ordinary Native builds
and the viewer WebAssembly contain no VM. The crate's descriptor/Rust contracts
also compile to WASM without the Native runtime dependency.

Core Wasm, no imports/start function; no WASI, filesystem, network, GPU or shaders.
Component Model/WIT adds little for this one bounded buffer exchange and is deferred.
Rust structures/traits are not the wire ABI. A Browser adapter should instantiate
the guest separately through JavaScript in a dedicated Worker, use transferable
bounded buffers and generation IDs, and terminate the Worker on deadline. Do not
embed wasmi in the viewer Wasm; a main-thread promise timeout cannot interrupt a
running guest. Browser loading/Worker lifecycle remains unimplemented.

Manifest <=8 KiB:

```json
{
  "wasm": "target/wasm32-unknown-unknown/release/path_boxes.wasm",
  "extension": {
    "id": "example.path-boxes",
    "apiVersion": 2,
    "topic": "/perception/objects",
    "schema": "nav_msgs/msg/Path",
    "input": "path_positions_v1"
  }
}
```

Module path is relative to the manifest. Unknown fields and incompatible versions
fail. The Rust descriptor has no Wasm file path. **ABI 1 line-record guests must be
rebuilt**: no legacy interpreter path is retained in this pre-1.0 experiment.

Exports:

- `memory`, `api_version() -> u32` (=2), `init()`.
- `input_ptr() -> u32`, `output_ptr() -> u32`: guest scratch buffers.
- `update(point_count:u32) -> u32`: complete output Box count, <=256.

Input is unchanged `path_positions_v1`: little-endian arrival/log i64 and
measurement i64 nanoseconds, then at most 1024 XYZ f64 triples (24,592 bytes).
Frame identity stays host-owned; output is in the same source frame.
Output is explicitly encoded little endian, **60 bytes per Box**:
`id:u64, center:3*f32, orientation:4*f32, size:3*f32, color:3*f32`.
256 Boxes occupy at most 15,360 bytes. This guest emits up to 32 Boxes (1,920 bytes),
where ABI 1 emitted their 384 edges (15,360 bytes). Native Rust output can also
include up to 4096 Line objects; this Wasm ABI currently returns Boxes only.

Host validates finite metric coordinates/corners within +/-1,000,000 m, positive
finite sizes, unit quaternion (squared norm tolerance 1e-5), RGB in [0,1], unique
IDs and counts, before rendering and after TF placement. Invalid pointers, traps
and fuel exhaustion are errors. Limits: 2 MiB module, strict compiler structural
limits, 8 MiB guest linear memory, one memory/table with <=1024 table entries,
256 KiB value stack, recursion <=128, 1M fuel per init/update. Fuel is **not a
wall-time deadline**; compilation/host allocations/GPU and temporary reset overlap
are outside linear memory. This remains an in-process VM.

Renderer policy: opaque one-pixel depth-tested, depth-writing wireframes. Grid,
built-in lines, overlay then points; existing point blending/depth behavior stays.
Overlay uses one separate buffer and only uploads on overlay revision change.
No alpha/variable width, arbitrary shaders, custom UI or direct wgpu access.

## Reproduce

```sh
rustup target add wasm32-unknown-unknown
cargo build --manifest-path examples/path-boxes/Cargo.toml \
  --target wasm32-unknown-unknown --release
mkdir -p target/scene-extension-independent/staggered
# Generate once; the tool refuses to overwrite an existing file.
cargo run -p ros-fixture --example scene_extension_fixture -- \
  target/scene-extension-independent/staggered/recording.mcap
cargo run --release -p viewer-native --features scene-extension -- \
  --mcap target/scene-extension-independent/staggered/recording.mcap --layout monitor \
  --scene-extension examples/path-boxes/extension.json

cargo test -p viewer-core --test path_inputs
cargo test -p viewer-extension
cargo test -p viewer-extension independent_rust_guest -- --ignored
cargo test -p viewer-native --features scene-extension
cargo test -p viewer-native --features scene-extension actual_wasm_indexed_seek -- --ignored
```

The generated fixture preserves the built-in Path and adds eight synthetic objects
2 m to its left, using a separate Path topic at half the publication rate and with
a 5 ms log-time offset. The original measurement/header stamp is preserved.
Guest IDs derive from position index;
they are demonstration identities, not detector tracking IDs. Both topics and their
independent latest-before values can be inspected. Ordinary playback needs no ROS
installation. Live extensions remain explicitly unsupported.

## Prior art and next step

[RViz Marker](https://github.com/ros2/common_interfaces/blob/jazzy/visualization_msgs/msg/Marker.msg)
provides the useful frame/time, object identity and shape precedent;
[Foxglove SceneEntity](https://docs.foxglove.dev/docs/sdk/schemas/scene-entity) and
[extensions](https://docs.foxglove.dev/docs/extensions) separate message conversion
from host rendering/custom panels. [Rerun visualizers](https://rerun.io/docs/concepts/visualization/customize-views)
keep Blueprint/layout distinct from visualization.
[PlotJuggler parsers](https://github.com/facontidavide/PlotJuggler/blob/main/plotjuggler_base/include/PlotJuggler/messageparser_base.h)
have a different lifetime from sources/transforms. These are design references,
not compatibility claims or justification for recreating a generic plugin system.

Next input step: a bounded host-side Detection3DArray decoder retaining IDs/pose/
extents and latest-only restore/admission, with typed ROS semantics. Browser
Workers, multiple runners, generated msg/idl types, labels/mesh/picking, persistent
diffs, UI/layout and Signal Transform remain separate measured requirements.

See [independent-input/Box verification](verification/scene-extension-independent/README.md)
and [the historical ABI 1 results](verification/scene-extension/README.md).
