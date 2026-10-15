# Web 4K/8-camera playback and seek

Three fresh browser processes per condition. Medians of successful runs; failures are counted separately.

| Dataset | OK / attempts | Playback ms | Focus content changes | Background changes / camera | Individual seek ms | Burst recovery ms | Peak PSS MiB |
|---|---:|---:|---:|---:|---:|---:|---:|
| none-8MiB | 3 / 3 | 3909.1 | 12.0 | 11.1 | 396.4 | 536.7 | 1586.5 |
| zstd-64MiB | 0 / 3 | — | — | — | — | — | — |

Content changes are actual visible canvas image changes during playback; the initial image is excluded.
Source input is 10 Hz per camera. Product admission is focus 10 Hz / background at most 5 Hz; these are not achieved display rates.
Seek completion requires correct image fingerprints on all eight cameras plus committed cursor. Burst ends with synchronous pause.
PSS includes dedicated Chrome descendants, not total system memory or GPU VRAM. Target sampling interval is 200 ms; peaks can be missed.
Software rendering, instrumented paints, cache allowed, 4-second synthetic input. No long-history memory guarantee or hardware GPU claim.
