# Scene Visualization Extensions

Optional Native extensions consume selected ROS feature state and return identified
metric Boxes for the existing Scene. Two independently built Wasm examples run:

- `examples/detection-boxes`: **vision_msgs 4.x Detection3DArray** (validated against
  ROS 2 Jazzy 4.1.1), using actual IDs, bbox poses/sizes and classification candidates.
- `examples/path-boxes`: the earlier independent Path-position projection demo.

A bounded host decoder and common guest SDK now connect Detection3DArray all the
way to Scene. The example recording contains **synthetic detections** over the
bundled camera/scan data, not real perception output. Arbitrary custom ROS message
types, Browser plugin loading and live Detection3D subscriptions are not implemented.

## Pipeline and ownership

```text
startup descriptor + panel requirements
    -> fixed SessionPlan union (unique StreamId/LatestBefore requirements)
    -> indexed recording reads
    -> FeatureRuntime admission/coalescing
    -> one host CDR decode -> latest Path / Detection3D feature state
    -> borrowed ExtensionInput::{Path, Detections}
    -> shared wire SDK encode -> independent Wasm instance -> shared SDK decode
    -> guest chooses IDs/geometry/colors -> validated SceneOverlay
    -> measurement-time TF placement -> existing SceneRenderer overlay buffer
```

Only enabled extensions add demands. Named inputs are independent of the displayed
Path binding; multiple continuous consumers of a stream share materialization.
Additional Path and Detection3D inputs use the same latest-in-batch admission
calculation before decode (greatest log time, then final batch order). TF history
and the displayed Path keep their existing policies. No RawMessage backing or
full decoded history is retained by this path. Query/Inspector/Plot/Health/Preview
remain distinct workloads; an Inspector query can independently decode its result.

Detection3DController retains one full message, arrival/log time and revision. Its
empty seek candidate preserves routing, revision baseline and counters. Exact
LatestBefore restoration and malformed-message rollback use the existing feature/
source transaction. A forward decode error increments errors and retains the prior
valid state. Empty detection arrays are valid latest values and clear old objects.

Guest execution follows successful feature commit. Seek/source reset discards old
objects and creates a fresh guest instance. Guest failures clear that overlay and
latch an error until reset; they cannot roll back or corrupt committed playback.
There are no asynchronous plugin results or dynamic subscriptions in this slice.

## ROS facts and projection limits

The host retains the **complete supported ROS model**: array header, every Detection3D
header and string ID, all classification hypotheses/scores, each hypothesis pose
and 36-element covariance, and bbox pose/full XYZ size. No top-class selection or
pose normalization is performed by the decoder. Scores must be finite; they are
not clamped to [0,1]. Pose/covariance components must be finite, bbox sizes nonnegative.

Bounds are enforced before allocation: 2 MiB CDR, 256 detections, 8 results/detection,
256 UTF-8 bytes for frames/IDs and 128 for class IDs. Plain CDR1 little/big endian
is accepted; other encapsulations/trailing old-format fields fail explicitly.
Older vision_msgs layouts with numeric class IDs or source-cloud fields are not
silently interpreted as 4.x. The independent rclpy [codec fixture](../tests/fixtures/ros2-detection3d/README.md)
protects field order/alignment, UTF-8, empty arrays and covariance preservation.

Initial Scene placement requires each detection header to match the array frame
and stamp (leading frame slashes are normalized for comparison). Mixed headers
remain intact in core state but generate an explicit presentation error; per-object
TF placement is future work. A successful snapshot freezes at acquisition-time TF;
missing TF retries on TF revision changes without rerunning the guest. It survives
subsequent TF history eviction. New input/target frame causes a new placement.

Path input remains the explicitly partial `path_positions_v1` projection, not a
full PoseStamped model. It has a 1024-point guest limit; its common CDR decoder still
materializes the latest Path under source/CDR limits. Detection input has an early
sequence cap before its dedicated host allocation.

## Box/ID contract

SceneBox: `SceneObjectId(u64)`, metric center, unit ROS x/y/z/w quaternion, full
positive local-axis size and opaque linear RGB. Complete snapshots replace the
prior snapshot: same ID updates, omitted ID deletes. IDs are unique across Box and
Line objects; canonical ordering avoids uploads for reordering alone. This is not
a persistent scene graph, diff cache, picking or label system.

The detection guest selects the highest-score class: person red, car green, unknown
blue. This policy lives in Wasm, not the host. It hashes nonempty ROS string IDs
into stable 64-bit handles and uses array index for empty IDs. Original strings stay
in the full input model; handles are not reversible IDs. Hash collisions/duplicate
IDs fail output validation; empty-ID fallback is not stable tracking across reorder.

Rigid TF placement transforms center/orientation, preserving size/ID. Renderer
expands Boxes to 12 edges only on overlay revision changes, using the existing
camera, grid, depth target and separate overlay buffer. Opaque one-pixel lines
write/test depth. Draw order: grid, built-ins, overlay, points; point blending/depth
policies stay unchanged. No display scale factors, alpha/variable line widths,
plugin GPU resources or arbitrary shaders are introduced.

## Shared SDK and ABI 3

Rust plugins implement VisualizationExtension using borrowed typed inputs.
`viewer-extension::wire` provides host **encode_input**, guest **decode_input** and
**encode_boxes**. It is a bounded, explicit little-endian wire codec, **not CDR**.
Guests neither parse ROS CDR nor instantiate a VM. The two examples share this SDK;
no new generic registry, UI framework or universal decoded-message bus is needed.

The native wasmi adapter retains the 8 MiB linear-memory/1M fuel limits. Compilation
is bounded by a 2 MiB module and strict structural limits; value stack <=256 KiB,
recursion <=128, one table <=1024 entries. Fuel is not a wall-time deadline. Compiled
code, host buffers, GPU and old/new instance overlap during reset are additional
memory. Errors include forbidden import names to make guest build problems visible.

Core's **timing** feature is enabled by default for normal Native/Web. Guest SDK
builds disable it because web-time/wasm-bindgen otherwise introduces browser imports.
Only stage timing instrumentation is gated; playback/admission/restore algorithms
are unchanged. Guests build in their own workspace with core default-features=false,
so host feature unification cannot add JS bindings to their modules. Actual wasmi
loading tests reject every import, including JS and WASI.

Manifest <=8 KiB: module path (relative to manifest) and descriptor id, apiVersion=3,
topic, schema, input. Supported schema/projection pairs are Path/path_positions_v1
and Detection3DArray/detection3d_array_v1. **ABI 1/2 guests must be rebuilt**; no legacy
wire interpreter remains. Box output stays the 60-byte record introduced in ABI 2.

Core Wasm exports: memory, api_version()->3, init(), input_ptr(), output_ptr(), and
**update(input_byte_length:u32)->box_count:u32**. No start/imports, WASI, filesystem,
network or GPU calls. Input <=2 MiB; all integers/f64 are little endian, strings are
u32 byte length + UTF-8 without padding/NUL. Kind:u32 (1 Path, 2 detections), then
arrival:i64. Path adds stamp:i64, frame:string, count:u32, XYZ:f64 triples. Detection
adds array header (stamp:i64/frame:string), count:u32, then each detection's header,
id:string, bbox position:3*f64/quaternion:4*f64/size:3*f64, results count:u32; each
result is class:string, score:f64, position:3*f64, quaternion:4*f64, covariance:36*f64.
The SDK rejects unknown tags, trailing/truncated data and all counts/strings over
limits. No Rust structure layout/pointers cross instances.

Output <=256 Boxes: id:u64, center:3*f32, quaternion:4*f32, size:3*f32, color:3*f32
(60 bytes each). Native Rust can also emit up to 4096 Lines. Host validates finite
coordinates/corners within +/-1,000,000 m, positive finite sizes, unit squared-norm
tolerance 1e-5, colors [0,1] and unique IDs, again after TF. Invalid display geometry
fails instead of changing physical scale or fabricating a pose.

A future Browser adapter should use a separate JS WebAssembly instance in a Worker,
transfer bounded buffers, scope results by generation and terminate on deadlines.
Do not put wasmi inside the viewer Wasm or assume a main-thread promise can interrupt
a running guest. WASI/Component Model/WIT is unnecessary for this current buffer API.

## Build and run

```sh
rustup target add wasm32-unknown-unknown
cargo build --manifest-path examples/detection-boxes/Cargo.toml \
  --target wasm32-unknown-unknown --release
mkdir -p target/detection3d/recording
# Once; the tool refuses to overwrite an existing recording.
cargo run -p ros-fixture --example detection3d_fixture -- \
  target/detection3d/recording/recording.mcap
cargo run --release -p viewer-native --features scene-extension -- \
  --mcap target/detection3d/recording/recording.mcap --layout monitor \
  --scene-extension examples/detection-boxes/extension.json \
  --inspect-topic /perception/detections

cargo test -p viewer-core --test detection3d
cargo test -p viewer-extension
cargo test -p viewer-extension actual_detection_guest -- --ignored
cargo test -p viewer-native --features scene-extension actual_detection3d_recording -- --ignored
# Always run the checked-out verification script, even with an older cached image:
docker compose run --rm dev bash docker/check.sh
```

The recording adds 23 synthetic detection frames, eight tracked objects and two
hypotheses per object, at half Path rate and +5 ms log-time offset. Recording playback
and tests using committed bytes require **no ROS installation**. ROS is only needed
to regenerate the optional independent rclpy fixture, not to run the viewer.

The Path example still works after its independent ABI 3 build; generate the prior
`scene_extension_fixture` and use its manifest/recording. Existing Box/Path evidence
is [archived](verification/scene-extension-independent/README.md).
Current [Detection3D end-to-end verification and screenshots](verification/detection3d/README.md)
record actual tests, performance and limitations.

Next useful steps: per-detection frame/time placement, an explicit stable string-ID
metadata/picking mapping, generated msg/idl types or Browser Worker host when needed.
Arbitrary custom decoders, Live detection subscriptions, labels/meshes, GPU shaders,
persistent diffs, UI/layout and Signal Transform remain separate requirements.
