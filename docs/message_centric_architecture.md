# Message-centric viewer architecture

The recording is the canonical persistent store. Opening a recording must not ingest or decode
the complete log. Continuous playback keeps the existing bounded, windowed data plane and routes
exact serialized `RawMessage` values only to features required by the open workspace.

## Characterized starting point

Before this migration, both Native and Web playback reduced messages through
`DomainRuntime -> DomainState`. That runtime also owned Camera admission, focused/background
scheduling, and coalescing. Native presentation then built Camera, BEV, Scene, and telemetry views
from the global state. Web Local and Remote already shared `RecordingDataPlane`, used fixed stream
selection, retained payloads as `Bytes`, froze the committed cursor while buffering, and rejected
stale loader generations. Native seek used a transactional temporary reader/runtime and one second
of TF pre-roll. Plot and Preview were already separate query/derived-artifact paths.

The characterization tests protect Camera ordering and scheduling, TF history, transactional
cursor commit, Local/Remote message parity, zero-copy payload sharing, data-plane coverage, and
bounded browser reads while the semantic layer is replaced.

## Target ownership

```text
SourceCatalog + Workspace requirements
                 -> SessionPlan (fixed stream selection and static routes)

RecordingDataPlane / Native indexed source / Live push
                 -> RawMessage (MCAP log time, Bytes payload)
                 -> explicit feature routes
                 -> CameraController / PathController / SceneController / SharedTfState
                 -> panel views
```

Controllers decode and retain only feature-specific bounded state. Concrete state may be shared
when there are real multiple consumers (notably TF and the current planning path), but there is no
generic projection registry, event bus, decoded-message database, or replacement global semantic
state. Playback and bounded inspection remain distinct workloads and playback retains priority.

Seek restoration is planned from explicit feature semantics and catalog facts. Recording-wide
message counts are coarse hints, not inferred publish rates. Exact playback/query time is always
MCAP log time. Preview and numeric overviews remain discardable, source-identified artifacts.
