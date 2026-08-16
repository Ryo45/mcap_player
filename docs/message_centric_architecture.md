# Message-centric viewer architecture

The recording is the canonical persistent store. Opening a recording does not ingest or decode
the complete log. Exact playback reads only streams required by the open workspace, retains bounded
serialized windows, and routes `RawMessage` values directly to concrete feature controllers.

## Ownership and data flow

```text
SourceCatalog + Workspace requirements
                 -> SessionPlan (fixed stream selection and static routes)

Native indexed source / Web RecordingDataPlane / ROS live push
                 -> RawMessage (MCAP log time + Bytes payload)
                 -> explicit StreamId routes
                 -> CameraController
                    PathController
                    OdometryController
                    TransformController
                    SceneController
                 -> feature state / read-only presentation snapshots
                 -> panels
```

`SourceCatalog` contains source facts only: recording time range, topic, schema, encoding, stable
session-local `StreamId`, and an optional recording-wide message count. It does not assign Camera,
TF, Scene, or UI meaning. `SessionPlan` resolves the current workspace's closed
`PlaybackRequirements` and `WorkspaceBindings` once when a session opens. The workspace owns the
configurable Path/Odometry/PointCloud/dynamic-TF/static-TF topic bindings; viewer-core owns only
their expected ROS schemas. A Camera panel selects its own topic. The configured priority Camera
only changes scheduling among already-selected Camera streams and never expands physical
selection. The playback hot path therefore dispatches by `StreamId`; it does not repeatedly inspect
topic or schema strings.

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
a 16 MiB payload cap. Plot extraction remains its specialized background numeric query, and Preview
remains a source-identified derived artifact; neither is folded into playback or inspection.

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
