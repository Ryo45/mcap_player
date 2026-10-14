# Headless measurements

Fresh process per repetition; cache state uncontrolled/cache-allowed.
Times are medians across runs. MiB = 2^20 bytes. RSS is sampled Linux process RSS, not heap size.

| Case | Selection / path | n | Read MiB | Expanded MiB | Work ms | RSS final / HWM MiB | CDR / JPEG decodes |
|---|---|---:|---:|---:|---:|---:|---:|
| lz4-1048576-grouped-20s-jpeg | all / forward | 3 | 10.923 | 11.420 | 47.47 | 6.66 / 6.85 | 280 / 80 |
| lz4-1048576-grouped-20s-jpeg | all / indexed | 3 | 10.923 | 11.419 | 51.91 | 6.54 / 6.67 | 280 / 80 |
| lz4-1048576-grouped-20s-jpeg | scan / forward | 3 | 10.923 | 11.420 | 14.82 | 3.97 / 4.20 | 200 / 0 |
| lz4-1048576-grouped-20s-jpeg | scan / indexed | 3 | 0.034 | 0.293 | 2.72 | 3.81 / 3.81 | 200 / 0 |
| lz4-1048576-mixed-20s-jpeg | all / forward | 3 | 10.930 | 11.420 | 46.82 | 5.60 / 9.06 | 280 / 80 |
| lz4-1048576-mixed-20s-jpeg | all / indexed | 3 | 28.444 | 29.726 | 58.91 | 6.80 / 10.79 | 280 / 80 |
| lz4-1048576-mixed-20s-jpeg | scan / forward | 3 | 10.930 | 11.420 | 10.20 | 5.95 / 7.66 | 200 / 0 |
| lz4-1048576-mixed-20s-jpeg | scan / indexed | 3 | 28.444 | 29.726 | 25.85 | 4.80 / 7.58 | 200 / 0 |
| lz4-1048576-mixed-20s-jpeg | scan / repeat | 3 | 61.309 | 64.045 | 35.52 | 6.60 / 6.60 | 0 / 0 |
| lz4-1048576-mixed-20s-jpeg | scan / repeat-cached | 3 | 0.961 | 1.001 | 2.66 | 3.73 / 5.61 | 0 / 0 |
| lz4-1048576-mixed-20s-jpeg | scan / seek | 3 | 59.330 | 61.950 | 61.27 | 4.70 / 6.51 | 0 / 0 |
| lz4-1048576-mixed-20s-jpeg | scan / seek-cached | 3 | 53.481 | 55.843 | 59.82 | 3.78 / 5.79 | 0 / 0 |
| lz4-65536-grouped-20s-jpeg | all / forward | 3 | 10.982 | 11.420 | 50.29 | 6.38 / 6.38 | 280 / 80 |
| lz4-65536-grouped-20s-jpeg | all / indexed | 3 | 10.982 | 11.419 | 47.47 | 6.36 / 6.36 | 280 / 80 |
| lz4-65536-grouped-20s-jpeg | scan / forward | 3 | 10.982 | 11.420 | 10.45 | 4.12 / 4.12 | 200 / 0 |
| lz4-65536-grouped-20s-jpeg | scan / indexed | 3 | 0.041 | 0.293 | 2.85 | 3.98 / 3.98 | 200 / 0 |
| lz4-65536-mixed-20s-jpeg | all / forward | 3 | 11.025 | 11.420 | 48.36 | 6.43 / 6.43 | 280 / 80 |
| lz4-65536-mixed-20s-jpeg | all / indexed | 3 | 12.602 | 13.066 | 56.65 | 6.29 / 6.29 | 280 / 80 |
| lz4-65536-mixed-20s-jpeg | scan / forward | 3 | 11.025 | 11.420 | 8.69 | 4.34 / 4.79 | 200 / 0 |
| lz4-65536-mixed-20s-jpeg | scan / indexed | 3 | 9.900 | 10.313 | 11.41 | 4.78 / 4.78 | 200 / 0 |
| lz4-65536-mixed-20s-jpeg | scan / repeat | 3 | 5.392 | 5.602 | 4.79 | 4.11 / 4.11 | 0 / 0 |
| lz4-65536-mixed-20s-jpeg | scan / repeat-cached | 3 | 0.100 | 0.088 | 0.38 | 4.01 / 4.08 | 0 / 0 |
| lz4-65536-mixed-20s-jpeg | scan / seek | 3 | 5.316 | 5.520 | 9.03 | 3.98 / 3.98 | 0 / 0 |
| lz4-65536-mixed-20s-jpeg | scan / seek-cached | 3 | 5.316 | 5.520 | 6.49 | 4.00 / 4.04 | 0 / 0 |
| lz4-8388608-grouped-20s-jpeg | all / forward | 3 | 10.923 | 11.420 | 53.01 | 6.67 / 6.80 | 280 / 80 |
| lz4-8388608-grouped-20s-jpeg | all / indexed | 3 | 10.923 | 11.419 | 50.23 | 6.54 / 6.67 | 280 / 80 |
| lz4-8388608-grouped-20s-jpeg | scan / forward | 3 | 10.923 | 11.420 | 14.63 | 3.98 / 4.23 | 200 / 0 |
| lz4-8388608-grouped-20s-jpeg | scan / indexed | 3 | 0.034 | 0.293 | 2.71 | 3.91 / 3.91 | 200 / 0 |
| lz4-8388608-mixed-20s-jpeg | all / forward | 3 | 10.922 | 11.420 | 49.53 | 11.94 / 19.64 | 280 / 80 |
| lz4-8388608-mixed-20s-jpeg | all / indexed | 3 | 134.571 | 140.742 | 133.30 | 20.86 / 36.75 | 280 / 80 |
| lz4-8388608-mixed-20s-jpeg | scan / forward | 3 | 10.922 | 11.420 | 13.83 | 10.62 / 19.29 | 200 / 0 |
| lz4-8388608-mixed-20s-jpeg | scan / indexed | 3 | 134.571 | 140.742 | 154.06 | 10.66 / 19.56 | 200 / 0 |
| lz4-8388608-mixed-20s-jpeg | scan / repeat | 3 | 491.426 | 513.566 | 584.67 | 3.86 / 19.61 | 0 / 0 |
| lz4-8388608-mixed-20s-jpeg | scan / repeat-cached | 3 | 7.680 | 8.024 | 9.52 | 3.68 / 19.41 | 0 / 0 |
| lz4-8388608-mixed-20s-jpeg | scan / seek | 3 | 385.165 | 402.479 | 419.75 | 10.59 / 19.50 | 0 / 0 |
| lz4-8388608-mixed-20s-jpeg | scan / seek-cached | 3 | 145.336 | 151.860 | 154.39 | 10.57 / 19.26 | 0 / 0 |
| none-1048576-grouped-20s-jpeg | all / forward | 3 | 11.434 | 11.420 | 47.60 | 6.13 / 6.21 | 280 / 80 |
| none-1048576-grouped-20s-jpeg | all / indexed | 3 | 11.433 | 11.419 | 42.05 | 5.91 / 5.91 | 280 / 80 |
| none-1048576-grouped-20s-jpeg | scan / forward | 3 | 11.434 | 11.420 | 7.91 | 3.88 / 3.88 | 200 / 0 |
| none-1048576-grouped-20s-jpeg | scan / indexed | 3 | 0.303 | 0.293 | 4.01 | 3.70 / 3.70 | 200 / 0 |
| none-1048576-mixed-20s-jpeg | all / forward | 3 | 11.424 | 11.420 | 41.87 | 7.14 / 7.70 | 280 / 80 |
| none-1048576-mixed-20s-jpeg | all / indexed | 3 | 29.730 | 29.726 | 46.29 | 7.34 / 8.83 | 280 / 80 |
| none-1048576-mixed-20s-jpeg | scan / forward | 3 | 11.424 | 11.420 | 4.16 | 5.71 / 7.52 | 200 / 0 |
| none-1048576-mixed-20s-jpeg | scan / indexed | 3 | 29.730 | 29.726 | 7.70 | 5.61 / 6.53 | 200 / 0 |
| none-1048576-mixed-20s-jpeg | scan / repeat | 3 | 64.107 | 64.045 | 7.39 | 5.37 / 5.37 | 0 / 0 |
| none-1048576-mixed-20s-jpeg | scan / repeat-cached | 3 | 1.004 | 1.001 | 0.70 | 3.54 / 4.45 | 0 / 0 |
| none-1048576-mixed-20s-jpeg | scan / seek | 3 | 62.010 | 61.950 | 8.66 | 4.68 / 4.68 | 0 / 0 |
| none-1048576-mixed-20s-jpeg | scan / seek-cached | 3 | 55.897 | 55.843 | 8.98 | 5.45 / 5.45 | 0 / 0 |
| none-1048576-mixed-600s-raw | all / forward | 3 | 2646.848 | 2646.453 | 1607.85 | 4.98 / 10.32 | 6000 / 0 |
| none-1048576-mixed-600s-raw | all / indexed | 3 | 3308.702 | 3308.279 | 2012.58 | 4.92 / 10.18 | 6000 / 0 |
| none-1048576-mixed-600s-raw | scan / forward | 3 | 2646.848 | 2646.453 | 1621.56 | 4.96 / 10.36 | 6000 / 0 |
| none-1048576-mixed-600s-raw | scan / indexed | 3 | 3308.702 | 3308.279 | 1987.44 | 4.87 / 10.27 | 6000 / 0 |
| none-1048576-mixed-600s-scan | all / forward | 3 | 8.787 | 8.784 | 19.78 | 5.84 / 5.84 | 6000 / 0 |
| none-1048576-mixed-600s-scan | all / indexed | 3 | 596.758 | 596.727 | 83.00 | 5.70 / 5.70 | 6000 / 0 |
| none-1048576-mixed-600s-scan | scan / forward | 3 | 8.787 | 8.784 | 19.58 | 5.83 / 5.83 | 6000 / 0 |
| none-1048576-mixed-600s-scan | scan / indexed | 3 | 596.758 | 596.727 | 83.36 | 5.67 / 5.67 | 6000 / 0 |
| none-1048576-mixed-60s-raw | all / forward | 3 | 264.687 | 264.646 | 40.52 | 5.94 / 10.20 | 600 / 0 |
| none-1048576-mixed-60s-raw | all / indexed | 3 | 329.878 | 329.834 | 178.23 | 4.93 / 10.18 | 600 / 0 |
| none-1048576-mixed-60s-raw | scan / forward | 3 | 264.687 | 264.646 | 137.87 | 4.96 / 10.28 | 600 / 0 |
| none-1048576-mixed-60s-raw | scan / indexed | 3 | 329.878 | 329.834 | 106.14 | 5.99 / 10.15 | 600 / 0 |
| none-1048576-mixed-60s-scan | all / forward | 3 | 0.881 | 0.879 | 2.14 | 4.52 / 4.52 | 600 / 0 |
| none-1048576-mixed-60s-scan | all / indexed | 3 | 52.759 | 52.755 | 8.54 | 4.51 / 4.51 | 600 / 0 |
| none-1048576-mixed-60s-scan | scan / forward | 3 | 0.881 | 0.879 | 2.72 | 4.50 / 4.50 | 600 / 0 |
| none-1048576-mixed-60s-scan | scan / indexed | 3 | 52.759 | 52.755 | 8.17 | 4.52 / 4.52 | 600 / 0 |
| none-65536-grouped-20s-jpeg | all / forward | 3 | 11.444 | 11.420 | 40.42 | 6.04 / 6.04 | 280 / 80 |
| none-65536-grouped-20s-jpeg | all / indexed | 3 | 11.443 | 11.419 | 47.50 | 6.01 / 6.01 | 280 / 80 |
| none-65536-grouped-20s-jpeg | scan / forward | 3 | 11.444 | 11.420 | 2.60 | 3.75 / 3.75 | 200 / 0 |
| none-65536-grouped-20s-jpeg | scan / indexed | 3 | 0.309 | 0.293 | 0.88 | 3.65 / 3.65 | 200 / 0 |
| none-65536-mixed-20s-jpeg | all / forward | 3 | 11.442 | 11.420 | 47.54 | 6.12 / 6.24 | 280 / 80 |
| none-65536-mixed-20s-jpeg | all / indexed | 3 | 13.089 | 13.066 | 41.67 | 5.94 / 5.94 | 280 / 80 |
| none-65536-mixed-20s-jpeg | scan / forward | 3 | 11.442 | 11.420 | 8.02 | 4.17 / 4.34 | 200 / 0 |
| none-65536-mixed-20s-jpeg | scan / indexed | 3 | 10.334 | 10.313 | 8.02 | 4.22 / 4.22 | 200 / 0 |
| none-65536-mixed-20s-jpeg | scan / repeat | 3 | 5.629 | 5.602 | 0.96 | 3.65 / 3.65 | 0 / 0 |
| none-65536-mixed-20s-jpeg | scan / repeat-cached | 3 | 0.103 | 0.088 | 0.27 | 3.65 / 3.65 | 0 / 0 |
| none-65536-mixed-20s-jpeg | scan / seek | 3 | 5.550 | 5.520 | 1.34 | 3.63 / 3.63 | 0 / 0 |
| none-65536-mixed-20s-jpeg | scan / seek-cached | 3 | 5.550 | 5.520 | 5.77 | 3.57 / 3.57 | 0 / 0 |
| none-8388608-grouped-20s-jpeg | all / forward | 3 | 11.434 | 11.420 | 42.17 | 6.18 / 6.25 | 280 / 80 |
| none-8388608-grouped-20s-jpeg | all / indexed | 3 | 11.433 | 11.419 | 47.10 | 5.96 / 6.02 | 280 / 80 |
| none-8388608-grouped-20s-jpeg | scan / forward | 3 | 11.434 | 11.420 | 7.64 | 3.87 / 3.87 | 200 / 0 |
| none-8388608-grouped-20s-jpeg | scan / indexed | 3 | 0.303 | 0.293 | 3.98 | 3.71 / 3.71 | 200 / 0 |
| none-8388608-mixed-20s-jpeg | all / forward | 3 | 11.422 | 11.420 | 41.68 | 8.54 / 16.33 | 280 / 80 |
| none-8388608-mixed-20s-jpeg | all / indexed | 3 | 140.745 | 140.742 | 68.15 | 11.74 / 28.75 | 280 / 80 |
| none-8388608-mixed-20s-jpeg | scan / forward | 3 | 11.422 | 11.420 | 5.60 | 7.08 / 14.88 | 200 / 0 |
| none-8388608-mixed-20s-jpeg | scan / indexed | 3 | 140.745 | 140.742 | 22.17 | 15.02 / 15.02 | 200 / 0 |
| none-8388608-mixed-20s-jpeg | scan / repeat | 3 | 513.987 | 513.566 | 58.56 | 11.57 / 11.57 | 0 / 0 |
| none-8388608-mixed-20s-jpeg | scan / repeat-cached | 3 | 8.033 | 8.024 | 3.52 | 3.59 / 11.42 | 0 / 0 |
| none-8388608-mixed-20s-jpeg | scan / seek | 3 | 402.810 | 402.479 | 50.19 | 11.60 / 11.60 | 0 / 0 |
| none-8388608-mixed-20s-jpeg | scan / seek-cached | 3 | 151.986 | 151.860 | 28.19 | 14.98 / 14.98 | 0 / 0 |
| zstd-1048576-grouped-20s-jpeg | all / forward | 3 | 10.797 | 11.420 | 46.48 | 6.50 / 6.50 | 280 / 80 |
| zstd-1048576-grouped-20s-jpeg | all / indexed | 3 | 10.797 | 11.419 | 53.58 | 6.84 / 6.84 | 280 / 80 |
| zstd-1048576-grouped-20s-jpeg | scan / forward | 3 | 10.797 | 11.420 | 13.37 | 4.27 / 4.33 | 200 / 0 |
| zstd-1048576-grouped-20s-jpeg | scan / indexed | 3 | 0.030 | 0.293 | 2.37 | 4.11 / 4.11 | 200 / 0 |
| zstd-1048576-grouped-20s-raw | all / forward | 3 | 87.953 | 88.216 | 60.31 | 11.79 / 11.79 | 200 / 0 |
| zstd-1048576-grouped-20s-raw | all / indexed | 3 | 87.953 | 88.215 | 76.03 | 4.13 / 9.32 | 200 / 0 |
| zstd-1048576-grouped-20s-raw | scan / forward | 3 | 87.953 | 88.216 | 75.88 | 4.26 / 6.19 | 200 / 0 |
| zstd-1048576-grouped-20s-raw | scan / indexed | 3 | 0.030 | 0.293 | 1.35 | 4.11 / 4.11 | 200 / 0 |
| zstd-1048576-mixed-120s-jpeg | all / forward | 3 | 53.746 | 68.551 | 209.77 | 6.63 / 9.07 | 1680 / 480 |
| zstd-1048576-mixed-120s-jpeg | all / indexed | 3 | 147.069 | 187.768 | 262.32 | 9.67 / 11.33 | 1680 / 480 |
| zstd-1048576-mixed-120s-jpeg | scan / forward | 3 | 53.746 | 68.551 | 37.20 | 6.66 / 8.85 | 1200 / 0 |
| zstd-1048576-mixed-120s-jpeg | scan / indexed | 3 | 147.069 | 187.768 | 112.13 | 6.87 / 7.85 | 1200 / 0 |
| zstd-1048576-mixed-20s-jpeg | all / forward | 3 | 9.009 | 11.420 | 44.25 | 5.76 / 8.96 | 280 / 80 |
| zstd-1048576-mixed-20s-jpeg | all / indexed | 3 | 23.394 | 29.726 | 49.37 | 10.85 / 10.85 | 280 / 80 |
| zstd-1048576-mixed-20s-jpeg | scan / forward | 3 | 9.009 | 11.420 | 6.14 | 6.98 / 6.98 | 200 / 0 |
| zstd-1048576-mixed-20s-jpeg | scan / indexed | 3 | 23.394 | 29.726 | 15.87 | 5.84 / 6.87 | 200 / 0 |
| zstd-1048576-mixed-20s-jpeg | scan / repeat | 3 | 50.559 | 64.045 | 14.75 | 5.77 / 5.77 | 0 / 0 |
| zstd-1048576-mixed-20s-jpeg | scan / repeat-cached | 3 | 0.793 | 1.001 | 0.94 | 3.75 / 5.35 | 0 / 0 |
| zstd-1048576-mixed-20s-jpeg | scan / seek | 3 | 49.109 | 61.950 | 20.90 | 5.76 / 5.76 | 0 / 0 |
| zstd-1048576-mixed-20s-jpeg | scan / seek-cached | 3 | 44.266 | 55.843 | 19.79 | 5.83 / 5.83 | 0 / 0 |
| zstd-1048576-mixed-20s-raw | all / forward | 3 | 87.490 | 88.216 | 23.60 | 6.42 / 11.64 | 200 / 0 |
| zstd-1048576-mixed-20s-raw | all / indexed | 3 | 107.751 | 109.209 | 96.23 | 6.44 / 11.66 | 200 / 0 |
| zstd-1048576-mixed-20s-raw | scan / forward | 3 | 87.490 | 88.216 | 34.99 | 7.49 / 12.74 | 200 / 0 |
| zstd-1048576-mixed-20s-raw | scan / indexed | 3 | 107.751 | 109.209 | 97.25 | 6.39 / 11.76 | 200 / 0 |
| zstd-1048576-mixed-20s-scan | all / forward | 3 | 0.007 | 0.294 | 0.85 | 4.24 / 4.24 | 200 / 0 |
| zstd-1048576-mixed-20s-scan | all / indexed | 3 | 0.114 | 5.874 | 1.72 | 4.30 / 4.30 | 200 / 0 |
| zstd-1048576-mixed-20s-scan | scan / forward | 3 | 0.007 | 0.294 | 0.84 | 4.31 / 4.31 | 200 / 0 |
| zstd-1048576-mixed-20s-scan | scan / indexed | 3 | 0.114 | 5.874 | 1.80 | 4.33 / 4.33 | 200 / 0 |
| zstd-1048576-mixed-600s-jpeg | all / forward | 3 | 267.672 | 342.752 | 904.09 | 9.24 / 9.28 | 8400 / 2400 |
| zstd-1048576-mixed-600s-jpeg | all / indexed | 3 | 740.645 | 949.504 | 1217.62 | 9.50 / 11.37 | 8400 / 2400 |
| zstd-1048576-mixed-600s-jpeg | scan / forward | 3 | 267.672 | 342.752 | 165.08 | 8.19 / 9.88 | 6000 / 0 |
| zstd-1048576-mixed-600s-jpeg | scan / indexed | 3 | 740.645 | 949.504 | 490.06 | 5.50 / 7.89 | 6000 / 0 |
| zstd-65536-grouped-20s-jpeg | all / forward | 3 | 10.899 | 11.420 | 51.41 | 6.41 / 6.53 | 280 / 80 |
| zstd-65536-grouped-20s-jpeg | all / indexed | 3 | 10.899 | 11.419 | 45.58 | 6.57 / 6.57 | 280 / 80 |
| zstd-65536-grouped-20s-jpeg | scan / forward | 3 | 10.899 | 11.420 | 11.46 | 4.20 / 4.20 | 200 / 0 |
| zstd-65536-grouped-20s-jpeg | scan / indexed | 3 | 0.037 | 0.293 | 2.64 | 4.12 / 4.12 | 200 / 0 |
| zstd-65536-mixed-20s-jpeg | all / forward | 3 | 11.023 | 11.420 | 46.30 | 6.44 / 6.57 | 280 / 80 |
| zstd-65536-mixed-20s-jpeg | all / indexed | 3 | 12.611 | 13.066 | 53.09 | 6.54 / 6.54 | 280 / 80 |
| zstd-65536-mixed-20s-jpeg | scan / forward | 3 | 11.023 | 11.420 | 12.10 | 4.25 / 4.81 | 200 / 0 |
| zstd-65536-mixed-20s-jpeg | scan / indexed | 3 | 9.938 | 10.313 | 8.97 | 4.59 / 4.86 | 200 / 0 |
| zstd-65536-mixed-20s-jpeg | scan / repeat | 3 | 5.454 | 5.602 | 6.35 | 4.06 / 4.06 | 0 / 0 |
| zstd-65536-mixed-20s-jpeg | scan / repeat-cached | 3 | 0.101 | 0.088 | 0.37 | 4.04 / 4.04 | 0 / 0 |
| zstd-65536-mixed-20s-jpeg | scan / seek | 3 | 5.328 | 5.520 | 5.76 | 3.95 / 3.95 | 0 / 0 |
| zstd-65536-mixed-20s-jpeg | scan / seek-cached | 3 | 5.328 | 5.520 | 6.59 | 3.89 / 3.89 | 0 / 0 |
| zstd-8388608-grouped-20s-jpeg | all / forward | 3 | 10.797 | 11.420 | 47.19 | 6.30 / 6.55 | 280 / 80 |
| zstd-8388608-grouped-20s-jpeg | all / indexed | 3 | 10.797 | 11.419 | 52.43 | 6.86 / 6.89 | 280 / 80 |
| zstd-8388608-grouped-20s-jpeg | scan / forward | 3 | 10.797 | 11.420 | 13.42 | 4.21 / 4.29 | 200 / 0 |
| zstd-8388608-grouped-20s-jpeg | scan / indexed | 3 | 0.030 | 0.293 | 0.97 | 4.12 / 4.12 | 200 / 0 |
| zstd-8388608-mixed-20s-jpeg | all / forward | 3 | 2.045 | 11.420 | 42.50 | 9.86 / 17.72 | 280 / 80 |
| zstd-8388608-mixed-20s-jpeg | all / indexed | 3 | 22.257 | 140.742 | 65.41 | 16.42 / 30.35 | 280 / 80 |
| zstd-8388608-mixed-20s-jpeg | scan / forward | 3 | 2.045 | 11.420 | 5.65 | 8.29 / 16.20 | 200 / 0 |
| zstd-8388608-mixed-20s-jpeg | scan / indexed | 3 | 22.257 | 140.742 | 20.21 | 16.59 / 16.59 | 200 / 0 |
| zstd-8388608-mixed-20s-jpeg | scan / repeat | 3 | 71.495 | 513.566 | 53.55 | 13.14 / 13.14 | 0 / 0 |
| zstd-8388608-mixed-20s-jpeg | scan / repeat-cached | 3 | 1.119 | 8.024 | 3.69 | 3.93 / 12.84 | 0 / 0 |
| zstd-8388608-mixed-20s-jpeg | scan / seek | 3 | 67.138 | 402.479 | 44.29 | 13.16 / 13.16 | 0 / 0 |
| zstd-8388608-mixed-20s-jpeg | scan / seek-cached | 3 | 27.621 | 151.860 | 21.38 | 13.19 / 13.19 | 0 / 0 |

## Seek read primitives

Percentiles are per-run, then medians across runs; the first cache-fill request is included.

| Case | Path | n seeks/run | p50 ms | p95 ms | p99 ms | Expanded MiB/run |
|---|---|---:|---:|---:|---:|---:|
| lz4-1048576-mixed-20s-jpeg | repeat | 64 | 0.504 | 0.956 | 1.910 | 64.045 |
| lz4-1048576-mixed-20s-jpeg | repeat-cached | 64 | 0.003 | 0.005 | 2.251 | 1.001 |
| lz4-1048576-mixed-20s-jpeg | seek | 64 | 0.932 | 1.093 | 2.412 | 61.950 |
| lz4-1048576-mixed-20s-jpeg | seek-cached | 64 | 0.995 | 1.223 | 2.497 | 55.843 |
| lz4-65536-mixed-20s-jpeg | repeat | 64 | 0.066 | 0.078 | 0.176 | 5.602 |
| lz4-65536-mixed-20s-jpeg | repeat-cached | 64 | 0.003 | 0.003 | 0.179 | 0.088 |
| lz4-65536-mixed-20s-jpeg | seek | 64 | 0.086 | 0.271 | 0.366 | 5.520 |
| lz4-65536-mixed-20s-jpeg | seek-cached | 64 | 0.091 | 0.140 | 0.389 | 5.520 |
| lz4-8388608-mixed-20s-jpeg | repeat | 64 | 8.878 | 9.052 | 11.139 | 513.566 |
| lz4-8388608-mixed-20s-jpeg | repeat-cached | 64 | 0.003 | 0.003 | 9.091 | 8.024 |
| lz4-8388608-mixed-20s-jpeg | seek | 64 | 7.272 | 9.029 | 11.478 | 402.479 |
| lz4-8388608-mixed-20s-jpeg | seek-cached | 64 | 0.002 | 7.388 | 8.999 | 151.860 |
| none-1048576-mixed-20s-jpeg | repeat | 64 | 0.086 | 0.145 | 0.609 | 64.045 |
| none-1048576-mixed-20s-jpeg | repeat-cached | 64 | 0.003 | 0.003 | 0.456 | 1.001 |
| none-1048576-mixed-20s-jpeg | seek | 64 | 0.126 | 0.213 | 0.374 | 61.950 |
| none-1048576-mixed-20s-jpeg | seek-cached | 64 | 0.131 | 0.351 | 0.819 | 55.843 |
| none-65536-mixed-20s-jpeg | repeat | 64 | 0.013 | 0.015 | 0.051 | 5.602 |
| none-65536-mixed-20s-jpeg | repeat-cached | 64 | 0.003 | 0.003 | 0.056 | 0.088 |
| none-65536-mixed-20s-jpeg | seek | 64 | 0.019 | 0.027 | 0.055 | 5.520 |
| none-65536-mixed-20s-jpeg | seek-cached | 64 | 0.025 | 0.257 | 0.310 | 5.520 |
| none-8388608-mixed-20s-jpeg | repeat | 64 | 0.801 | 1.111 | 3.138 | 513.566 |
| none-8388608-mixed-20s-jpeg | repeat-cached | 64 | 0.003 | 0.003 | 3.116 | 8.024 |
| none-8388608-mixed-20s-jpeg | seek | 64 | 0.778 | 1.299 | 3.018 | 402.479 |
| none-8388608-mixed-20s-jpeg | seek-cached | 64 | 0.003 | 1.482 | 3.156 | 151.860 |
| zstd-1048576-mixed-20s-jpeg | repeat | 64 | 0.215 | 0.224 | 0.760 | 64.045 |
| zstd-1048576-mixed-20s-jpeg | repeat-cached | 64 | 0.003 | 0.003 | 0.704 | 1.001 |
| zstd-1048576-mixed-20s-jpeg | seek | 64 | 0.316 | 0.418 | 0.840 | 61.950 |
| zstd-1048576-mixed-20s-jpeg | seek-cached | 64 | 0.318 | 0.422 | 0.973 | 55.843 |
| zstd-65536-mixed-20s-jpeg | repeat | 64 | 0.093 | 0.097 | 0.160 | 5.602 |
| zstd-65536-mixed-20s-jpeg | repeat-cached | 64 | 0.003 | 0.003 | 0.162 | 0.088 |
| zstd-65536-mixed-20s-jpeg | seek | 64 | 0.095 | 0.104 | 0.162 | 5.520 |
| zstd-65536-mixed-20s-jpeg | seek-cached | 64 | 0.103 | 0.160 | 0.257 | 5.520 |
| zstd-8388608-mixed-20s-jpeg | repeat | 64 | 0.698 | 1.599 | 3.429 | 513.566 |
| zstd-8388608-mixed-20s-jpeg | repeat-cached | 64 | 0.003 | 0.003 | 3.307 | 8.024 |
| zstd-8388608-mixed-20s-jpeg | seek | 64 | 0.665 | 1.293 | 3.002 | 402.479 |
| zstd-8388608-mixed-20s-jpeg | seek-cached | 64 | 0.003 | 1.279 | 3.140 | 151.860 |
