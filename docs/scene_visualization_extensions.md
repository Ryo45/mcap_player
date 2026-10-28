# Scene Visualization Extension — first vertical slice

This branch implements one optional Native Scene extension, not a panel framework.
The first guest takes **Path positions**, creates colored wireframe boxes itself,
and overlays them on the existing Scene. **Custom ROS messages, Detection3DArray,
full PoseStamped fields and Browser loading are not implemented.**

## Investigation and decision

The existing SessionPlan resolves a closed selection from workspace requirements.
FeatureRuntime shares concrete decoders, stages an empty candidate for seek, and
commits only after restore succeeds. Path already provides LatestBefore, metric
XYZ, frame and both times. Camera admission coalesces serialized images before
CDR/pixel work; TF instead retains history and must not use that drop policy.
NativeWorkspace holds feature truth; PresentationState borrows it after commit.
ScenePresentationState places Path/cloud with TF; SceneRenderer uses separate
`revision` and `cloud_revision` buffers. Query, Plot, Health and Preview remain
independent. Tests use encoded CDR fixtures, a shared GPU renderer and the existing
performance-probe representative suite.

Choices considered: a generic typed-message registry and new extension controller;
a dedicated Detection3D decoder; or sharing the existing partial Path feature.
The last is sufficient to prove Wasm-to-Scene without a second decoder or restore
protocol. This is a **named partial projection**, `path_positions_v1`, not a
replacement for a ROS message model. Future typed inputs should preserve relevant
ROS fields using host decoders; do not reinterpret PathFrame as full nav_msgs/Path.

Native first avoids adding async Worker lifecycle to the Web data plane. Core Wasm
with no imports can run as a separate browser WebAssembly instance later. A browser
adapter should run in a dedicated Worker (termination on deadline), use transferable
bounded buffers and generation IDs; never embed wasmi inside viewer WebAssembly.
A main-thread JS instance cannot be reliably interrupted using a promise timeout.

We use [wasmi 2.0](https://docs.rs/wasmi/2.0.0/wasmi/) (Rust 1.86 minimum, compatible with the repository's 1.91 toolchain)
as a small interpreter rather than introducing a JIT/Wasmtime distribution. Fuel
bounds guest instructions; linear memory is capped. This is optional behind
`viewer-native/scene-extension`, so normal Native and Web builds have no VM.
Component Model/WIT and WASI are deferred: no resources, filesystem, network or
system calls are needed. Rust structs/traits are not the wire ABI.

Prior art, consulted as primary sources:

- [RViz Marker](https://github.com/ros2/common_interfaces/blob/jazzy/visualization_msgs/msg/Marker.msg):
  frame/time, IDs, colored lines and cubes are sufficient for the first use case.
  RViz's C++ Display plugin is not a portable browser ABI. The ROS tutorial site
  was access-blocked; the message definition was readable.
- [Foxglove SceneEntity](https://docs.foxglove.dev/docs/sdk/schemas/scene-entity) and
  [extensions](https://docs.foxglove.dev/docs/extensions): separate conversion from
  host Scene drawing and from custom panels; IDs and explicit lifetimes matter.
- [Rerun visualizers](https://rerun.io/docs/concepts/visualization/customize-views):
  layout/Blueprint and visualization are separate concerns; a new custom view is
  unnecessary for an extra layer in an existing view.
- [PlotJuggler parser contract](https://github.com/facontidavide/PlotJuggler/blob/main/plotjuggler_base/include/PlotJuggler/messageparser_base.h):
  parsing, source ingestion and transforms have distinct lifecycles. We do not
  introduce a Plot/Query extension API for this Scene use case.

## Data flow and ownership

```text
startup manifest + panel requirements
    -> workspace require_path + optional_transforms
    -> existing SessionPlan / indexed MCAP selection / restore
    -> existing FeatureRuntime PathController (one CDR decode)
    -> committed PathState revision
    -> WasmExtension / native VisualizationExtension
    -> validated SceneOverlay in the Path frame
    -> host TF at Path measurement time to configured fixed frame
    -> SceneSnapshot.overlay + overlay_revision
    -> existing SceneRenderer camera/depth target, separate line buffer
```

The manifest topic must match the workspace Path binding. This deliberate first
slice does not remap a built-in consumer or silently subscribe to another Path.
Missing/wrong-schema input fails source planning; a file with no predecessor at
the cursor has an empty overlay. Live extension input is explicitly rejected.
Camera selection changes preserve the extension input requirement.

The host owns input selection, CDR decoding, validation, metric coordinates, TF,
GPU resources and error display. The guest chooses objects, sizes and colors.
The Native Rust trait `VisualizationExtension` consumes a borrowed PathFrame and
returns the same `SceneOverlay` without any VM. It is deliberately limited to the
one input projection implemented today.

Only the latest materialized Path is retained. At most one guest update runs per
new Path revision seen by presentation; intermediate revisions in a tick do not
cross the ABI. Built-in Path currently decodes every selected message; this slice
**does not add raw-message coalescing or change built-in Path semantics**. If future
extension-only high-rate inputs are added, admission must precede decode there.
A second consumer of this Path introduces no second CDR decode.

A snapshot freezes in the fixed frame at its measurement-time TF once placement
succeeds (not frame-locked). Later TF eviction/updates do not move or remove it.
Missing TF retries on TF revision changes without rerunning the guest; changing
the target frame also redoes placement. Equal output does
not change the overlay revision. Overlay updates do not rebuild Path/platform or
cloud buffers. Normal rendering/resize/camera operations use the existing renderer.
Seek/source transitions reset the guest to a fresh instance **after successful
playback commit** and discard old output. No guest participates in restore's
fallible source/controller transaction. Errors clear the overlay and disable that
guest until the next reset; the committed cursor and other features remain usable.
There are no asynchronous extension results in this implementation.

## ABI version 1

Manifest JSON (8 KiB maximum): wasm (relative to manifest) and extension descriptor
(id, apiVersion=1, topic, schema=`nav_msgs/msg/Path`, input=`path_positions_v1`). Unknown fields fail. The Rust trait descriptor has no Wasm file path.
The module independently exports `api_version() -> u32`, checked against version 1.

No imports or start function are allowed. Exports:

- `memory`: linear memory, at most 8 MiB, one instance/memory, at most one table with 1024 elements.
- `init()`: initializes guest state; dropping the Store destroys it. Reset creates
  a fresh instance from the already compiled Module, then invokes init again.
- `input_ptr() -> u32`, `output_ptr() -> u32`: guest-owned bounded scratch buffers.
- `update(point_count: u32) -> u32`: returns line count, maximum 4096.

All integers/floats are little endian, no alignment assumptions. Input:
arrival/log time i64, measurement time i64 (nanoseconds), then `point_count`
triples of f64 metric XYZ, maximum 1024 points (24,592 bytes). Positions are in the
Path frame; output must stay in that frame. The host retains frame identity and
uses it for TF, so the guest cannot choose an unrelated output frame. Per-pose
headers and orientations are absent from this explicitly partial input.

Output: `line_count` 40-byte records of id:u32, start:3*f32, end:3*f32,
color:3*f32. IDs must be unique in each complete replacement snapshot. Output is
at most 163,840 bytes. Coordinates must be finite and within +/-1,000,000 metres;
linear RGB must be finite in [0,1]. Host revalidates transformed output too.
No arbitrary memory pointers are dereferenced by the host. Bounds violations,
traps and fuel exhaustion are errors. Each init or update has 1,000,000 fuel;
module bytes <=2 MiB, compiler structural limits, value stack <=256 KiB and
recursion <=128. Fuel is an instruction budget, **not a wall-time deadline**.
Module parsing/compilation is bounded by byte size, not fuel. This is an in-process
VM, not isolation from runtime implementation vulnerabilities.

Initial renderer policy: opaque, one-pixel lines using the existing line pipeline,
depth tested and depth writing. Grid, built-in lines, overlay, then points; opaque
points use depth writes and translucent points retain existing blending. No line
alpha, configurable width, labels or shaders are promised. Box wireframes are
12 lines generated by the guest. IDs reserve replacement/diff semantics but the
initial implementation uploads a whole bounded overlay only when it changes.

## Build and run

```sh
rustup target add wasm32-unknown-unknown
cargo build --manifest-path examples/path-boxes/Cargo.toml \
  --target wasm32-unknown-unknown --release
cargo run --release -p viewer-native --features scene-extension -- \
  --layout monitor --scene-extension examples/path-boxes/extension.json

# Same app and recording without the extension:
cargo run --release -p viewer-native --features scene-extension -- --layout monitor

# Real Rust guest execution (requires the independent build above):
cargo test -p viewer-extension
cargo test -p viewer-extension independent_rust_guest -- --ignored
cargo test -p viewer-native --features scene-extension
```

Use `--mcap FILE` to select a recording. The bundled fixture contains
`/planning/path`; boxes are synthetic class-colored markers along that Path, not
real detections. For another topic, change both workspace binding and manifest,
then rebuild the host. The guest does not depend on this repository or ROS.

## Deliberately deferred

Browser Worker adapter, multiple extensions, Detection3DArray/full ROS types,
generated msg/idl types, extension-only admission, persistent objects/diffs,
labels/mesh/picking, arbitrary shaders, UI/layout and signal transforms are
separate next steps. The next small input step is a host-side bounded typed
Detection3DArray decoder and latest-only admission/restore route, sharing decoding
among consumers, not a universal subscription/decoder registry.

Performance and verification results are recorded in
[the verification note](verification/scene-extension/README.md).
