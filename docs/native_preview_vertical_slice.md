# Native Preview → Exact vertical slice

## Ownership

```text
App
├─ ViewerSession          Recording/Live source, exact DomainState, and playback capability
├─ PlotLoader             exact/full-resolution speed query
├─ PreviewCoordinator     PreviewArtifact + latest PreviewSnapshot
├─ BookmarkState          durable bookmarks.json document
├─ PresentationState      exact presentation snapshots
├─ NativeWorkspace        layout, Panel runtime, preview_time
└─ Graphics
   ├─ exact Camera textures
   └─ preview Camera textures (latest frame per CameraId)
```

`preview.mcap` and `bookmarks.json` are discovered beside the opened main MCAP. The source summary
fingerprint is computed before either sidecar is accepted. Missing, corrupt, future-version, or stale
sidecars produce diagnostics while exact playback continues. The Viewer never starts the Builder.

Panels receive only the latest snapshot-derived Camera texture handles, SignalOverview, bookmark
slice, and Preview availability. They do not receive the Coordinator, artifact, playback session,
domain state, file handles, or MCAP reader.

## Scrub lifecycle

The Plot contains a small log-wide scrub band. A click issues one exact seek. A drag follows this
state sequence:

```text
pointer down
  → remember whether playback was running
  → temporarily pause when necessary
  → set Workspace preview_time
  → query PreviewCoordinator

pointer move
  → update preview_time and PreviewSnapshot
  → leave Playback cursor and DomainState unchanged

pointer release
  → clear preview_time
  → issue one exact main-MCAP seek
  → clear exact presentation/GPU history through the existing seek path
  → restore the pre-drag playing state
```

The Overview Plot uses the Preview SignalOverview min/max envelope when available. Follow mode,
current speed, and the existing background Plot query remain exact. Thus Push, Query, and Preview
paths remain separate.

## Camera and bookmarks

Preview JPEGs are decoded/uploaded only when `(CameraId, arrival_time, width, height)` changes. Their
texture slots and egui TextureIds are separate from exact Camera resources, so scrubbing cannot
overwrite the current exact image. The Camera Panel displays `PREVIEW` and suppresses exact Path
overlay while the thumbnail is active; after release it returns to `EXACT`.

Point bookmarks appear as markers, while interval bookmarks appear as a translucent span in the
scrub band with boundary markers on the Plot. Clicking a bookmark start emits a normal exact seek.
Editing is intentionally absent. Saves use write/flush/sync to a temporary sibling followed by
rename.

## Allocation and fallback

v0 loads the complete small Preview artifact synchronously. Camera JPEG bytes live in that artifact;
only the selected frames are decoded and the GPU cache is bounded to the latest texture per Camera.
Signal points have a Panel-owned CPU display cache. If Preview is unavailable, the exact camera,
PlotLoader, playback controls, BEV, and Scene continue unchanged.

Deferred work includes async loading/cancellation, automatic generation, multi-resolution signal
LOD, LiDAR/detection Preview, bookmark editing, Web production IndexedReader playback, and compressed
Preview chunks.
