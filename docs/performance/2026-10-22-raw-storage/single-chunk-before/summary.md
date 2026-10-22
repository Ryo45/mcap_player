# Raw storage比較の集計

計画は各条件3反復。実際の成功/試行数は表を参照。数値は成功した反復の中央値で、3/3未満は完全な比較として扱わない。

| case | app | 成功/試行 | focus Hz | 最低 Hz | timeline frame/s | wall位相P95 ms | CPU % | PSS MiB | 起動〜ready ms |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| raw-lz4-single-4s-camera-1 | lichtblick | 0/1 | — | — | — | — | — | — | — |
| raw-lz4-single-4s-camera-1 | native | 0/1 | — | — | — | — | — | — | — |
| raw-lz4-single-4s-camera-4 | lichtblick | 0/1 | — | — | — | — | — | — | — |
| raw-lz4-single-4s-camera-4 | native | 0/1 | — | — | — | — | — | — | — |
| raw-none-single-4s-camera-1 | lichtblick | 2/2 | 9.82 | 9.82 | 9.82 | 107.63 | 74.78 | 2970.40 | 2439.18 |
| raw-none-single-4s-camera-1 | native | 2/2 | 9.47 | 9.47 | 9.47 | 159.74 | 11.98 | 2081.66 | 914.59 |
| raw-none-single-4s-camera-4 | lichtblick | 0/2 | — | — | — | — | — | — | — |
| raw-none-single-4s-camera-4 | native | 2/2 | 9.62 | 9.46 | 9.46 | 165.46 | 27.26 | 2092.42 | 988.54 |
| raw-zstd-8m-4s-camera-1 | lichtblick | 2/2 | 9.81 | 9.81 | 9.81 | 109.07 | 78.62 | 1160.42 | 2087.26 |
| raw-zstd-8m-4s-camera-1 | native | 2/2 | 9.48 | 9.48 | 9.48 | 159.22 | 22.47 | 269.10 | 211.43 |
| raw-zstd-8m-4s-camera-4 | lichtblick | 2/2 | 9.82 | 9.82 | 9.82 | 111.38 | 236.72 | 2430.25 | 2702.55 |
| raw-zstd-8m-4s-camera-4 | native | 2/2 | 9.48 | 9.48 | 9.48 | 160.53 | 35.28 | 287.49 | 211.21 |
| raw-zstd-single-4s-camera-1 | lichtblick | 0/2 | — | — | — | — | — | — | — |
| raw-zstd-single-4s-camera-1 | native | 1/2 | 9.47 | 9.47 | 9.47 | 160.20 | 11.31 | 2112.20 | 2672.30 |
| raw-zstd-single-4s-camera-4 | lichtblick | 0/1 | — | — | — | — | — | — | — |
| raw-zstd-single-4s-camera-4 | native | 1/1 | 9.47 | 9.47 | 9.47 | 161.87 | 21.97 | 2110.51 | 2767.47 |

PSSは再生中のsample最大。timeline frame/sは画像のlog timeの進み。wall位相は再生開始を基準にした差であり、実際のcursorと画像間の年齢ではない。起動〜readyはUI自動設定・検査待ち込み。失敗試行・予備試行・計測側修正前の試行もmanifestへ保存する。
