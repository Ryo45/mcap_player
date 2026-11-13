# Web egui UI experiment

Branch: `feature/web-egui-ui`. This is a separately built experiment, not a replacement published on Pages.

The production Web host assembles controls, topic browsing and Inspector fields directly in DOM callbacks. Native uses egui. The data plane and feature/presentation/rendering code are already shared; the UI/input layer is the principal source of divergence. Moving Source ownership into a new cross-platform session abstraction would not fix this UI duplication.

The experiment uses eframe 0.33.2, matching the existing egui/egui-wgpu version, to own browser input, touch, DPI, resize and the wgpu surface. A small `viewer-ui` crate contains the actual playback controls and lazy Inspector field tree used by both Native and Web. It has no browser, file or MCAP dependencies. Browser DOM access is restricted to the canvas, file picker and startup error reporting.

Local file opening, bounded indexed reads, admission, transactional restore, source generations, Inspector queries and Camera Worker decoding reuse the existing implementation. Worker ImageBitmaps upload directly into wgpu camera textures; there is no CPU canvas readback. SceneRenderer still renders to a reusable texture, composited by egui as in Native. Geometry remains metres. Queries do not alter continuous playback subscriptions.

Build the alternative entry point:

```sh
cd apps/viewer-web
trunk build egui.html --release --dist ../../target/web-egui-ui/web
```

Use the project's development container if local build prerequisites are unavailable. Serve that output through HTTP, open the page, and select an indexed MCAP using **Open MCAP**. No backend is required. The ordinary `trunk build --release` continues to build the production host for comparison.

Initial validation scope: Local MCAP, shared playback controls, independent latest-before Inspection, focused camera selection and 3D Scene. Desktop uses Topics/Inspection at left and Scene/Camera at right; narrow screens use tabs. This is intentionally not yet a production migration: Remote selection, runtime bindings JSON, BEV, camera projection overlays, layout configuration and background camera thumbnails still need porting. Their source/query/rendering implementations remain available; do not replace them with separate implementations.

The optional `egui-ui` feature isolates the experiment. WebGPU is a requirement, as explicitly agreed for this migration. WebGL was tested as a fallback, but the shared SceneRenderer requires texture view formats unavailable there. The fallback is not included in this implementation. Browsers without WebGPU show an initialization error; their former DOM-only Inspector availability is outside the agreed support requirement.

Software SwiftShader WebGPU rejected Worker ImageBitmap imports in this environment. Camera import is disabled with an explicit message for CPU adapters so the rest of the UI remains usable. This is not evidence about smartphone GPU behavior. Actual GPU testing is the acceptance target.

Loader and Worker completions are polled at 16 ms only while playback or asynchronous work is active. Settled paused views stop polling and repainting; the source picker explicitly wakes egui. Moving active-work polling to completion-driven wakes is a possible later refinement. `egui_status()` exposes a bounded read-only diagnostic snapshot for browser tests; all tested actions should use real egui pointer/touch input, not hidden DOM controls.

Replacement gate: verify Native shared-widget regressions, default Web build, real Local playback/seek/source replacement, lazy inspection, camera image formats/alpha, touch seeking after EOF, WebGPU behavior, and matched-size camera throughput/memory. Keep this branch separate until these checks and the missing production UI features are resolved. A successful rendering screenshot alone is insufficient evidence for replacement.

## Verification on 2026-11-13

The Native and default Web regression suites passed, as did workspace Clippy with warnings denied, workspace tests (528 passed, 16 ignored), the shared-crate wasm32 checks, and both Trunk release entries. The optional experimental wasm32 entry currently reports unused-code warnings for production UI functionality it does not yet expose; its strict warning-free build is not an acceptance claim.

An isolated Chrome using the NVIDIA GPU opened the released TurtleBot3 seven-camera FHD recording. The experiment showed the shared SceneRenderer, imported focused-camera images, inspected `/vehicle/speed_mps` using the shared lazy field tree, reached natural EOF, sought backwards through actual touch input at a 390×844 emulated viewport, resumed playback, and replaced the source with the camera-formats fixture. Source replacement cleared the Inspector selection/result and camera texture. Settled paused playback produced zero additional camera texture uploads. This uses desktop Chrome's touch emulation, not an Android device. The production Web timeline regression was also exercised separately.

A four-second real-GPU smoke run produced 39 focused-camera texture updates from the 10 Hz sample. This is a basic liveness check, **not** a matched-display performance comparison. Throughput, process memory, eight-camera backgrounds, 4K stress, PNG/Raw alpha equivalence and actual Android Chrome still need verification before production replacement.

With release builds and `data-wasm-opt="0"` for both entries, Wasm size was approximately 1.86 MiB for the production entry and 4.71 MiB for egui (gzip approximately 0.62 vs 1.87 MiB). This is an initial-download cost; gzip sizes were calculated locally, not measured from GitHub Pages transfers. Shared fonts and the egui browser host add code even though MCAP reading and decoding are reused.

Reproduce the canvas functional check (run from repository root, provide downloaded sample data):

```sh
python3 tools/performance-probe/check_web_egui.py \
  --data target/sample-data/release/turtlebot3-7cam-fhd-10s.mcap \
  --replacement tests/fixtures/camera-formats/camera_formats.mcap \
  --output target/web-egui-ui/check \
  --wake-display \
  --vulkan-icd /usr/share/vulkan/icd.d/nvidia_icd.json
```

The hardware-browser helper uses an isolated Chrome profile and refuses software fallback. Omit the NVIDIA ICD argument on another suitable setup. Output includes the actual desktop screenshot and a functional result JSON; these local artifacts are not committed en masse.

The experiment demonstrates that UI sharing does not require rewriting playback or browser I/O. The next small step is to move source settings (runtime bindings and Remote selection) into egui while retaining the existing settings validation and Remote client. Then migrate background camera presentation/overlays and run equivalent-workload performance checks. Remove the DOM entry and experimental feature only once production features and performance gates are satisfied; do not maintain two permanent Web UIs.
