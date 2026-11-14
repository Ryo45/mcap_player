# Shared egui panels on the Web

Branch: `feature/web-egui-ui`. The experiment now covers the main Web host's principal display and source-setting functions. It remains separate from main and published Pages so the two applications can be compared. The DOM host is a temporary comparison target, not a second UI to maintain indefinitely.

## Boundaries

`viewer-ui` owns the actual Camera, Scene, BEV, Topics, Inspector, Status and speed/yaw Plot panels: typed configuration, local interaction/expansion/chart caches and egui drawing. Their former Native panel/view implementations were removed. Playback controls, Inspector field rendering, schema details and source diagnostics also use the common code.

Panels borrow presentation/query data and texture IDs, and return `ViewerAction` / `PanelRenderRequests`. They own no source, playback clock, MCAP reader, decoded message history or GPU resources. The crate has no file, browser or MCAP dependency. `viewer-core` remains source-neutral; metre coordinates and transactional seek semantics are unchanged.

`viewer-ui::panel::Panel` owns the shared closed factory, requirement declarations, source reset and drawing dispatch. `PanelStore<P>` handles layout traversal, identity validation, instance ownership and Scene/BEV singleton rules for both concrete host enums. `viewer-ui::layout::show_layout` handles weighted splits, clipping and output aggregation. A generic store is justified by the actual two hosts: Native includes platform-specific query/plugin panels; Web needs no universal plugin registry or panel trait hierarchy. Native retains its wrapper enum for these differences.

The Web host uses eframe 0.33.2, matching egui/egui-wgpu and wgpu 27. It owns browser input, resize/DPI, I/O, independent queries and GPU textures. Native retains its OS I/O, workers and rendering bridge. Web uses existing Local/Remote data-plane loaders, Worker JPEG/PNG/Raw image work, generation checks and direct ImageBitmap-to-wgpu import without canvas readback. Scene/BEV render to reusable textures as in Native. A single ScenePresentation snapshot feeds both views per frame; a second panel does not rebuild or reset accumulated clouds. Texture resize updates existing egui bindings.

Direct DOM use is confined to browser boundaries: hidden file inputs, the eframe canvas, startup/error reporting and browser/Worker APIs. No topic list, Inspector field tree, graph or camera-control UI is built as DOM widgets in this entry.

### Camera settings have distinct owners

- **Panel display and binding:** `viewer-ui::camera::{CameraPanel,CameraPanelConfig}` owns image topic, thumbnails, overlay visibility and scheduling-priority configuration. Native/Web use the same `select_topics` resolution/validation. Focus derives from panel state rather than a second Web-owned selected-camera variable.
- **Decode/display pixels:** panels emit requested pixel sizes; browser Worker/GPU code implements them. No separate UI camera-size constants are introduced.
- **Physical calibration:** `egui_app::settings::PhysicalSettings` owns the loaded `CameraCalibrationSet`, independently of panel style/topic selection. The bundled calibration is explicitly labelled **demo, not measured**.
- **3D viewpoint:** `viewer-ui::scene::ScenePanel` and SceneRenderer own Chase/Free/VehicleEye interaction. These are not camera-sensor calibration settings.
- **Source inputs:** `OpenSettings` owns pending WorkspaceBindings/point-cloud overrides and layout requirements. These are snapshotted when opening a source. Active bindings survive camera selection and pending-setting edits.

Combining these into a giant camera/profile configuration would mix independent facts and policies. The remaining host differences are physical-file acquisition and GPU execution, not two camera panel implementations.

## Current Web functionality

| Area | Migrated behavior |
| --- | --- |
| Source | Browser Local and Recording Server list/open; stale-generation rejection; previous source retained if opening fails |
| Runtime settings | Bindings JSON/reset, explicit point-cloud override; invalid bindings block new opens |
| Physical facts | Platform geometry JSON/body enable, fixed/ground frame, camera calibration JSON; explicit errors |
| Camera | Common topic dropdown, all background thumbnails, projection overlays, focus priority; native Worker/image formats retained |
| Scene/BEV | Common panels/renderers, viewpoint, reset/orbit/zoom, accumulation and cloud color controls |
| Topics/Inspector | Common schema browsing and lazy field UI; cursor-follow/manual refresh; configured Inspector panels |
| Plot | Common speed and yaw Plot UI, separate bounded Odometry query, cursor marker/seek interaction |
| Layout | Monitor/Standard/Cameras/Inspection presets and JSON load, common split layout; narrow screens use panel tabs |

Default Web layout is `config/layouts/web_monitor.json`: Topics left, Scene and Camera right, camera thumbnails/overlay enabled. The configuration goes through the common factory, not separate Web copies of camera options. **Settings** holds source/physical file pickers and Remote selection. Bindings/config files stay in the browser. JSON files are bounded to 1 MiB. Layout validation precedes replacement.

Continuous input requirements are collected from actual panel instances before opening a source; Plot and Inspector do not add continuous streams. Local/Remote resolve the same SessionPlan. Empty initial camera preferences resolve from schema-filtered catalog facts. An image-free recording can still open; explicitly required missing inputs follow normal SessionPlan errors.

Layout changes render immediately but their **input requirements apply to the next source open**; this is shown in Settings. There is no dynamic subscription broker. Default camera panels plan all images, allowing selection during playback. A custom fixed-camera layout only plans its requested streams: the dropdown offers only those planned streams. An out-of-plan action reports an explicit reopen error. Reopen with an appropriate layout to request other streams. Native retains its existing transactional replan behavior.

The host shares one query per pinned Inspector topic and admits at most two concurrent Inspector requests, with interactive priority and round-robin pinned admission. Source/seek invalidation cancels old requests. Errors clear old message/metadata and stay local to the affected query. Each controller retains a latest snapshot, not decoded history.

One independent Odometry query decodes each selected message once for both speed and yaw, using the existing decoder and reducers. Limits: first 60 seconds, 100,000 messages, 2,048 reduced points per signal; cumulative 256 MiB read / 512 MiB decompression checks after returned windows, in addition to loader budgets. Local requests one bounded range to avoid repeated Chunk decompression; Remote requests one-second windows to respect server limits. These work checks are not pre-I/O reservations. Fixed results survive Seek. Disabled plots create no query. Source replacement/disabling/drop cancels the loader.

## Adding another panel

Implement drawing/configuration/state once in `viewer-ui` and add the type to the closed `Panel` enum/factory. Define its input demand in `Panel::contribute_requirements`, source-bound cache reset in `Panel::reset_for_source`, and borrowed-input dispatch in `Panel::show`. Both hosts already call these entry points. Native now stores all common types as `NativePanel::Shared(Panel)`; adding another common variant does not require a Native enum variant, a factory conversion arm, or a Web drawing arm.

`panel_input::PanelInput` contains one frame's borrowed presentation, texture metadata, signal/query views and preview state. Its resource/preview/scene view types are shared. Hosts normalize their actual results into this input once per layout pass; numeric-series and inspection metadata are not allocated again for every panel. There are no payload/image/cloud copies or query execution in this conversion. Native keeps typed query errors and converts them to borrowed display errors only at the UI boundary. Interactive and pinned Inspector views remain separate, preserving their independent follow/refresh behavior.

`PanelRequirements` distinguishes continuous `playback` demands, independent Plot `signals`, and deduplicated Inspector topics. Native wraps these with its own Health/Diagnostics/plugin requirements. Web caches the same declarations on layout changes and uses them to configure bounded query controllers. Status consumes current Odometry and no longer starts a historical Plot query on Native just to display speed; Plot queries remain opt-in.

Web handles Scene/BEV GPU requests after common layout drawing. Native retains its existing render pass. Camera binding/replan transactions, query workers, file/HTTP operations, generation checks and GPU resources remain host-owned. Source UI reset runs after a source commits, followed by binding the new cameras; it does not replace transactional Seek restoration.

A panel using existing inputs and actions can now be added within the common UI. A genuinely new input, query workload, command or GPU primitive still requires host integration; automatic support for arbitrary future data access is not claimed. Native Health, Diagnostics, Explore/Query and plugin panels have not been migrated; unsupported Web layout types show placeholders rather than pretending to implement their UI. Arbitrary numeric-field plots remain Native-only; Web hides their add-series buttons. These were not features of main's Web UI.

The [follow-up contract regression report](performance/2026-11-14-panel-contracts/report.md) covers the shared dispatch and lifecycle changes, including the observed FHD CPU increase and short-window background-rate variation.

## Build and run

```sh
cd apps/viewer-web
trunk build egui.html --release --dist ../../target/web-egui-ui/web
```

Use the development container if host prerequisites are unavailable. Serve the output over HTTP and open it in a **WebGPU-capable browser**. No WebGL fallback is planned. Main's ordinary `trunk build --release` remains available for comparison. GPU-less SwiftShader external-image import is not supported; the UI explicitly reports that camera limitation rather than allowing an unhandled wgpu panic.

## Verification and costs

See the [main-versus-egui performance comparison](performance/2026-11-14-web-panel-parity/report.md) and the earlier [panel-sharing comparison](performance/2026-11-14-shared-panels/report.md). Seven FHD cameras retained about 10 Hz focus / 5 Hz backgrounds. Eight 4K cameras retained about 10 Hz focus; slowest backgrounds were about 2.4 Hz main / 2.7 Hz egui. The measured egui CPU was lower and steady PSS similar. These short real-GPU tests do not prove all workloads or mobile devices have identical performance.

The release Wasm download grows: main comparison build about 1.90 MiB raw / 0.63 MiB gzip; current egui about 5.56 MiB raw / 2.14 MiB gzip (`wasm-opt=0` both). Gzip is calculated locally, not measured from Pages. UI host/fonts/plotting add an initial-download and compilation cost even when steady playback is efficient. Actual startup-to-first-image latency and mobile browser memory are still acceptance work.

Actual NVIDIA Chrome checks cover Local/Remote playback/seek, external renamed bindings, invalid bindings blocking open, multiple Inspector panels with full ROS schemas, missing-topic errors, speed/yaw queries, BEV, camera thumbnails/overlays, natural EOF, backwards touch seek at 390×844, source replacement and stale graph-read rejection. Settled paused playback causes zero additional camera imports. Desktop touch emulation is **not** actual Android testing. Bundled calibration is not evidence of measured projection accuracy. Real-GPU texture readback and rendered pixels agree for RGBA8, BGRA8 and PNG at half/zero/full alpha; this verifies the egui import path, not every browser or Native/Web color-management combination.

Verification passed after the shared-contract refactor: workspace **537 tests** (16 ignored) and Clippy with warnings denied; Native optional plugin tests **92 passed** (8 ignored), ROS Live feature tests **88 passed** (5 ignored); shared wasm32 checks, strict egui wasm32 Clippy and both Web release entries. The Native release build also rendered and visibly played on the hardware GPU. Ignored optional/environment tests were not claimed as executed. `egui_status()` is bounded read-only diagnostics; checks drive real pointer/touch events and file-picker inputs. No hidden mutable test API is used.

```sh
python3 tools/performance-probe/check_web_egui.py --plot \
  --data target/sample-data/release/turtlebot3-7cam-fhd-10s.mcap \
  --replacement tests/fixtures/camera-formats/camera_formats.mcap \
  --dist target/web-parity/egui --output target/web-parity/new-local-check \
  --wake-display --vulkan-icd /usr/share/vulkan/icd.d/nvidia_icd.json

python3 tools/performance-probe/check_web_panel_parity.py \
  --dist target/web-parity/egui --server target/web-parity/recording-server \
  --data target/workspace-bindings/recording.mcap \
  --bindings target/workspace-bindings/bindings.json \
  --sample target/sample-data/release/turtlebot3-7cam-fhd-10s.mcap \
  --output target/web-parity/new-remote-check
```

Build `recording-server` in release mode and provide the generated renamed-bindings fixture (`tools/ros-fixture/examples/workspace_bindings_fixture.rs`). The small fixture deliberately omits nested type definitions: it tests bindings/playback, while the real sample tests successful Inspector decoding. The hardware helper owns an isolated Chrome process, refuses software fallback, and explicitly wakes/restores the display; it never unlocks the desktop. Large raw results stay under ignored `target/`.

Next acceptance work is actual Android WebGPU and comparable startup-to-usable-image timing. Remove the old DOM entry and experimental feature only after those decisions; this change does not merge main or deploy Pages.

The [merge-readiness corrections](web_egui_merge_readiness.md) document strict layout validation, connection-scoped Remote catalog/list state, cancellation/deadlines, multi-channel topic metadata and the expanded regression checks.
