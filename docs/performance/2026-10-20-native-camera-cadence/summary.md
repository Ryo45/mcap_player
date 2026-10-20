# Native Camera cadence results

Three runs per variant; medians of per-run metrics. P95 is the median of per-run P95s.

| Variant | Observed Hz | Progress Hz | Lag median / P95 ms | Sampled peak PSS MiB | CPU % of one core |
|---|---:|---:|---:|---:|---:|
| baseline | 9.78 | 9.86 | 165 / 217 | 260.2 | 40.8 |
| periodic | 9.86 | 9.86 | 157 / 170 | 269.4 | 39.8 |

Separate diagnostic runs, n=1 per variant; stages are CPU boundaries, not GPU completion.

| Stage | Baseline median / P95 ms | Periodic median / P95 ms |
|---|---:|---:|
| received->admitted | 0.03 / 50.09 | 0.02 / 0.03 |
| admitted->submitted | 0.55 / 2.03 | 0.29 / 2.22 |
| submitted->started | 0.03 / 0.04 | 0.02 / 0.03 |
| started->decoded | 32.53 / 32.90 | 29.77 / 32.50 |
| decoded->uploaded | 7.88 / 25.30 | 5.95 / 15.78 |
| uploaded->painted | 0.25 / 1.08 | 0.07 / 0.09 |
| received->painted | 50.47 / 93.22 | 35.66 / 50.03 |
