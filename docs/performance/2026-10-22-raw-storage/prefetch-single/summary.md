# Raw storage比較の集計

計画は各条件3反復。実際の成功/試行数は表を参照。数値は成功した反復の中央値で、3/3未満は完全な比較として扱わない。

| case | app | 成功/試行 | focus Hz | 最低 Hz | timeline frame/s | wall位相P95 ms | CPU % | PSS MiB | 起動〜ready ms |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| raw-prefetch-lz4-single-4s-camera-4 | native | 3/3 | 9.47 | 9.47 | 9.47 | 161.34 | 22.97 | 1169.40 | 1265.80 |
| raw-prefetch-none-single-4s-camera-4 | native | 2/3 | 9.48 | 9.48 | 9.48 | 160.39 | 25.30 | 1170.68 | 591.40 |
| raw-prefetch-zstd-8m-4s-camera-4 | native | 3/3 | 9.47 | 9.47 | 9.47 | 161.43 | 35.93 | 325.75 | 260.76 |
| raw-prefetch-zstd-single-4s-camera-1 | native | 2/3 | 9.64 | 9.64 | 9.64 | 135.04 | 10.81 | 1179.33 | 1415.28 |
| raw-prefetch-zstd-single-4s-camera-4 | native | 3/3 | 9.47 | 9.47 | 9.47 | 159.57 | 24.30 | 1191.73 | 1412.29 |

PSSは再生中のsample最大。timeline frame/sは画像のlog timeの進み。wall位相は再生開始を基準にした差であり、実際のcursorと画像間の年齢ではない。起動〜readyはUI自動設定・検査待ち込み。失敗試行・予備試行・計測側修正前の試行もmanifestへ保存する。
