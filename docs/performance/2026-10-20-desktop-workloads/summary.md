各条件3反復の中央値。CPUは1 core=100%。PSSはsample最大の中央値。背景最低Hzは各run内の背景カメラ最小値の中央値。

| 表示 | app | focus Hz | 背景最低 Hz | Scene変化 Hz | CPU % | PSS MiB |
|---|---|---:|---:|---:|---:|---:|
| camera-1 | native | 9.86 | — | — | 42.7 | 272.0 |
| camera-1 | lichtblick | 9.86 | — | — | 133.4 | 1435.8 |
| camera-4 | native | 9.86 | 5.01 | — | 73.2 | 300.8 |
| camera-4 | lichtblick | 9.27 | 8.51 | — | 368.2 | 2129.2 |
| camera-8 | native | 9.86 | 4.93 | — | 106.1 | 316.4 |
| camera-8 | lichtblick | 4.60 | 3.84 | — | 410.4 | 3423.8 |
| camera-focus-7 | native | 9.86 | 5.01 | — | 110.0 | 331.6 |
| camera-focus-7 | lichtblick | 4.51 | 3.84 | — | 402.1 | 3487.4 |
| raw-closed | native | — | — | — | 5.3 | 210.2 |
| raw-closed | lichtblick | — | — | — | 30.8 | 504.2 |
| raw-open | native | — | — | — | 8.3 | 210.6 |
| raw-open | lichtblick | — | — | — | 31.0 | 539.1 |
| scan-360 | native | — | — | 9.95 | 3.3 | 210.6 |
| scan-360 | lichtblick | — | — | 9.95 | 36.8 | 598.7 |
| scan-3600 | native | — | — | 9.95 | 3.6 | 217.6 |
| scan-3600 | lichtblick | — | — | 9.95 | 42.3 | 603.3 |
| scan-36000 | native | — | — | 9.95 | 5.7 | 244.2 |
| scan-36000 | lichtblick | — | — | 10.03 | 39.0 | 609.0 |
| cloud-10000 | native | — | — | 9.95 | 4.2 | 241.7 |
| cloud-10000 | lichtblick | — | — | 9.95 | 36.3 | 642.8 |
| cloud-60000 | native | — | — | 9.95 | 7.2 | 247.0 |
| cloud-60000 | lichtblick | — | — | 9.95 | 45.8 | 876.5 |

P95はrunごとのP95の中央値で、sampleをpoolしたP95ではない。

| 表示 | app | focus lag 中央値 / P95 ms | 最も遅い背景 lag P95 ms |
|---|---|---:|---:|
| camera-1 | native | 160 / 215 | — |
| camera-1 | lichtblick | 112 / 168 | — |
| camera-4 | native | 160 / 176 | 356 |
| camera-4 | lichtblick | 466 / 526 | 568 |
| camera-8 | native | 140 / 173 | 321 |
| camera-8 | lichtblick | 2490 / 3572 | 3674 |
| camera-focus-7 | native | 156 / 210 | 317 |
| camera-focus-7 | lichtblick | 2564 / 3722 | 3872 |
