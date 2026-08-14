# Native showcase layout

The Native Viewer keeps `config/layouts/native_default.json` as its default workspace. A separate
showcase can be selected at startup:

```bash
cargo run -p viewer-native -- \
  --layout showcase \
  --mcap mcap/turtlebot3_7cam_fhd/turtlebot3_7cam_fhd_0.mcap \
  --camera-topic /camera/front/image/compressed
```

Use `--layout standard` (or omit `--layout`) for the existing Camera / BEV / Plot / Scene screen.

## Composition

The showcase is expressed entirely by `config/layouts/native_showcase.json`; there is no composite
Showcase panel or view. Its `LayoutDocument` contains:

```text
Column 0.70 / 0.30
├─ Row 0.22 / 0.56 / 0.22
│  ├─ CameraPanel: /camera/front_left/image/compressed
│  ├─ CameraPanel: /camera/front/image/compressed + Path overlay
│  └─ CameraPanel: /camera/front_right/image/compressed
└─ Row 0.38 / 0.38 / 0.24
   ├─ PlotPanel: vehicle speed
   ├─ PlotPanel: yaw rate
   └─ StatusPanel
```

Camera topics are exact config values, not semantic `left`/`right` roles and not topic-name
heuristics in `viewer-core`. A Camera panel without `cameraTopic` retains the existing interactive
focus-and-thumbnails behavior. `showOverlay` controls overlay composition without checking a
Showcase-specific panel ID.

The front panel sets `schedulerPriority: true`. At source open, Native resolves that panel's topic
to the session-local `CameraId` and gives it the existing focused-camera 10 Hz scheduling policy.
This scheduler choice is separate from the camera selected for display by each fixed Camera panel.

## Data ownership

- Camera images and Path overlay use the shared Domain presentation path. Graphics continues to
  own exact/preview textures and the overlay snapshot cache.
- Speed and yaw-rate histories are independent ViewerSession-owned `/odom` queries. They are not
  added to `DomainState` as time-series history. One background scan derives both signals and
  publishes bounded intermediate series, so a large compressed recording does not keep the panels
  behind a loading placeholder until the complete query finishes.
- `StatusPanel` is a normal `NativePanel` and reads only the narrow per-frame panel context.
