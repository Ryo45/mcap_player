# Raw storage比較の集計

計画は各条件3反復。実際の成功/試行数は表を参照。数値は成功した反復の中央値で、3/3未満は完全な比較として扱わない。

| case | app | 成功/試行 | focus Hz | 最低 Hz | timeline frame/s | wall位相P95 ms | CPU % | PSS MiB | 起動〜ready ms |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| raw-lz4-8m-camera-1 | lichtblick | 3/3 | 9.90 | 9.90 | 9.90 | 114.26 | 111.19 | 1397.61 | 2354.03 |
| raw-lz4-8m-camera-1 | native | 3/3 | 9.73 | 9.73 | 9.73 | 163.51 | 21.27 | 268.57 | 213.42 |
| raw-lz4-8m-camera-4 | lichtblick | 3/3 | 9.91 | 9.91 | 9.91 | 119.86 | 231.39 | 2682.01 | 2547.11 |
| raw-lz4-8m-camera-4 | native | 3/3 | 9.73 | 9.73 | 9.73 | 164.82 | 32.45 | 310.83 | 260.26 |
| raw-none-8m-camera-1 | lichtblick | 2/3 | 9.90 | 9.90 | 9.90 | 113.50 | 95.70 | 1359.72 | 2054.43 |
| raw-none-8m-camera-1 | native | 3/3 | 9.73 | 9.73 | 9.73 | 163.22 | 14.98 | 256.67 | 210.80 |
| raw-none-8m-camera-4 | lichtblick | 3/3 | 9.91 | 9.91 | 9.91 | 114.50 | 196.02 | 2563.78 | 2182.35 |
| raw-none-8m-camera-4 | native | 3/3 | 9.73 | 9.73 | 9.73 | 165.43 | 21.97 | 286.36 | 262.78 |
| raw-split-zstd-8m-camera-1 | lichtblick | 3/3 | 9.90 | 9.90 | 9.90 | 113.32 | 120.96 | 1391.66 | 2461.52 |
| raw-split-zstd-8m-camera-1 | native | 3/3 | 9.73 | 9.73 | 9.73 | 163.43 | 22.64 | 268.69 | 212.14 |
| raw-split-zstd-8m-camera-4 | lichtblick | 3/3 | 9.90 | 9.90 | 9.90 | 117.12 | 249.78 | 2676.98 | 2719.56 |
| raw-split-zstd-8m-camera-4 | native | 3/3 | 9.73 | 9.73 | 9.73 | 162.78 | 37.61 | 316.01 | 208.98 |
| raw-unchunked-camera-1 | lichtblick | 0/3 | — | — | — | — | — | — | — |
| raw-unchunked-camera-4 | lichtblick | 0/3 | — | — | — | — | — | — | — |
| raw-zstd-128m-camera-1 | lichtblick | 2/3 | 9.90 | 9.90 | 9.90 | 114.24 | 166.06 | 2448.15 | 3009.89 |
| raw-zstd-128m-camera-1 | native | 3/3 | 7.55 | 7.55 | 9.73 | 309.36 | 39.61 | 612.35 | 560.78 |
| raw-zstd-128m-camera-4 | lichtblick | 3/3 | 9.90 | 9.90 | 9.90 | 117.98 | 217.81 | 3573.73 | 2545.73 |
| raw-zstd-128m-camera-4 | native | 3/3 | 8.22 | 7.53 | 9.73 | 314.49 | 45.83 | 563.23 | 513.62 |
| raw-zstd-256m-camera-1 | lichtblick | 3/3 | 9.90 | 9.90 | 9.90 | 114.25 | 157.81 | 3810.31 | 3256.58 |
| raw-zstd-256m-camera-1 | native | 3/3 | 6.38 | 6.38 | 9.06 | 605.19 | 39.11 | 969.74 | 966.86 |
| raw-zstd-256m-camera-4 | lichtblick | 0/3 | — | — | — | — | — | — | — |
| raw-zstd-8m-camera-1 | lichtblick | 3/3 | 9.90 | 9.90 | 9.90 | 113.15 | 123.87 | 1355.93 | 2402.77 |
| raw-zstd-8m-camera-1 | native | 3/3 | 9.73 | 9.73 | 9.73 | 163.27 | 22.64 | 274.30 | 218.19 |
| raw-zstd-8m-camera-4 | lichtblick | 3/3 | 9.91 | 9.91 | 9.91 | 119.33 | 247.29 | 2683.13 | 2694.56 |
| raw-zstd-8m-camera-4 | native | 3/3 | 9.73 | 9.73 | 9.73 | 165.31 | 33.95 | 310.13 | 212.96 |

PSSは再生中のsample最大。timeline frame/sは画像のlog timeの進み。wall位相は再生開始を基準にした差であり、実際のcursorと画像間の年齢ではない。起動〜readyはUI自動設定・検査待ち込み。失敗試行・予備試行・計測側修正前の試行もmanifestへ保存する。
