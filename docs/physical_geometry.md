# Physical geometry and TurtleBot3

All physical positions and translations use metres: **1.0 = 1 metre**. Path,
LaserScan, TF, platform bounds and the grid have no independent display multipliers.
Viewport zoom, perspective and pixel stroke sizes are presentation choices.

## Independent facts and policies

`viewer-core::PlatformGeometry` contains a named reference frame and asymmetric
ROS xyz `min`/`max` bounds. It validates finite, ordered bounds. 8 ROS-frame box corners are derived from those bounds. BEV footprint coordinates
and renderer precision conversion belong to presentation. Geometry does not contain camera
calibration, topics, layout, sensor mounts or query/render settings.

`PathMessage` and `PathFrame` retain f64 xyz positions in the source frame.
`PathController` preserves them without BEV conversion or ground offsets. This
viewer currently renders the position polyline, not the PoseStamped orientations.
Empty pose frame names inherit the Path frame; different nonempty frames are an
explicit unsupported-format error rather than silently reinterpreted positions.

`PathProjectionPolicy` selects a fixed world frame and an optional ground frame.
The Path is anchored using its header measurement time. Camera projection then
transforms the anchored world positions at the image measurement time. BEV uses
the current platform reference pose. Scan points still use their acquisition-time
TF; body corners use the current Odometry measurement time (Scan/Path time when
Odometry is absent). MCAP log time remains the playback/seek timeline.

Scene and BEV borrow the same presentation products. Scene receives world xyz
metre positions, converting only axis convention to `[-left, up, -forward]`.
BEV receives right/forward positions and the physical footprint in its reference
frame. Renderers know no robot model names. Missing TF hides the affected physical
geometry and exposes its route; the viewer never substitutes the latest pose for
an unavailable historical transform.

Ground projection converts into the selected TF frame, replaces z with zero in
that frame, and converts onward. The default preserves the recorded z. For the
sample, `base_footprint` provides the ground reference; its TF to `base_link` is
0.01 m. No copy of that offset is stored in platform settings or PathController.

## TurtleBot3 Waffle Pi example

The bundled recording's Scan, camera and caster transforms match the Waffle Pi
URDF. The default example is [turtlebot3_waffle_pi.json](../config/platform_geometry/turtlebot3_waffle_pi.json):

```text
reference_frame = base_link
forward: [-0.197, 0.0855] m
left:    [-0.153, 0.153] m
up:      [-0.010, 0.13125] m
extent:   0.2825 × 0.306 × 0.14125 m
```

These are an axis-aligned envelope of the official URDF's collision primitives,
not a detailed mesh or a claim of measured dimensions for every TurtleBot3.
The source is the [ROBOTIS Waffle Pi URDF](https://github.com/ROBOTIS-GIT/turtlebot3/blob/main/turtlebot3_description/urdf/turtlebot3_waffle_pi.urdf)
(checked 2026-10-13). The base box is centred at `[-0.064, 0, 0.047]`, with size
`[0.266, 0.266, 0.094]`; the wheels extend sideways to ±0.153 and down to -0.010;
the camera collision extends forward to 0.0855; the LDS collision reaches 0.13125.
Their union gives the bounds above. These primitive dimensions are already SI;
URDF mesh asset unit conversion is not a viewer display correction.

This default is for the bundled example. Topic/frame names do not identify robot
hardware: supply different bounds for Burger, a modified robot, or a car, or hide
the body when its physical shape is unknown. Rear-axle origins need asymmetric
forward bounds, not a renderer-specific shift.

## Native and Web configuration

Native:

```sh
cargo run -p viewer-native -- --layout monitor \
  --platform-geometry config/platform_geometry/turtlebot3_waffle_pi.json \
  --fixed-frame odom --ground-frame base_footprint
```

Omit `--ground-frame` to retain the authored z. `--no-platform-geometry` disables
the body; `--platform-geometry FILE` supplies another source-neutral shape.
`--camera-calibration FILE` remains separate. Presentation JSON reads are capped
at 64 KiB. A bad geometry is an explicit startup error; missing/invalid camera
calibration disables the overlay while allowing recording/Scene use.

Web's Scene settings expose separate platform and camera JSON inputs, a body
visibility checkbox, world frame and optional ground frame. Invalid uploads
leave the last accepted settings intact and show an error. Independent upload
generations reject stale completion. Changing calibration resets overlay caches;
projection policy and geometry changes invalidate their affected presentation.

## Calibration and sample data

The bundled calibration identifies itself as **demo calibration, not measured**.
Its intrinsics/distortion are not validated against real CameraInfo. Provenance
is shown with overlay status. A supplied file without provenance is labelled
unspecified; no unknown camera silently borrows another camera's calibration.
Projection still applies TF extrinsics, pinhole intrinsics and distortion.
Image-size conversion assumes a resize of the calibration image, not a crop.

The sample Path is generated: 31 positions from 0 to 15 m. Its length remains
15 m; the actual Scan readings are about 0.12–2.98 m. A long generated plan is not
fixed by shrinking the renderer. The seven camera images are derivative display
fixtures, not seven independently calibrated real cameras.

## Validation and limits

`viewer-core/tests/physical_geometry.rs` exercises CDR → controllers → TF → Scene
with a 1 m elevated Path, valid 1/2 m Scan readings, a 10/20 m translated and rotated
world pose, the 0.01 m footprint offset, and the physical body envelope. It checks
path distance, preserved z, Scan range, transformed height, asymmetric footprint,
world-frame paths, mixed frame rejection and cache invalidation. Renderer tests
check 1 m grid segments and distance-preserving axis conversion. Grid lines use
short metric segments to avoid long-line near-plane clipping at close zoom.

The existing dynamic TF store remains bounded to its current one-second arrival
history, with exact measurement-time predecessor lookup. Sparse old body-frame
Paths whose anchor TF is no longer available are visibly unavailable in the
projection; there is no unbounded history or scan fallback. Fixed-world Paths do
not need the old body pose. Additional bounded anchor restoration, CameraInfo
loading, runtime URDF/mesh import and terrain estimation are separate future work.

## Checks for this change

Workspace tests and warnings-denied Clippy, WASM checks and Web release build, and
ROS 2 Jazzy tests passed. Native and Chrome opened the same recording at the final
cursor, with 360 Scan points and 30 visible Camera overlay points. Browser checks
covered arbitrary asymmetric geometry, invalid JSON rollback, ground TF, missing
ground TF and restoring the original path policy. README images show both clients
with the configured physical envelope, 1 m grid and unscaled source positions.
