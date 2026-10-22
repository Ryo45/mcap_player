# Raw storage比較の集計

計画は各条件3反復。実際の成功/試行数は表を参照。数値は成功した反復の中央値で、3/3未満は完全な比較として扱わない。

| case | app | 成功/試行 | focus Hz | 最低 Hz | timeline frame/s | wall位相P95 ms | CPU % | PSS MiB | 起動〜ready ms |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| raw-prefetch-zstd-256m-camera-1 | native | 3/3 | 9.89 | 9.89 | 9.89 | 159.51 | 45.10 | 971.47 | 565.41 |
| raw-prefetch-zstd-256m-camera-4 | native | 3/3 | 9.73 | 9.73 | 9.73 | 165.59 | 53.07 | 972.94 | 514.57 |
| raw-prefetch-zstd-8m-camera-4 | native | 3/3 | 9.73 | 9.73 | 9.73 | 165.19 | 34.78 | 323.13 | 263.74 |

PSSは再生中のsample最大。timeline frame/sは画像のlog timeの進み。wall位相は再生開始を基準にした差であり、実際のcursorと画像間の年齢ではない。起動〜readyはUI自動設定・検査待ち込み。失敗試行・予備試行・計測側修正前の試行もmanifestへ保存する。
