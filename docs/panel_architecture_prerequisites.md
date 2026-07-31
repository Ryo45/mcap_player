# Panel architecture prerequisites

This document records the ownership boundaries that precede a future Native panel
architecture. The current fixed layout remains intentional; there is no panel
registry or serialized layout yet.

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
   ├─ CameraViewState
   │  └─ focused camera
   ├─ PlotViewState
   │  ├─ Overview / Follow viewport
   │  └─ cached egui plot points
   ├─ SceneViewState
   │  └─ scan accumulation setting
   └─ ViewerInteractionState
      └─ optional preview time
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

## Assumptions for panel introduction

- Camera panels may have multiple instances.
- Camera GPU textures remain shared by `CameraId`.
- The first panel implementation allows at most one BEV panel.
- The first panel implementation allows at most one Scene panel.
- Plot state can move from `WorkspaceState::plot` into a Plot panel runtime.
- Other view states can likewise move into their corresponding panel runtimes.
- The fixed view input/output types should guide the initial panel interface.
- Push-domain data and background query data must not be merged into one manager.
- `LayoutDocument`, `PanelRegistry`, split/tabs, persistence, and generic query
  management are deliberately deferred.
