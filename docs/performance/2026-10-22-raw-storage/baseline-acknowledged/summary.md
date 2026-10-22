# Raw storage比較の集計

計画は各条件3反復。実際の成功/試行数は表を参照。数値は成功した反復の中央値で、3/3未満は完全な比較として扱わない。

| case | app | 成功/試行 | focus Hz | 最低 Hz | timeline frame/s | wall位相P95 ms | CPU % | PSS MiB | 起動〜ready ms |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| raw-zstd-256m-camera-1 | native | 3/3 | 7.38 | 7.38 | 9.88 | 458.14 | 38.61 | 985.44 | 867.02 |
| raw-zstd-256m-camera-4 | native | 3/3 | 7.88 | 7.21 | 9.73 | 463.16 | 45.93 | 983.25 | 864.97 |

PSSは再生中のsample最大。timeline frame/sは画像のlog timeの進み。wall位相は再生開始を基準にした差であり、実際のcursorと画像間の年齢ではない。起動〜readyはUI自動設定・検査待ち込み。失敗試行・予備試行・計測側修正前の試行もmanifestへ保存する。
