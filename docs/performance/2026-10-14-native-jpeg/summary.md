# Native JPEG decode comparison

Three fresh processes per condition, CPU decode (workers2 uses two threads and a per-tick barrier). No GPU. Times/RSS are medians.

| Dataset | Decode | Read ms | JPEG ms | Work ms / 4000 ms log | JPEG count | Retained RGBA MiB | Peak RSS MiB |
|---|---|---:|---:|---:|---:|---:|---:|
| 3840x2160-8cam-10hz-none-8MiB-jpeg | rgba | 855.86 | 7091.59 | 7929.10 / 4000 | 179 | 253.125 | 451.14 |
| 3840x2160-8cam-10hz-none-8MiB-jpeg | native-full | 180.42 | 5795.00 | 5975.57 / 4000 | 179 | 253.125 | 401.75 |
| 3840x2160-8cam-10hz-none-8MiB-jpeg | native-half | 855.18 | 4651.24 | 5510.36 / 4000 | 179 | 63.281 | 170.93 |
| 3840x2160-8cam-10hz-none-8MiB-jpeg | native-layout | 848.83 | 3930.38 | 4784.75 / 4000 | 179 | 11.371 | 113.39 |
| 3840x2160-8cam-10hz-none-8MiB-jpeg | native-layout-workers2 | 897.61 | 2725.36 | 3633.01 / 4000 | 179 | 11.371 | 131.21 |
| 3840x2160-8cam-10hz-zstd-64MiB-jpeg | rgba | 1344.34 | 7015.03 | 8389.22 / 4000 | 179 | 253.125 | 566.80 |
| 3840x2160-8cam-10hz-zstd-64MiB-jpeg | native-full | 1341.56 | 5809.02 | 7172.67 / 4000 | 179 | 253.125 | 516.75 |
| 3840x2160-8cam-10hz-zstd-64MiB-jpeg | native-half | 1365.98 | 4680.88 | 6066.97 / 4000 | 179 | 63.281 | 279.24 |
| 3840x2160-8cam-10hz-zstd-64MiB-jpeg | native-layout | 1371.14 | 3941.70 | 5342.22 / 4000 | 179 | 11.371 | 227.81 |
| 3840x2160-8cam-10hz-zstd-64MiB-jpeg | native-layout-workers2 | 1369.16 | 2704.66 | 4102.24 / 4000 | 179 | 11.371 | 245.12 |
| 3840x2160-4cam-20hz-zstd-64MiB-jpeg | rgba | 1500.85 | 3986.74 | 5512.50 / 4000 | 100 | 126.562 | 440.54 |
| 3840x2160-4cam-20hz-zstd-64MiB-jpeg | native-full | 1494.87 | 3259.92 | 4785.50 / 4000 | 100 | 126.562 | 390.42 |
| 3840x2160-4cam-20hz-zstd-64MiB-jpeg | native-half | 1526.30 | 2605.98 | 4156.98 / 4000 | 100 | 31.641 | 247.88 |
| 3840x2160-4cam-20hz-zstd-64MiB-jpeg | native-layout | 1508.00 | 2310.66 | 3847.02 / 4000 | 100 | 9.393 | 226.25 |
| 3840x2160-4cam-20hz-zstd-64MiB-jpeg | native-layout-workers2 | 1515.27 | 1625.74 | 3175.00 / 4000 | 100 | 9.393 | 233.27 |
| 3840x2160-8cam-10hz-zstd-512MiB-grouped-jpeg | rgba | 1284.10 | 6917.55 | 8237.39 / 4000 | 179 | 253.125 | 822.45 |
| 3840x2160-8cam-10hz-zstd-512MiB-grouped-jpeg | native-full | 1338.10 | 5734.24 | 7076.40 / 4000 | 179 | 253.125 | 793.86 |
| 3840x2160-8cam-10hz-zstd-512MiB-grouped-jpeg | native-half | 1364.59 | 4610.17 | 5989.68 / 4000 | 179 | 63.281 | 631.27 |
| 3840x2160-8cam-10hz-zstd-512MiB-grouped-jpeg | native-layout | 1374.17 | 3897.25 | 5285.14 / 4000 | 179 | 11.371 | 579.38 |
| 3840x2160-8cam-10hz-zstd-512MiB-grouped-jpeg | native-layout-workers2 | 1377.01 | 2664.92 | 4050.77 / 4000 | 179 | 11.371 | 611.39 |
