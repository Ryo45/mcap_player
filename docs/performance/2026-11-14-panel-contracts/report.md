# Shared panel contracts: follow-up regression check

The follow-up centralizes panel input demands, source reset and drawing dispatch. Native stores common instances as `NativePanel::Shared(Panel)`; Web no longer has its own per-panel drawing match. Both normalize source/query/GPU metadata into borrowed `PanelInput` values once per layout pass. No file reader, query executor, GPU owner or decoded history moved into `viewer-ui`.

A common declaration distinguishes continuous playback inputs, independent signal queries and Inspector topics. Status now asks only for current Odometry, removing its unnecessary Native historical Plot request. A new common panel using existing input/action capabilities needs registration, demand, reset and drawing changes inside `viewer-ui`; neither host needs another drawing variant. New transport, query or GPU capabilities still need host integration.

## Measurements

Baseline: `1052acd`, the preceding fuller egui migration. Candidate: the source committed with this report. Preserved before/after release artifacts and their hashes are recorded in [summary.json](summary.json). Three interleaved repetitions per variant on NVIDIA RTX 5070 Ti; 1920×1080, DPR 1. Native uses Standard; Web uses Monitor with all thumbnails and overlays, Plot off. This compares before/after within each host, not Native against Web.

The normal responsive UI was measured without raster patches. Both Web variants produced a 633×356 focused image and 1330×420 Scene target. FHD uses the real seven-camera TurtleBot3 sample, five-second intervals. The eight-camera 4K fixture is only 3.9 seconds, so it uses two-second intervals. The file is hashed before launching, allowing OS cache reuse. No compiler or other GPU experiment ran during accepted timing. Raw data stays under `target/panel-contract/`.

| Median | Before | After |
| --- | ---: | ---: |
| Native CPU cores | 0.196 | 0.193 |
| Native process-tree PSS, MiB | 220.5 | 221.2 |
| Web FHD CPU cores | 0.646 | 0.667 |
| Web FHD process-tree PSS, MiB | 469.3 | 466.2 |
| Web FHD focus imports/s | 10.03 | 9.91 |
| Web FHD slowest background imports/s | 4.91 | 4.86 |
| Web 4K CPU cores | 2.790 | 2.766 |
| Web 4K process-tree PSS, MiB | 1328.7 | 1301.5 |
| Web 4K focus imports/s | 10.26 | 10.00 |
| Web 4K slowest background imports/s | 2.72 | 2.46 |

The FHD CPU median is **3.2% higher**. Before runs span 0.640–0.675 cores; after 0.659–0.669, so these short samples overlap and cannot establish a persistent CPU regression or zero overhead. The slowest 4K background median is also lower; the difference is smaller than one frame per two-second interval. It must not be hidden as “identical performance.” A longer 4K recording is needed to resolve a small sustained scheduling effect. Focus remains approximately 10 Hz and all measured cursor/wall ratios approximately 1.0. Neither host stalled or showed browser errors in accepted runs. Both 4K variants miss the 5 Hz background target.

CPU 1.0 means one occupied logical core. PSS excludes separately accounted GPU VRAM; allocator/GC and high-water variation are not isolated. Image import counters measure distinct images, not compositor FPS or end-to-end freshness. Native screenshots establish visible playback; Native camera cadence is not instrumented. The Wasm gzip size changed only from 2,245,105 to 2,245,371 bytes; the larger egui-versus-DOM download tradeoff from the previous report remains.

An initial 4K attempt failed in the sampler while reading a Chrome child's restricted `/proc/PID/io`. It is excluded. The sampler now retains already-readable CPU/PSS and explicitly records `io_unavailable_pids` when optional I/O counters are inaccessible. It does not silently label partial I/O totals complete. The complete 4K comparison was rerun; no application fix was involved.

## Verification

- Workspace: **537 passed, 16 ignored**; formatting and workspace Clippy with warnings denied.
- Common wasm32 check and strict egui wasm32 Clippy.
- Native optional Scene/plugin tests: **92 passed, 8 ignored**; ROS Live feature: **88 passed, 5 ignored**.
- Native release and both Web release entries build. Native visibly plays on the hardware GPU.
- Real-GPU Local: Camera/Scene, paused zero additional image imports, Inspector, speed Plot, fixed result retained across Seek, natural EOF, touch Seek at emulated mobile size, Source replacement and stale graph result rejection.
- Real-GPU Remote: renamed bindings, Seek, BEV, speed/yaw plots, multiple Inspector panels with full ROS schemas and missing-input errors.
- Default DOM timeline checks passed separately.
- New common tests cover query-demand deduplication without continuous routes, camera identity reset with configured binding retained, and visible Status speed without a historical result, including hiding a sample after the display cursor.

Actual Android testing, PNG/Raw alpha equivalence and long-duration 4K scheduling precision remain outside this follow-up. This branch is not merged into main or deployed to Pages.

## Reproduce

Preserve the `1052acd` release Native binary and egui distribution, then build the candidate into different paths. No measurement patches are needed. For example:

```sh
python3 tools/performance-probe/measure_panel_sharing.py \
  --kind web --before target/panel-contract/before-web \
  --after target/panel-contract/after-web \
  --data target/sample-data/release/turtlebot3-7cam-fhd-10s.mcap \
  --output target/panel-contract/new-fhd --seconds 5 --repeats 3
```

Use `--kind native` with binary paths for Native, and `--seconds 2` with the short 4K fixture. The hardware helper owns an isolated Chrome process, wakes/restores the display and rejects software fallback.
