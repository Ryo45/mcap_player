# Session planning and shared domain refactor

## Purpose

This document fixes the intended boundary before the implementation is moved. It is the design
record for a staged, behavior-preserving refactor; the first stage adds characterization tests and
does not introduce `SessionPlan` or change runtime behavior.

The target flow is:

```text
                           Catalog
                              |
                        SessionPlan
                 shared-domain participation policy
                              |
                 +------------+------------+
                 |                         |
             Recording                   Live
          Local / Remote                  ROS
                 |                         |
                 +-------- RawMessage -----+
                              |
                        DomainRuntime
                              |
                     DomainPipelineSet
                              |
                       DomainUpdate
                              |
                        DomainState
                              |
                           Panels
```

`DomainState` means the world state shared by the whole Viewer session. It is not a registry of
every ROS message the Viewer can decode. Panel-specific history, caches, query results, and view
state remain outside the shared domain unless more than one Viewer capability genuinely needs the
same world state.

## Compile-time decoding and session-time policy

The way a supported schema is decoded remains compile-time Rust code such as
`decode_odometry()`, `decode_compressed_image_bytes()`, and `decode_tf_message()`. These decoder
primitives are reusable outside the shared domain. A Plot or future Inspector may decode a message
without adding a `DomainUpdate` variant.

At session open, the Viewer examines a source catalog and makes a runtime policy decision: which
concrete streams participate in the shared domain and what domain role each stream has. The
planned owner of that decision is one `SessionPlan`, initially shaped approximately as:

```rust,ignore
struct SessionPlan {
    domain_routes: Vec<DomainRoute>,
    primary_camera: Option<CameraId>,
}

struct DomainRoute {
    stream: StreamDescriptor,
    target: DomainTarget,
}

enum DomainTarget {
    Camera(CameraId),
    Telemetry,
    Path,
    PointCloud,
    Transforms { is_static: bool },
}
```

These are design sketches, not committed API. `DomainTarget` names the domain meaning rather than
the input ROS type. Schema-specific decoder selection remains in the compiled pipeline builder.
No decoder registry, `TypeMap`, typed event bus, or family of selection/binding/capability types is
needed for this migration.

## Current ownership and temporary boundaries

`SessionPlan` now owns the standard Viewer policy for cameras, `/planning/path`, `/odom`, `/scan`,
`/tf`, and `/tf_static`. It discovers compressed-image streams, moves the configured primary stream
to the first slot, and therefore determines `CameraId` assignment. `DomainPipelineSet` turns the
plan's routes into schema-checked pipelines. `DomainRuntime` owns that pipeline set, `DomainState`,
camera coalescing, camera presentation scheduling, and domain reduction metrics. `PlaybackCore` is
now a compatibility facade that adds source-read timing and delegates shared-domain work.

Recording I/O, buffering, prefetch, and cursor candidate/commit remain outside `PlaybackCore`.
Web local-file and remote-server recordings already share `RecordingDataPlane`; Native local
playback retains its mmap-based `McapPlayback` path. Those source differences are intentionally not
part of this refactor stage.

Web Local now retains every MCAP channel in its source catalog, while Web Remote retains every
raw ROS 2 CDR representation supported by the existing transport contract. Both build the same
`SessionPlan` and derive their fixed loader selection from its routes: Local translates routes to
topics and Remote translates them to server stream IDs. Loader coverage therefore remains defined
against one immutable selection, while unselected source descriptors stay available for future
panel-specific queries.

The first concrete panel-specific path is the Native vehicle-speed Plot. The Plot contributes a
single concrete `vehicle_speed` requirement through `NativeWorkspace`; `PlaybackSession` turns it
into a recording-only speed query request, and a Session-owned worker returns `LoadedSignal` to the
Plot view. The panel never receives a path, mmap, playback session, or MCAP reader, and odometry
samples are not added to `DomainState` merely to serve the full-resolution Plot query. A second,
explicit Inspector query can read bounded arrival/payload metadata for one topic through the same
Session ownership boundary without mutating `DomainState`; no generic query manager or Inspector
UI framework was introduced.

Native Live now feeds its camera-only `LatestMailbox` into the same `DomainRuntime` used by
Recording. The latest-only behavior belongs to that current camera adapter; it must not be
generalized to all future live domain streams because ordered TF updates cannot safely use the same
drop policy.

## Invariants to preserve

- Catalog order plus primary-camera selection produces the same Camera IDs and topics.
- `/odom` updates telemetry; `/planning/path` updates the BEV path; `/scan` updates the source-frame
  point cloud; `/tf` and `/tf_static` update dynamic and static transform history respectively.
- Pipeline dispatch is built once from stream IDs. The runtime hot path does not repeat topic or
  schema policy decisions.
- Focused cameras retain the current 10 Hz presentation policy, background cameras the current
  5 Hz policy, and pending camera messages remain latest-only/coalesced.
- Recording reads and domain processing succeed before the visible playback cursor is committed.
- A cold seek clears transient domain state, preserves static transforms, restores the current TF
  pre-roll behavior, and does not expose partially staged state.
- `RecordingDataPlane` keeps a fixed stream selection and its existing coverage semantics.
- Local and remote recording windows reduce to the same domain result.
- Panels do not own source implementations, MCAP readers, or remote clients. Shared domain data may
  be observed by panels; panel-specific data must travel through a session-owned path.

## Characterization coverage in stage 0

| Behavior | Test boundary |
| --- | --- |
| camera discovery, primary camera, CameraId order, standard topic routes | `session_plan::tests::builds_current_camera_and_shared_domain_policy_once` |
| Camera, odometry, path, scan, dynamic/static TF reduction | `tests/tf_fixture.rs` |
| schema-checked pipeline dispatch and Camera CDR/JPEG ownership | `pipeline::tests` |
| focused/background scheduling and camera coalescing | `tests/playback_scenario.rs` |
| transactional/cold seek domain behavior | `tests/playback_scenario.rs` and `playback::tests` |
| current Live camera mailbox to domain path | `viewer-native::live::tests` |
| local IndexedReader and remote batch parity | `viewer-web::playback::tests::local_indexed_and_remote_batch_windows_reduce_to_the_same_domain_state` |

The Live test deliberately characterizes only the existing camera path. Multi-stream live routing
and per-route admission policies need concrete implementations before a broader abstraction is
chosen.

## Staged migration

1. **Stage 0 — design and characterization:** this document and tests only.
2. **Stage 1 — `SessionPlan`:** represent the current `standard_bindings()` result without changing
   routing behavior, then make the builder the single owner of Viewer domain policy.
3. **Stage 2 — domain pipeline naming and construction:** make the domain-only role of
   `PipelineSet` explicit and build it from the plan while keeping decoder functions reusable.
4. **Stage 3 — `DomainRuntime`:** move domain reduction, state, camera admission/scheduling, and
   domain metrics out of `PlaybackCore`; leave clock, data plane, buffering, and seek orchestration
   outside.
5. **Stage 4 — Live convergence:** replace Live's duplicate pipeline/state pair with the same
   `DomainRuntime`, without forcing Recording and Live behind one source trait.
6. **Stage 5 — Local/Remote planning cleanup:** make catalogs describe source contents and make
   both recording adapters use the same session policy. Fixed data-plane selection stays intact.
7. **Stage 6 — one panel-specific vertical slice:** implement a non-domain message request and
   panel-owned state before designing any reusable query/router mechanism.
8. **Stage 7 — Plot and Inspector:** move source access behind session/recording query boundaries
   while keeping query results outside `DomainState`.
9. **Stage 8 — namespace and ID cleanup:** reconsider names and newtypes only after ownership has
   stabilized.

Each stage is independently buildable and testable. A new abstraction is deferred until at least
two concrete uses demonstrate the need for it.
