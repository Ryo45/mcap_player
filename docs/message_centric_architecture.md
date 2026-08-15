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
`PlaybackRequirements` once when a session opens. The playback hot path therefore dispatches by
`StreamId`; it does not repeatedly inspect topic or schema strings.

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

Callers request only `seek(T)`. Each controller declares a concrete restore meaning and the planner
combines it with catalog facts:

- Camera, Path, Odometry, and Scene use `RecentSample`.
- Dynamic TF uses `History(1 second)`.
- Static TF uses explicit `Persistent` semantics.

For a recent sample, the planner estimates a period from recording duration divided by recorded
message count and reads four periods, clamped to 250 ms–20 s. Missing statistics use a centralized
10 s fallback. A bounded range with no matching sample leaves that feature unavailable; it does not
trigger recursive or unbounded `latest_before` reads.

Native restore reads all planned ranges into temporary messages, repositions the forward source,
then resets/replays controllers and commits the cursor. Any physical read failure leaves both the
visible controller state and cursor unchanged. Repeated `/tf_static` messages are retained and
replayed only when valid at the target. Web uses the same planner for bounded non-persistent restore,
cancels stale loader generations, and publishes no partial window. Its concrete static-TF archive is
replayed on restore; a source-level sparse persistent bootstrap for a direct Web seek before static
messages have ever been observed remains a focused follow-up rather than a generic temporal store.

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
- Exact log time is the playback/query timeline; equal-time ordering is deterministic by stream ID
  while stable source order within a stream is retained.

Preview MCAP, signal envelopes, and future feature-specific indexes are discardable derived
artifacts tied to a source fingerprint. They do not replace exact MCAP messages.
