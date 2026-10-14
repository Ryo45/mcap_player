# Rust module responsibilities

Module names describe the concept first. Add a role such as `loader`, `controller`,
`view`, or `writer` when it identifies a distinct responsibility. Read the full
module path: `remote::catalog` needs no repeated `remote` prefix. Size alone is not
a reason to split a module; ownership and independent reasons to change are.

## Crate boundaries

- `viewer-core`: source-neutral facts, schema/CDR, pure reductions, clocks,
  feature state and presentation. No dependency on MCAP or its codecs.
- `viewer-mcap`: MCAP bytes, indexed access, recording traversal and queries.
- Frontend apps: platform I/O orchestration, async/thread lifecycle and GUI.
- Renderer crates: camera preparation/textures, BEV or Scene GPU rendering.
- `viewer-remote-protocol`, `viewer-layout`, `viewer-preview-mcap`: their concrete
  wire/document/artifact contracts. Their narrow crate context makes names such
  as `catalog`, `model`, `schema`, `reader` and `writer` sufficiently specific.

Internal modules need not be public. Explicit root exports keep the current API
reviewable independently of file layout. Parent modules may implement their own
composition. A parent entry uses its domain filename (`app.rs`, `recording.rs`,
`presentation.rs`), with children in the same-named directory. Module paths and
visibility are independent of this physical naming choice.

## Placement map

| Change | Owner |
| --- | --- |
| Browser event wiring and committed frame order | `viewer-web/app.rs` |
| Browser camera canvases, thumbnails, JPEG preparation, overlays | `viewer-web/app/camera.rs` |
| Browser source opening/replacement and its generation | `viewer-web/app/source.rs` |
| Remote source selection UI | `viewer-web/app/source/remote.rs` |
| Remote HTTP / Local file access | `viewer-web/remote`, `viewer-web/local` |
| Browser Scene/BEV GPU lifecycle and interactions | `viewer-web/app/scene.rs` |
| Browser viewer snapshot / diagnostic DOM | `viewer-web/app/presentation.rs` |
| MCAP source metadata, forward position and chunk ownership | `viewer-mcap/recording/source.rs` |
| Physical Message Index parsing, timelines, predecessor traversal | `viewer-mcap/recording/source/index.rs` |
| Independent latest-before reader and bounded caches | `viewer-mcap/recording/source/latest.rs` |
| Exact range query contract and execution | `viewer-mcap/recording/source/range.rs` |
| Pure Message Index candidate/tie algorithms | `viewer-mcap/recording/message_index.rs` |
| Latest-before demand limits/budget | `viewer-mcap/recording/latest_limits.rs` |
| Numeric, Odometry, Health, Diagnostics query execution | `viewer-mcap/query` |
| Source catalog facts and capabilities | `viewer-core/recording/catalog.rs` |
| ROS message definition parsing and field model | `viewer-core/message/schema.rs` |
| CDR wire primitives / selected numeric extraction | `viewer-core/message/cdr/wire.rs`, `numeric.rs` |
| Path feature state | `viewer-core/feature/path.rs` |
| Point Cloud admission/decode/state updates | `viewer-core/feature/controllers/point_cloud.rs` |
| Source-neutral derived artifact identity | `viewer-core/derived/source_fingerprint.rs` |
| Preview build information and products | `viewer-core/derived/preview.rs` |
| Bookmark documents | `viewer-core/derived/bookmark.rs` |
| Native builtin Odometry overview worker/request | `viewer-native/odometry_signals.rs` |
| Native borrowed signal display inputs | `viewer-native/signal_view.rs` |

Schema parsing has no numeric/Inspector dependency. Numeric discovery and compiled
CDR skipping remain in `cdr::numeric`; Inspector uses the shared schema model to
build its own lazy view. `SourceFingerprintValidationError` and
`PreviewValidationError` now belong to the concepts they validate, rather than
using bookmark errors. MCAP open/read/query failures use `McapSourceError`.

`PathFrame`/`PathState` describe path data used by both BEV and Scene;
`PointCloudController` describes LaserScan materialization, while Scene
presentation still owns transforms and accumulation. The existing controller
hierarchy remains explicit; this change introduces no new controller abstraction.

## Preserved ownership and behavior

Module moves do not change file-handle sharing, work/memory budgets, exact log-time
and tie rules, transactional seek, static-TF persistence, loader generations,
continuation completeness, camera scheduling, or wire/document formats. Browser
source-open and transport-request generations retain their separate scopes.

Native `App` remains the application root, `ViewerSession` owns sources and
queries, and workspace owns the selected feature runtime. Browser `WebPlayback`
retains its corresponding runtime and data plane. These platform ownership
choices are not unified as part of naming work.

Renderer implementations remain cohesive in their existing crates. Tool command
splitting and further GUI module changes should follow a concrete independent
change, rather than mechanically creating one file per function/type.

## Verification of this reorganization

Existing test declarations are preserved (365 before/after). Workspace tests and
Clippy with warnings denied passed, as did core/MCAP/renderers WASM checks, Web
WASM Clippy and the Trunk release build. ROS 2 Jazzy tests passed (78 passed,
2 existing ignored). Chrome exercised Local/Remote source opening and replacement,
automatic/manual Inspector refresh, exact seek, nested CDR, seven camera thumbnails
and Scene/BEV rendering. Core's dependency tree, including dev/build edges, remains
free of MCAP and its codecs.

## Imports

Import external concepts directly and explicitly from their owning crate, for
example `use viewer_core::{ArrivalTime, RawMessage, SourceCatalog};`. Do not
forward an external crate into the local root with a wildcard re-export. This
makes ownership and each file's dependencies visible without inspecting another
crate's entire API. Local public surface exports remain explicit.

## Physical geometry

`viewer-core/platform_geometry.rs` owns frame-relative metre bounds;
`presentation/path.rs` owns fixed-frame anchoring and optional TF ground-plane
projection. Controllers retain canonical xyz. `viewer-web/app/physical_settings.rs`
loads independent geometry/calibration inputs and projection settings. Renderers
consume already-placed geometry. See [physical_geometry.md](physical_geometry.md).
