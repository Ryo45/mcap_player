# Results

Three runs per row; medians of per-run metrics. P95 column is the median of per-run P95s.

| Recording / selected | App | Observed Hz | Source progress Hz | Lag median / P95 ms | Sampled peak PSS MiB | GPU VRAM after play MiB | CPU % of one core | Seek median [min–max] ms | Rejected samples |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 1cam-20s / 1 | native | 9.69 | 9.86 | 169 / 215 | 250.1 | 25 | 34.5 | 118 [94–122] | 0 / 720 |
| 1cam-20s / 1 | lichtblick | 9.95 | 9.95 | 115 / 170 | 298.1 | 191 | 27.6 | 106 [86–122] | 0 / 720 |
| 8cam-60s-one-selected / 1 | native | 9.69 | 9.85 | 173 / 220 | 259.7 | 25 | 40.1 | 119 [119–131] | 2 / 720 |
| 8cam-60s-one-selected / 1 | lichtblick | 9.94 | 9.94 | 113 / 171 | 296.6 | 190 | 31.7 | 128 [110–144] | 9 / 720 |
