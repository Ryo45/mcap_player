# Web JPEG comparison

Chrome headless, 1920×1080, DPR=1. Same 179 JPEG jobs per mode; three repetitions on one page. Medians. No MCAP I/O.

| Mode | JPEG + base canvas wall ms | Max 10 ms heartbeat gap ms | Retained base canvas MiB |
|---|---:|---:|---:|
| wasm-rgba | 16241.60 | 226.00 | 253.125 |
| bitmap-full | 10841.80 | 27.10 | 253.125 |
| bitmap-fhd | 11501.50 | 11.90 | 8.295 |
| bitmap-fhd-workers2 | 6798.60 | 13.70 | 8.295 |
