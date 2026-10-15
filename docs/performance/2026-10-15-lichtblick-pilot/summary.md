# Web viewer pilot

Three fresh Chrome processes/profiles per app. Medians, MiB = 2^20 bytes.

| App | Before open PSS MiB | Before play PSS MiB | Peak sampled playback PSS MiB | Settled PSS MiB | Cursor run s |
|---|---:|---:|---:|---:|---:|
| mcap | 315.69 | 453.02 | 1124.42 | 1022.35 | 20.10 |
| lichtblick | 438.50 | 1081.05 | 1328.71 | 1023.48 | 19.95 |

Cursor completion is not displayed FPS. UI-ready times have different predicates; see raw runs.
200 ms target sampling can miss peaks. CPU/I/O live-process counters are diagnostic only.
Dataset includes LaserScan: mcap also materializes it; Lichtblick subscribes only camera 0.
mcap Scene is hidden via display:none (zero CSS size skips surface rendering); initial GPU resources remain.
This pilot does not measure selective-topic savings, long-history growth, cold disk, seek latency or hardware GPU throughput.
