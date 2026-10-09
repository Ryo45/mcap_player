# Session and feature-controller ownership

This document previously described the interim `DomainRuntime -> DomainState` refactor. That
intermediate architecture has been removed. The current design is documented in
[`message_centric_architecture.md`](message_centric_architecture.md).

The stable ownership boundary is now:

```text
SourceCatalog
  -> workspace PlaybackRequirements
  -> SessionPlan
  -> Recording or Live RawMessage delivery
  -> FeatureRuntime (admission/coalescing/materialization in concrete controllers)
  -> read-only presentation inputs
  -> panels
```

`ViewerSession` owns the open Recording or Live source and session-owned query paths. Native
`NativeWorkspace` owns the controllers selected by its layout. Web playback owns the corresponding
controllers next to its `RecordingDataPlane`. Panels receive narrow read-only inputs and never own
an MCAP reader, HTTP client, loader, or playback session.

Bounded Plot overviews and occasional exact detail/current values remain separate session queries.
Preview remains a derived sidecar path. Neither is inserted into continuous controller state merely to make it globally
available. New panel-specific message support should normally add a decoder/controller and a
requirement, not a universal state variant.
