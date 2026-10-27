# Native / Web parity

This inventory compares the current Native application with Browser Local and
Recording Server playback. Platform-specific I/O remains separate; MCAP log time,
exact predecessor selection, transactional seek, bounded state, and CDR decoding
remain shared observable requirements.

## Current inventory

| Area | Native | Web (Browser Local / Remote) | Remaining work |
| --- | --- | --- | --- |
| Recording playback | Local indexed MCAP | Browser `File.slice()` and Recording Server | Native remote source UI, if needed |
| Playback and seek | Pause, speed, exact restore | Same shared clock/controllers; async complete-window buffering and restore | Keep parity regressions as features expand |
| Topic catalog | Topic tree, schema, counts | Filterable topic list; all channels, schemas, encodings and counts | Hierarchical tree and numeric-field selection |
| Message Inspector | Cursor-driven background latest-before queries; lazy fields and page navigation | Automatic cursor follow (250 ms), coalesced exact latest-before across topic channels, manual Refresh and auto on/off; lazy CDR containers and next-page navigation | Previous/first page navigation, schema definition view |
| Monitor layout | Left Topics / Inspector, right Scene and camera | Same arrangement; smaller screens stack the panels | Resizable splits and configurable layouts |
| Cameras | Schema-based image-topic dropdown per panel, shared textures, scheduler focus and trajectory overlays | Schema-based image-topic dropdown, thumbnails, scheduler focus and same overlays | Multiple independently focused camera panels |
| BEV | Shared GPU renderer | Shared GPU renderer, selectable in the Scene region | Independent BEV panel when configurable layouts arrive |
| 3D Scene | Shared renderer, chase/free/vehicle-eye cameras, Scan/TF, accumulation | Same renderer and bounded presentation state; camera modes, drag orbit, wheel zoom, reset, accumulation | Touch orbit controls and richer TF diagnostics |
| Camera-less recordings | Query-only and optional-data layouts | Open catalogs with optional available continuous streams; empty selections read no message chunks | Dedicated query-only presets |
| Numeric / builtin Plot | Numeric field queries, speed/yaw-rate overview/detail/current | Not implemented | Async bounded Local/Remote query I/O plus shared extraction/reduction |
| Topic Health / Diagnostics | Bounded query snapshots and session worker | Not implemented | Async index/range readers plus shared reductions; no full-recording scan fallback |
| Preview / bookmarks | Preview sidecars and timeline bookmarks | Not implemented | Browser/Remote artifact loading and lifecycle |
| Layout persistence | Validated layout documents and presets | Fixed monitor workspace | Browser layout host using the shared persistent model |
| ROS live | Configured ROS 2 subscriptions; camera focus within subscribed inputs | Not implemented | Separate transport requirement; browser cannot directly reuse Native ROS subscriptions |
| MCAP compression | Uncompressed, Zstd, LZ4 | Browser Local: uncompressed, Zstd, LZ4; Recording Server: uncompressed, Zstd | Server LZ4 support |

## First delivered slice: monitor workspace

The Web UI presents topics and messages on the left, with 3D Scene above the camera
on the right. BEV remains available through the view selector. Topic selection does
not add playback routes or retain recording-duration payload histories. Camera-less
catalogs are valid, and a missing preferred camera falls back to an available camera.
An empty continuous selection produces complete empty windows without a Local chunk
read or Remote HTTP request.

Inspector uses an independent source loader, reusing the indexed latest-before
primitive used for restore. Automatic follow is enabled by default, starts at most
once per 250 ms, and keeps one active request plus one coalesced cursor demand.
Forward ticks do not cancel active work. A completed snapshot remains visible with
its query cursor while refreshing; paused cursors reuse the result. Manual Refresh
and automatic on/off remain available. New messages reset container/page navigation.
One compiled stream schema is reused; message identity is not inferred from timestamps.
A topic switch,
successful seek, or source replacement invalidates it; Remote cancellation aborts
HTTP, and Local cancellation stops further reads after the current `File.slice()`
completes. A failed seek preserves the previous committed inspection. The query
cursor and MCAP metadata remain visible, so a paused snapshot is distinguishable
from the current playback cursor. Equal log times use stream ID, then the source's
chunk and record offset order.

Local indexed queries now compare predecessors across overlapping/non-monotonic
chunk ranges instead of returning the first match. Message Index entries need not
be sorted. The same correction applies to Browser seek restore.

Native and Browser Local share the pure Chunk decompression/record traversal and
Message Index predecessor selection in `viewer-mcap`. Native uses its already
sorted index spans for logarithmic lookup; Browser consumes unsorted entries in
one pass. Chunk/end-time pruning and equal-time chunk/offset ties have one shared
implementation. Source adapters retain physical reads, CRC checks where supported,
compression acceptance, caches, cancellation and query admission budgets. Chunk
traversal yields payload slices of the input backing and reports structured parse/
decompression errors without allocating a shared full-chunk message collection.

Local restore/Inspector work reserves at most 4,096 physical index reads and 32 MiB
of index bytes (including record headers), 256 MiB of compressed plus uncompressed
chunk input, and 200,000 selected messages. These are whole-query limits and can
reject recordings whose restore exceeds them. Filtering exact records happens
before message admission, avoiding a vector of every selected-channel message in
a candidate chunk. Physical chunk input and retained `Bytes` backing are bounded
separately from Inspector's existing 16 MiB logical payload limit. Topics with more
than 64 channels return an inspection error. Rendering allows at most 512 field
visits and 32 containers per message; the shared decoder pages 64 fields at a time.
Unsupported/malformed schemas or payloads show an error while retaining MCAP metadata.

Scene uses `ScenePresentationState` and `SceneRenderer`, including acquisition-time
TF conversion and the 65,536-point accumulation bound. GPU frame caches are invalidated
on source replacement and successful seek, because revisions can repeat across
reset feature states. A WebGPU failure is displayed and leaves catalog, Inspector
and camera functionality available.

## Suggested follow-up order

1. Bring Topic Health and Diagnostics to both Web sources. Share the domain
   reduction and budget rules; introduce async physical query primitives where
   the current synchronous `McapSource` boundary prevents reuse. Do not fetch all
   recording payloads to compute timestamp facts.
2. Add numeric-field Plot through the Topic browser, preserving overview/detail/
   current semantics and independent playback selection.
3. Add resizable/configurable layouts, then multiple camera panels and bookmark/
   preview artifacts. Use `viewer-layout` for the persistent model, with a concrete
   DOM host; do not import the Native `egui` panel implementation into Web.

Each step should cover Browser Local and Remote together and add regressions for
stale requests, exact nanoseconds, equal-time ties, failed seek rollback, and work/
retained-memory limits where affected.

Camera selection behavior and its bounded/transactional read path are described in
[camera_topic_selection.md](camera_topic_selection.md).
