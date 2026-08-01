# Native panel layout

The Native viewer's central area is restored from the versioned JSON embedded from
`config/layouts/native_default.json`. Playback controls, live status, source status,
file opening, and application errors remain outside the layout document.

## Ownership

```text
App
├─ PlaybackSession
├─ PlotLoader
├─ PresentationState
├─ NativeWorkspace
│  ├─ LayoutDocument                 persistent model
│  ├─ PanelRuntimeStore
│  │  ├─ CameraPanel                 CameraViewState
│  │  ├─ BevPanel
│  │  ├─ PlotPanel                   PlotViewState + plot cache
│  │  ├─ ScenePanel                  SceneViewState
│  │  └─ PlaceholderPanel            original config + error
│  ├─ ViewerInteractionState         shared preview time
│  └─ scheduler focused camera       last camera-panel interaction
└─ Graphics
   ├─ Camera textures shared by CameraId
   ├─ one BEV renderer
   └─ one Scene renderer
```

`viewer-layout` owns only the persistent model and structural validation. It depends
on `serde` and `serde_json`, and has no dependency on UI, rendering, playback, or
viewer domain crates. Native capability checks and typed config parsing happen
while building `PanelRuntimeStore`.

Panel-local camera focus is intentionally separate from the one decode-scheduler
focus. Each Camera panel remembers what it displays. The last camera interaction is
also forwarded to `PlaybackSession` because the current decoder has one global
priority camera. The bundled layout contains one Camera panel, so this preserves
the previous behavior.

## Persistent JSON

The document has schema version 1 and contains split and panel nodes:

```json
{
  "schemaVersion": 1,
  "root": {
    "kind": "split",
    "direction": "row",
    "children": [
      {
        "weight": 1,
        "node": {
          "kind": "panel",
          "id": "camera-main",
          "panelType": "camera",
          "configVersion": 1,
          "title": "Camera",
          "config": {
            "fit": "contain",
            "showThumbnails": true
          }
        }
      },
      {
        "weight": 1,
        "node": {
          "kind": "panel",
          "id": "bev-main",
          "panelType": "bev",
          "configVersion": 1,
          "config": {}
        }
      }
    ]
  }
}
```

Weights are relative and are normalized during rendering. Pixel positions and
runtime state are never serialized. Unknown fields are accepted, while unknown
node kinds remain deserialization errors.

The bundled layout is:

```text
Column
├─ 0.36 Row
│  ├─ 0.5 Camera  (camera-main)
│  └─ 0.5 BEV     (bev-main)
├─ 0.22 Plot      (speed-main)
└─ 0.42 Scene     (scene-main)
```

## Native panels and placeholders

The concrete enum variants are Camera, BEV, Plot, Scene, and Placeholder. Camera
and Plot may have multiple runtime instances. The current GPU implementation
supports at most one BEV and one Scene; later valid instances become placeholders.

A placeholder is also created when:

- `panelType` is unknown;
- the panel config cannot be decoded;
- `configVersion` is unsupported; or
- a BEV or Scene exceeds the Native singleton capability.

The placeholder retains panel ID, title, requested type, config version, original
JSON config, and the exact error. A missing runtime discovered by the layout host is
rendered as an inline error instead of panicking.

If the bundled document itself cannot be parsed or validated, `NativeWorkspace`
constructs a programmatic one-panel emergency layout and exposes the load error in
the application diagnostics.

## Rendering path

```text
LayoutDocument
  → NativeLayoutHost recursive traversal
  → normalize weights and calculate child rects
  → PanelId lookup in PanelRuntimeStore
  → NativePanel::show
  → existing Camera / BEV / Plot / Scene view functions
  → ViewerAction + concrete render requests
```

The host does not load MCAP data, mutate playback, parse panel configs, create GPU
resources, or start queries. Camera textures remain shared by `CameraId`; BEV and
Scene sizes and camera input continue through their existing renderers.

## Deliberate limitations

There is no split dragging, tabs, layout editing, file import/export, user
persistence, generic panel registry, plugin loading, query manager, or hidden-panel
optimization. Layout JSON is bundled at compile time and is not read relative to
the process working directory.
