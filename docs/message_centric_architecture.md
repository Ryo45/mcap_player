# Message-centric viewer architecture

The recording is the canonical persistent store. Opening a recording does not ingest or decode
the complete log. Exact playback reads only streams required by the open workspace, retains bounded
serialized windows, and routes `RawMessage` values directly to concrete feature controllers.

## Ownership and data flow

```text
SourceCatalog + Workspace requirements
                 -> SessionPlan (fixed stream selection and static routes)

Native indexed source / Web RecordingDataPlane / ROS live push
                 -> RawMessage (log time, publish time, sequence, Bytes payload)
                 -> explicit StreamId routes
                 -> FeatureRuntime
                    serialized admission/coalescing
                    concrete Camera/Path/Odometry/Transform/Scene materialization
                 -> feature state / read-only presentation snapshots
                 -> panels
```

`SourceCatalog` contains source facts only: recording time range, topic, schema, encoding, stable
session-local `StreamId`, original schema encoding/definition bytes, and an optional recording-wide
message count. Native and Browser Local share one `Arc<SchemaDefinition>` per MCAP schema across
stream descriptors. Remote catalog JSON preserves the same bytes as `schemaData`; unsupported
message/schema encodings remain visible in the catalog. It does not assign Camera,
TF, Scene, or UI meaning. `SessionPlan` resolves the current workspace's closed
`PlaybackRequirements` and `WorkspaceBindings` once when a session opens. The workspace owns the
configurable Path/Odometry/PointCloud/dynamic-TF/static-TF topic bindings; viewer-core owns only
their expected ROS schemas. A Camera panel selects its own topic. The configured priority Camera
only changes scheduling among already-selected Camera streams and never expands physical
selection. The playback hot path therefore dispatches by `StreamId`; it does not repeatedly inspect
topic or schema strings.

`SessionPlan` deliberately retains its name: it is already an immutable **continuous playback**
plan. It contains no Plot or Inspector demand. Native builtin Plot panels contribute only signal
queries; their Odometry query topic comes from workspace bindings independently of playback
selection. A Status panel can still request continuous Odometry for its current telemetry.

`FeatureRuntime` owns the continuous materialization boundary, not just controller aggregation.
Controllers implement its concrete feature-specific admission, decode and history policies. Panels,
Camera overlays, Scene and diagnostics borrow shared materialized state. They do not decode their
own Camera/TF/PointCloud copies. Camera compressed-image materialization lives here; expensive
JPEG pixel decode is shared by the platform `CameraBaseImageTracker`, with GPU resources owned by
renderers. No arbitrary topic history enters this boundary.

Controllers own feature-specific semantics. Camera admission and focused/background scheduling
happen before JPEG decode. Dynamic TF retains ordered bounded history; static TF keeps a small exact
persistent message archive. Scene owns bounded point accumulation. A `ViewerPresentation` is only a
read-only UI snapshot built from controller state each frame; it is not persistent truth, a temporal
database, or a second global world model.

There is no `DomainState`, `DomainUpdate`, generic Projection, event bus, decoded-message registry,
or compatibility facade. Adding a visualization normally adds requirements, a concrete controller
or pure decoder, and a view without extending a universal semantic enum.

## Sequential playback

Playback and random inspection are separate workloads. Native uses indexed reads over its mmap.
Web Local and Remote share `RecordingDataPlane` while retaining distinct loaders:

```text
Browser File.slice -> BrowserMcapWindowLoader --+
                                                  -> SerializedWindow
Recording Server  -> RemoteWindowLoader ---------+       -> RecordingDataPlane
                                                          -> RawMessage routes
```

`RecordingDataPlane` owns fixed-selection coverage, one-window-at-a-time prefetch, buffering,
generation cancellation, and a 256 MiB resident-backing budget. It commits no semantic state.
Local and Remote payloads remain `Bytes` slices of chunk/page backing allocations. The playback
clock advances only after the required read and synchronous controller processing succeed; while
buffering or seeking, the previously committed controller state remains visible.

ROS Live is intentionally a push path rather than an artificial recording source. Its camera
mailbox feeds the same `RawMessage -> CameraController` boundary. Camera coalescing is not promoted
to a universal drop policy because ordered streams such as TF have different semantics.

## Seek restoration

Callers request only `seek(T)`. Each controller declares a concrete restore meaning:

- Camera, Path, Odometry, and Scene use the exact latest message whose MCAP log time is `<= T`.
- Dynamic TF uses `History(1 second)`.
- Static TF uses explicit `Persistent` semantics.

Latest-before restoration is a small physical primitive, not a generic query API. Native builds a
sparse `StreamId -> ChunkIndex` list at open, reads Message Index records lazily for the requested
streams, chooses predecessor entries without decompressing data, groups candidates by Chunk, and
streams each selected Chunk once. It does not expand all Message Index entries into RAM. Browser
Local performs the same candidate/group/read operation with `File.slice`; Remote calls the
Recording Server restore endpoint. An indexed recording with no predecessor leaves that feature
unavailable. A non-empty stream without Message Index records reports indexed restore as
unavailable rather than scanning the recording prefix or constructing a side index.

The one-second dynamic-TF policy controls both normal runtime retention and seek history. Static TF
is different: each recording session bootstraps its complete, expected-small persistent message
archive once and replays updates with log time `<= T`, including repeated publications after node
restart. Native reads the archive from indexed chunks; Browser Local and Remote retain it in the Web
session after the first cold seek.

Every platform gathers latest/history/persistent restore messages before changing visible state.
It then repositions/rebases forward playback, synchronously resets and replays controllers, and
finally commits the cursor. Any read, decompression, index, or generation failure leaves the old
cursor and controller values visible. Restore describes practical feature state at `T`; it does not
promise reconstruction of unrelated messages that no selected controller consumes.

## Bounded inspection

Occasional inspection uses `RangeQuery` with an exact `[start,end)` range plus hard message-count
and payload-byte limits. It returns original serialized messages, clones only `Bytes` handles, and
does not mutate the playback source cursor or controller state. Native Inspector uses this path with
a 16 MiB payload cap. Plot extraction remains a separate numeric query, and Preview remains a
source-identified derived artifact; neither is folded into playback or inspection.

## Metadata exploration and selected-field Plot

`TopicTree::from_catalog` reads only source metadata. It retains every stream and its original
schema definition, exposes numeric scalar paths from supported `ros2msg` definitions, and reports
missing/unsupported schemas explicitly. Native shows topic/schema definitions in the source sidebar;
clicking a scalar adds a session-owned field Plot demand. Removing a series cancels its worker
request. Neither action mutates the continuous plan or cursor. Opaque and non-CDR topics remain
visible, even though numeric extraction currently requires CDR.

```text
SourceCatalog -> TopicTree (metadata only)
                         -> stream + field path demand
                            -> compile NumericFieldExtractor once
                            -> indexed selected-stream/range serialized reads
                            -> skip preceding fields, extract selected scalar
                            -> bounded SignalOverviewReducer -> NumericPlotOverview -> UI
```

The extractor supports plain little/big endian ROS 2 XCDR1, `ros2msg` dependency definitions, nested
messages, primitive numeric scalars, and skipping strings, fixed arrays and sequences before a
selected scalar. It stops at that scalar: it neither copies strings/arrays nor creates a universal
ROS value tree. Schema depth, definition size, field expansion and variable-message traversal have
explicit bounds. Arrays/sequence elements as selected paths, wstring, ros2idl, parameter-list CDR
and XCDR2 are not implemented. Plot numbers are `f64`, so large 64-bit integers can lose display
precision; source payloads and timestamps remain exact.

Three concrete access patterns are separate:

- `query_numeric_overview`: streaming min/max reduction, bounded by requested point count plus
  source metadata and one chunk. Index facts skip unrelated chunks; no exact-history vector exists.
- `query_numeric_detail`: visible `[start,end)` range with existing message/byte caps and an explicit
  `complete` flag.
- `query_numeric_current`: exact latest serialized source message `<= cursor`, found through Message
  Index records. It does not approximate current values from overview buckets.

Native currently renders full-recording overview field plots, with at most eight series of 4,000
points each and one cancellable worker per session. Detail/current APIs exist in core but are not
yet scheduled by this UI. Web Local/Remote preserve identical metadata and raw-message semantics;
interactive field Plot UI and query transport adapters remain future work.

`NumericPlotOverview` is a bounded query product and the boundary for a future source-fingerprinted
artifact keyed by stream, schema, field, time range and extraction/reduction version. The current
implementation scans selected indexed chunks on first request; removing/re-adding a series scans
again. It does not claim multi-TB overview artifact reuse yet. Existing speed/yaw-rate remain
explicit builtin derived signals, sharing their current Odometry scan and bounded reduction.

## Time facts

`RawMessage.arrival_time` means MCAP **log time**, which defines playback/query ordering. Original
MCAP publish time and sequence are retained independently, including zero values; Live can omit
these MCAP-only facts. ROS header/measurement timestamps remain feature-specific payload facts.
Numeric queries do not infer a measurement time from arbitrary timestamp-like fields. Exact numeric
detail/current preserve publish time and sequence, while overview is deliberately a discardable
display reduction.

## Performance invariants

- Browser Local never reads the whole MCAP into WASM memory.
- Web loaders publish only complete logical windows; continuation pages never leak upward.
- Playback-required windows are protected by the DataPlane retention policy.
- Routing clones `Bytes` handles, never payload buffers.
- Camera CDR parsing retains JPEG as a slice of the same payload backing.
- Camera and point-cloud decode occur only after a route accepts the message.
- Physical chunk/page caches are bounded accelerators, never semantic truth.
- Multi-stream latest-before decompresses a shared candidate Chunk at most once per restore.
- Exact log time is the playback/query timeline; equal-time ordering is deterministic by stream ID
  while stable source order within a stream is retained.

Preview MCAP, signal envelopes, and future feature-specific indexes are discardable derived
artifacts tied to a source fingerprint. They do not replace exact MCAP messages.
