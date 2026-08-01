# Panel architecture prerequisites

This document records the ownership boundaries established before the Native panel
architecture. The next implementation step has now replaced the fixed central
composer with a bundled, serialized layout and concrete Native panel runtimes; see
[`native_panel_layout.md`](native_panel_layout.md).

## Data flow

Playback-derived, continuously updated data uses the push path:

```text
McapPlayback / ROS live
  → DomainUpdate
  → DomainState
  → PresentationState
  → Camera / BEV / Scene views
```

Plot data uses a separate query path:

```text
PlotLoader worker
  → LoadedSignal
  → Plot view
```

The push and query paths remain separate. Plot extraction does not run through
playback ticks, and plot loading failures do not stop playback.

## UI interaction flow

```text
PlaybackView + Presentation snapshots + WorkspaceState
  → fixed Camera / BEV / Plot / Scene view functions
  → ViewerAction
  → App::apply_actions
  → WorkspaceState / PlaybackSession
```

`PlaybackView` is read-only. Playback operations use `PlaybackCommand` as their
single command type. Preview time belongs to `ViewerInteractionState` and does not
seek or modify the playback clock.

Scene orbit, zoom, reset, and camera mode remain concrete `SceneViewOutput` values.
Graphics applies these requests to the existing single Scene renderer before the
egui frame is painted.

## State ownership

```text
App
├─ PlaybackSession
│  ├─ MCAP or ROS live source
│  ├─ playback clock
│  └─ current DomainState
├─ PlotLoader
│  └─ generation-scoped background speed query
├─ PresentationState
│  └─ CPU-side presentation builders, metrics, and overlay status
└─ WorkspaceState
   ├─ LayoutDocument
   ├─ PanelRuntimeStore
   │  ├─ CameraPanel → focused camera
   │  ├─ PlotPanel → Overview / Follow and plot cache
   │  └─ ScenePanel → scan accumulation
   └─ ViewerInteractionState → optional preview time
```

`Graphics` owns backend and GPU resources only:

```text
Graphics
├─ wgpu surface / device / queue
├─ egui context / state / renderer
├─ Camera textures shared by CameraId
├─ one BEV renderer
└─ one Scene renderer
```

## Fixed view boundaries

The current fixed-layout composer calls four concrete view functions:

```text
show_camera_view(input)         → CameraViewOutput { actions }
show_bev_view(input)            → BevViewOutput { logical_size }
show_plot_view(input, state)    → PlotViewOutput { actions }
show_scene_view(input)          → SceneViewOutput { actions, size, camera input }
```

These inputs and outputs are the prototypes for future panel interfaces. They do
not receive `PlaybackSession`, `DomainState`, MCAP readers, worker channels, or the
whole `Graphics` object.

## Assumptions retained by the first panel implementation

- Camera panels may have multiple instances.
- Camera GPU textures remain shared by `CameraId`.
- The first panel implementation allows at most one BEV panel.
- The first panel implementation allows at most one Scene panel.
- Plot and other view states now live in their corresponding panel runtimes.
- The fixed view input/output types should guide the initial panel interface.
- Push-domain data and background query data must not be merged into one manager.
- Split editing, tabs, user persistence, and generic query management remain
  deliberately deferred.
