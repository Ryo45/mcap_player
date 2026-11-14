# Web egui merge-readiness corrections

2026-11-14. Branch: `feature/web-egui-ui`. This records the correction pass after the shared-panel migration and the final changes for promotion to the default Web entry.

## Findings and fixes

| Finding | Correction |
| --- | --- |
| External layout JSON was parsed without validating the document; an empty split could later panic when enabling Plot | Validate before replacing any layout/panels/requirements. Reject unsupported versions, invalid weights, duplicate IDs and excessive panel counts. Guard first-child access. Removing the final Plot creates a supported Topics panel. |
| A recording list fetched from A could be used with a newly edited server URL B | Keep the list, catalog and API client in one connection. URL edits clear connection/selection/catalog; Open requires a selection from that connection. |
| A hung Remote request kept source controls busy indefinitely | Add cancellation and a 30-second deadline, using the existing AbortSignal and generation checks. Only the request owning the current generation may clear busy state or commit results. |
| Topic details could show the first channel's schema while Inspector displayed a different channel | One selectable row per topic, details for every channel, explicit channel of the displayed message, per-channel schema/encoding/count. Restore case-insensitive filtering. |
| Fixed-camera layouts offered topics outside their initial SessionPlan | Offer only planned cameras; reject an out-of-plan action explicitly. Settings explains reopening after changing input requirements. |
| DOM's standalone catalog preview was missing | Add **Load Catalog only** to Remote settings, including recording revision/time range/channel metadata. It neither opens playback nor reads message batches. |
| Web compiled numeric-field exploration for every schema without exposing that feature | Build metadata-only TopicTree on Web; Native retains numeric exploration. Definitions remain shared by Arc and Inspector still decodes lazily. |
| Fresh startup controls could overlap because an empty source label participated in the wrapped header | Omit the empty label. Settings now toggles and scrolls within the viewport. |

No new source/query broker, cache, SessionPlan reconfiguration or renderer abstraction was introduced. Source commits still reset bitmap/GPU/presentation/query state after the candidate has opened successfully. Query and continuous playback remain separate.

## Verification

- Workspace: **540 passed, 16 ignored**, Clippy with warnings denied and formatting checks.
- Native optional plugin configuration: **92 passed, 8 ignored**. ROS 2 Live: **88 passed, 5 ignored** in the sourced container environment.
- Shared wasm32 checks, strict egui wasm32 Clippy, egui and default DOM release builds.
- Actual NVIDIA Chrome: Local playback/pause, Inspector, bounded speed/yaw Plot, Plot-driven Seek without query rerun, EOF/backwards touch Seek/replay, Source replacement and stale Plot read rejection.
- Remote: renamed bindings, malformed bindings preventing open, camera selection, BEV/layout/Inspector panels, catalog-only operation without message reads.
- Fault injection at standard fetch/clock boundaries: cancel, timeout, edited URL and late successful responses; cancelled Remote Open followed by Local Open cannot replace the newer source. Five invalid layout classes preserve source/cursor/layout; subsequent Plot controls remain usable.
- Fixed camera plan: only its four requested cameras are offered even when the catalog has seven.
- GPU pixel checks: RGBA8, BGRA8 and PNG match expected premultiplied texels and rendered RGB at half/zero/opaque alpha, including repeated Seek. COPY_SRC is added to the test texture only for one-texel readback; production rendering is unchanged. This is functional verification, not a performance benchmark.
- Desktop-to-390×844 emulation: Settings remains accessible. This is **not an actual Android device test**.

Ignored tests are not claimed as run. Existing Source/Seek rollback and stale-query tests remain part of the suite. The source-revision diagnostic increments only at a successful commit, allowing tests to distinguish reopening the same filename. Tests drive real egui input/file pickers and observe bounded read-only diagnostics; no mutable Rust test API was added.

Raw logs, screenshots and per-run traces are deliberately kept under ignored `target/web-merge-audit/`; the automated checks are retained in Git.

## Performance spot check

Against the previous branch commit `f4aa021`, seven FHD cameras were measured for five seconds, three alternating repetitions per variant, without a simultaneous build or GPU workload. Median CPU was **0.672 → 0.633 cores**, median PSS **473.1 → 474.5 MiB**, camera imports **40.03 → 40.00/s**, and timeline progress **1.003 → 1.003 s/s**. Focus stayed **9.84–10.00 Hz**, with every background **at least 4.90 Hz**. Paused playback added zero camera imports. This short check shows no material regression in this workload; it does not establish a general speedup or settle the earlier 4K background-rate variation. See the [small retained summary](web_egui_merge_readiness_summary.json) and [earlier broader comparison](performance/2026-11-14-panel-contracts/report.md).

Native also built and visibly played on NVIDIA/Vulkan. The default DOM entry passed its unchanged-Seek/pointer-cancellation/blur regression check. Native smoke does not claim automated Native Seek verification.

## Reproduce

Build both entries as described in [the experiment](web_egui_ui_experiment.md). Build `recording-server` in release mode. Generate the existing renamed-bindings fixture if needed. With the released FHD sample available locally:

```sh
python3 tools/performance-probe/check_web_merge_readiness.py \
  --dist target/web-merge-audit/egui \
  --server target/web-parity/recording-server \
  --data target/sample-data/release/turtlebot3-7cam-fhd-10s.mcap \
  --replacement target/workspace-bindings/recording.mcap \
  --output target/web-merge-audit/new-fault-check
```

The test owns isolated servers/Chrome and explicitly wakes/restores the display. `--replacement` is the renamed-bindings recording used as the second server's same-ID recording. RGBA/PNG checks use the tracked small `camera_alpha.mcap` fixture. See `check_web_egui.py --plot` and `check_web_panel_parity.py` for ordinary playback checks.

## Remaining acceptance work

Actual Android Chrome WebGPU, mobile memory/startup-to-first-image timing and broader Native/Web color-management comparisons remain unverified. Layout input requirements apply to the next source open; changing them does not silently rebuild the active SessionPlan. Native-only Health/Diagnostics/Explore/plugin panels and arbitrary numeric-field plots remain outside this Web migration; unsupported panel types report placeholders.

The standard Trunk/Pages target is now `egui.html`; the generated entry is still `index.html` and uses the configured Pages base path. The old DOM source entry can be built explicitly for comparisons; deleting it is deferred.

Final pre-merge corrections explicitly schedule repaint every 100 ms while a Remote request is pending, independently of spinner animation, so a stalled request reaches its deadline even when Settings is closed and playback is paused. Physical body visibility, geometry JSON, successful frame application and calibration JSON invalidate both Scene and BEV renderer caches. Existing presentation resets for geometry/frame changes are retained; body visibility changes preserve accumulated cloud history and camera textures. The real-GPU regression checks wait through the real 30-second deadline without pointer/keyboard input, and check that physical edits refresh both GPU layers once without moving the cursor or reimporting images.

Final default-entry verification reran workspace Clippy and all 540 tests (16 ignored), shared wasm32 checks, and the standard locked Trunk release build with `/mcap_player/`. The targeted Web/UI/Scene/BEV packages passed 100 tests (5 ignored). Optional plugin/ROS Live counts above refer to the preceding correction pass; these configurations were not rerun for the Web-only repaint/cache changes.
