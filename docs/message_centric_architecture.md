# Message-centric viewer architecture

The recording is the canonical persistent store. Opening a recording does not ingest or decode
the complete log. Exact playback reads only streams required by the open workspace, retains bounded
serialized windows, and routes `RawMessage` values directly to concrete feature controllers.

Native forward traversal prunes chunks using selected channel facts before reading payloads.
Chunk start times define traversal order; overlapping chunks are merged by log time, stream ID,
physical chunk index, then record order. Chunk end times need not be monotonic. Seek uses a
prefix maximum of end times to locate possible overlaps, stages reads before committing, and
skips messages strictly before the requested forward position. Active serialized Chunk backings
are allocated only as required by the selected streams' chronological merge, with a 4 GiB
emergency ceiling on 64-bit platforms. There is no fixed 256 MiB admission gate or preallocation;
large Chunks and time-overlapping parts can use more backing when needed. Individual compressed
and expanded Chunk payloads are capped at 2 GiB; metadata and other source records keep their 256 MiB bound. An
excessive overlap still produces an explicit error instead of unbounded retention. Consumed
Chunks are released. Transaction staging and the caller's returned batch
can temporarily retain additional backings; the allowance is not a total RSS limit. Playback
latest-before restore allows up to 8 GiB of cumulative compressed-plus-expanded Chunk work on
64-bit platforms, separately from query defaults and the resident forward backing allowance.
These are ceilings, not promises to fit a memory-constrained host; large Chunks increase seek
latency and transient memory as well as retained backing.

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
session-local `StreamId`, original schema encoding/definition bytes, an optional recording-wide
message count, and supported source operations. Native and Browser Local share one `Arc<SchemaDefinition>` per MCAP schema across
stream descriptors. Remote catalog JSON stores each schema once in `schemas` (ID, encoding, bytes), and streams
reference it through `schemaId`. Catalog clones share byte backing; the Web adapter shares one
`Arc<SchemaDefinition>` per referenced schema without copying its bytes. Unsupported
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

The Native `monitor` workspace places an embedded `topics` panel in the left 30%, and a Scene
above one front Camera in the right 70% (equal vertical weights). The concrete Topics panel owns
only its filter and bounded Inspector expansion pages. It borrows metadata, the Session-selected
topic and Inspector query results; clicking a row emits the existing `SelectInspectorTopic` action.
Its presence suppresses the auxiliary source sidebar and floating Inspector. Schema/numeric Plot
controls share the regular Topic Tree helpers. Selection remains dynamic query demand and never
changes `SessionPlan`.

Scene's `showTelemetry` config enables a compact current-value banner from borrowed playback,
telemetry, camera presentation and source diagnostics. It does not contribute signal overview
demand. `optionalData` makes Scene point-cloud/TF inputs optional at plan resolution; all available
bound inputs still use the same continuous materialization and GPU ownership as other workspaces.
Missing inputs remain visible through diagnostics. The monitor Camera is fixed to one front topic
with planned-path overlay enabled. Live monitoring uses the same feature views, while Generic
Inspector remains unavailable without a recording cursor. This preset introduces no arbitrary
decoded-message history or additional renderer instance.

`FeatureRuntime` owns the continuous materialization boundary, not just controller aggregation.
Controllers implement its concrete feature-specific admission, decode and history policies. Panels,
Camera overlays, Scene and diagnostics borrow shared materialized state. They do not decode their
own Camera/TF/PointCloud copies. Camera CDR metadata/payload materialization lives here; expensive
image pixel decode/Raw conversion belongs to presentation, with GPU resources owned by renderers.
Native uses `CameraDecodeQueue`: two shared image workers (TurboJPEG for JPEG), one coalesced pending image per camera,
a two-result completion channel, and at most 32 camera identities. Image inputs are compact owned
copies under a shared 64 MiB limit for pending/active input; completed results and displayed
state retain only CameraFrameMetadata plus pixels, releasing encoded copies at decode completion. Workers do not
pin MCAP Chunk backing. Successful seek/source transitions invalidate old generations. Failed
restore leaves current presentation intact. The UI uploads completed images without waiting for
pixel decode and uses the displayed image's measurement time/frame ID for its overlay.
Physical display pixels select 1/1, 1/2, 1/4, or 1/8 IDCT output; projection remains in original
source pixel coordinates, including aspect ratio/cropping for odd dimensions. Resize/focus changes
can re-decode the same message. Browser uses two bounded camera Web Workers with resized
`ImageBitmap` output and explicit close; source dimensions and displayed frame time remain the
projection inputs. Browser queue generations survive view reset so old work continues counting
toward the concurrency limit. See [Browser JPEG presentation](web_jpeg.md). No arbitrary topic
history enters this boundary.

Route resolution does not infer whether a source is a recording from the presence of a time range.
Native, Browser Local, and Remote recording adapters explicitly validate Catalog,
ForwardPlayback, ExactSeek, HistoryRestore, and PersistentRestore capabilities at open time against
the selected plan. Push/live adapters use the same route resolution without pretending to support
recording restore operations.

Controllers own feature-specific semantics. Camera admission and focused/background scheduling
happen before JPEG decode. Camera deadlines follow fixed 100 ms / 200 ms periods, preserving
the existing camera phases rather than adding UI tick lateness to every deadline. A late tick
skips missed periods and materializes only the latest pending message per camera; it does not
replay a catch-up backlog. These are nominal periodic rates, not strict minimum spacing between
two consecutive admissions. Focus changes and restore explicitly reanchor the schedule.
Dynamic TF retains ordered bounded history; static TF keeps a small exact
persistent message archive. Scene owns bounded point accumulation. A `ViewerPresentation` is only a
read-only UI snapshot built from controller state each frame; it is not persistent truth, a temporal
database, or a second global world model.

There is no `DomainState`, `DomainUpdate`, generic Projection, event bus, decoded-message registry,
or compatibility facade. Adding a visualization normally adds requirements, a concrete controller
or pure decoder, and a view without extending a universal semantic enum.

## Sequential playback

Playback and random inspection are separate workloads. Native uses bounded positional reads over its open file handle.
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
to a universal drop policy because ordered streams such as TF have different semantics. The
concrete ROS handle owns executor-thread stop/join and the latest successfully admitted source
arrival used for freshness diagnostics; controller presentation state is not an I/O liveness
authority.

## MCAP crate boundary

`viewer-mcap` owns MCAP I/O, byte processing, indexed queries and native recording
transport. It depends on `viewer-core` for source-neutral contracts, codecs,
reducers and feature state. Core has no dependency on the MCAP adapter or codecs.
See [viewer_mcap_boundary.md](viewer_mcap_boundary.md) for the extraction scope and
dependency checks.

## Safe Native file access

`McapSource::open_file` reads Summary and Message Index records by range, and owns only metadata
plus bounded chunk buffers. It never maps the mutable recording into a Rust slice. Immutable
in-memory input uses `from_bytes`; `copy_from_slice` explicitly advertises its full-input copy.
`read_through(T)` is inclusive, and `time_range()` returns `RecordingTimeRange { start,
end_exclusive }` with an explicit `end_inclusive()` conversion for the playback clock.

Native playback, numeric/builtin Plot and Inspector readers share the same open file handle and
metadata through `fork_reader`; they do not reopen the pathname for each query. Path replacement
therefore cannot silently redirect a running query to another recording. Cached payloads are
`Bytes` slices of heap-owned read/decompression buffers and remain valid after file truncation or
modification. `viewer-mcap` forbids unsafe code; file-backed mmap and its dependency have been
removed from application/tools input paths.

Reads check source length/mtime before and after copying. Observable changes return `SourceChanged`
or an I/O error; these checks are **best-effort consistency checks, not proof of file immutability**.
Concurrent writes cannot cause invalid Rust references even when a same-size change is not detected
by metadata. The Native reader bounds each physical record and uncompressed chunk to 256 MiB.
Summary/index metadata scales with source metadata; full-resolution messages never remain resident
for the recording duration. Message Index records for one candidate chunk share one bounded range
read. Offline tools use a bounded sequential `FileMessageStream` that also supports unchunked data.

Forward reads stage position/chunk changes and publish them only after all required reads succeed;
a failed multi-chunk read does not consume messages behind the old visible cursor. Empty-selection
playback/seek reads no message chunks. The seek transaction below retains the same guarantees.

## Structured failures

Planning/window errors have domain variants such as `MissingRequiredStream`, `UnsupportedRestore`,
`NonContiguousWindow`, `MessageOutsideWindow` and `InvalidRange`. Source, range, numeric query, Web
transport/playback and Native query-worker errors retain their underlying Rust errors through
`Error::source()`. Schema exploration errors likewise remain typed. Browser exceptions retain their
original JS values at the FFI boundary. Views and UI diagnostics format errors for display; internal
layer conversions do not replace I/O, MCAP, JSON or decoder errors with their formatted messages.
Contextual invalid-record descriptions remain leaf details under explicit error categories.

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

## Generic cursor Inspector

Native's Topic Tree offers **Inspect at cursor** for every topic, including schemas/encodings that
cannot yet be decoded. Selection creates session-owned query demand; it does not add continuous
streams, change the playback cursor or mutate FeatureRuntime. Fixed Inspector panels use the same
path and now configure only `topic` (the earlier `maxMessages` range-preview setting is removed).

At committed playback cursor `T`, the worker queries the exact latest MCAP message with log time
`<= T`. For topics containing multiple source channels it chooses the latest across those channels,
using deterministic stream/source ordering for ties and decoding with the chosen channel's schema.
The UI displays log time, original publish time/sequence (including zero), typed ROS header stamp
and the field tree. Header stamp is never substituted for the log-time query timeline. No predecessor
is an explicit empty result; missing index/schema/unsupported encoding is an explicit error with
original message metadata retained when available.

```text
Topic Tree / Inspector panel -> topic + committed cursor demand
  -> one coalescing Inspector worker -> LatestMessageReader
  -> sparse chunk lookup + Message Index predecessor
  -> original RawMessage -> InspectorSchema / InspectorMessage (query-only view model)
  -> bounded field pages -> Inspector UI
```

The existing latest-before primitive and `LatestMessageReader` share indexed position resolution.
Per-stream chunk references, sorted by start time with prefix maximum end times, are built once at
source open. Cursor queries binary-search those sparse references; they do not scan the recording
or all chunk facts on every frame. Overlapping chunk time ranges choose the actual latest entry.
The cursor reader caches **one Message Index span and one serialized/decompressed chunk**, never a
recording-wide message history or decoded ROS store. Adjacent cursor changes within that chunk do
not repeat index reads/decompression. Record identity reuses an Inspector view while its message
remains the same. Source changes keep one worker, reset its physical/view caches and reject stale
results. The bounded mailbox coalesces pending cursor requests. A moving cursor shows the last
completed query's target explicitly with an updating indicator; a future message is hidden after
a backward seek until its query completes. Paused unchanged cursors issue no new work.

`InspectorMessage` lives in viewer-core's dedicated `inspection` module. It is not a canonical
message/domain object, never enters FeatureRuntime, and is not used by Generic Plot. Its schema
layout reuses the existing ros2msg AST and CDR reader. It supports plain XCDR1 (both endian modes),
numeric/bool/string scalars, fixed arrays, sequences, nested and empty messages. Strings display
at most 512 characters. Containers retain schema+byte offsets, not all decoded children: expansion
creates at most 64 fields/elements per page; sequential pages carry a next offset for variable-size
elements. A final variable-size container need not be traversed to display its length. Preceding
variable-size fields sometimes require bounded CDR traversal to locate a later field; traversal and
schema-depth budgets report an error rather than doing unbounded work. XCDR2/parameter-list CDR,
wstring/ros2idl and incomplete dependency definitions currently report unsupported-schema errors.

The worker permits eight topics and at most 16 MiB of compact Inspector-owned payload per result.
Compaction is intentional on this cold query path so tiny messages do not pin eight large chunks.
Up to two view generations may overlap during UI handoff; unchanged views share Arc ownership.
The physical cache remains bounded by the Native 256 MiB record/chunk limit plus descriptor/index
overhead. Each UI view retains at most 32 expanded pages / 2,048 fields, with bounded navigation
history. Large messages above the payload budget show metadata and a clear limit error.

`RangeQuery` still provides exact `[start,end)` serialized queries with message/byte caps for other
on-demand workloads. Plot and Preview keep their own specialized query/artifact paths. Inspector
UI is Native; the view model is WASM-compatible but browser cursor-query UI/transport integration
is not implemented by this change.

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
selected scalar, including the uint8 wire member inserted by ROS 2 for empty/constant-only messages.
It stops at that scalar: it neither copies strings/arrays nor creates a universal
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
points each. Pending fields for the same stream/range share one schema parse and one read of each
indexed chunk. Adding a field to a running group cancels and regroups it; completed series remain
available. One numeric worker is retained across source replacement: its cancellation is requested
and the next source waits asynchronously for it to finish before starting another numeric job.
Cancellation is checked before chunk reads/decompression and during record parsing; a codec call
already in progress finishes before cancellation can take effect. Shutdown joins the worker.
Detail/current APIs exist in core but are not
yet scheduled by this UI. Web Local/Remote preserve identical metadata and raw-message semantics;
interactive field Plot UI and query transport adapters remain future work.

Native `--layout explore` opens recordings without Camera/Odometry/TF playback requirements or
persistent bootstrap. Empty-selection playback and seek touch no message chunks, including while
advancing the timeline. Its Topic Tree is usable for scalar-only recordings. Standard/showcase retain
their own strict continuous requirements. Remote metadata retains non-CDR streams even when those
streams have no Message Index; server startup validates indexed restore for CDR streams only, and
requested indexed operations still report unsupported streams explicitly.

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

MCAP Chunk decompression and record traversal, plus Message Index predecessor
selection, are I/O-independent helpers in `viewer-mcap` used by Native and Browser
Local. Source adapters own physical reads and resource/cancellation policy; the
shared byte traversal preserves `Bytes` backing and record offsets. Sorted and
unsorted index consumption share predecessor and tie semantics without imposing
sorting or a cache on both adapters.

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

## Native recording health queries

Topic Health and Diagnostics add session-owned recent-window query demands without
FeatureRuntime routes. Topic Health uses a query-owned chunk-major Message Index traversal with one resident parsed span and reduces log times; Diagnostics
reads selected serialized ranges and aggregates producer-reported DiagnosticArray
statuses. They retain separate result types and panel semantics while sharing one
concrete coalescing window-query worker. See
[native_recording_health.md](native_recording_health.md) for boundaries, budgets,
source replacement/seek lifecycle and comparison with Inspector and Plot queries.

## Rust module placement

See [module_structure.md](module_structure.md) for current implementation owners.
Path materialization uses `PathFrame`/`PathState` independently of its BEV/Scene
views. `PointCloudController` owns LaserScan / PointCloud2 materialization; Scene presentation
retains TF projection and accumulation. Shared ROS schema and artifact identity
are source-neutral core concepts. Module moves preserve playback/query contracts.

## Physical positions

Path state now retains f64 ROS xyz in its source frame. Presentation anchors it
at measurement time and creates world Scene geometry, reference-frame BEV geometry
and camera-time projection. Platform bounds are independent core facts, configured
by the frontend; no physical feature has a renderer correction multiplier.
See [physical_geometry.md](physical_geometry.md) for contracts and bounded TF limits.

PointCloud2 XYZ and source RGBA/intensity attributes are decoded once in core; Scene presentation owns color mode and fixed intensity-range mapping. Native/Web use the same linear RGBA and existing TF/accumulation path. See [PointCloud2](point_cloud2.md).

Camera also accepts PNG CompressedImage and 8-bit ROS Image in the same routes/state/queues. Raw layout retains width, height, step and channel encoding; payload bytes remain shared until presentation admission. Raw conversion samples directly into display-sized RGBA, preserving row padding semantics without a full-size RGBA intermediate. PNG requires full codec reconstruction before resizing; its source work is bounded but not reduced by a smaller target. See [Camera image formats](issue/016-camera-image-formats/README.md) for limits and verification.

Native ROS Live explicitly declares Image topics with repeatable `--live-raw-camera-topic`.
Other configured Camera topics use CompressedImage. These source facts resolve before SessionPlan,
so subscriptions work before publishers start and selected Raw/Compressed cameras share the
same latest-only admission, feature state and bounded pixel workers. The adapter serializes borrowed
8-bit Raw pixels directly into CDR without a temporary pixel Vec. Shared encoding/layout validation
rejects unsupported encodings, malformed stride/data lengths and excessive payloads before admission.
DDS deserialization/QoS buffers remain outside the viewer's own latest-only mailbox budget.
