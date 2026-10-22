# Seek再訪の集計

各条件3プロセス。後半の操作列を再訪として集計し、各プロセスの中央値／nearest-rank P95の中央値を示す。

| phase | pattern | 成功/試行 | 再訪中央値 ms | 再訪P95 ms | 操作全体のsample最大PSS MiB |
|---|---|---:|---:|---:|---:|
| after | alternating | 3/3 | 86.454 | 96.238 | 745.8 |
| after | same | 3/3 | 83.903 | 86.095 | 489.9 |
| after | scrub | 3/3 | 82.669 | 96.327 | 293.4 |
| after | single | 3/3 | 86.276 | 95.978 | 1172.9 |
| before | alternating | 3/3 | 430.462 | 438.769 | 995.4 |
| before | same | 3/3 | 418.179 | 427.539 | 981.0 |
| before | scrub | 3/3 | 100.281 | 108.341 | 269.9 |
| before | single | 3/3 | 1296.088 | 1308.264 | 3041.8 |
| lichtblick | scrub | 3/3 | 59.918 | 78.130 | 1063.6 |
| physical-after | alternating | 3/3 | 0.005 | 0.005 | — |
| physical-after | same | 3/3 | 0.005 | 0.005 | — |
| physical-after | scrub | 3/3 | 0.003 | 0.003 | — |
| physical-after | single | 3/3 | 0.008 | 0.008 | — |
| physical-before | alternating | 3/3 | 344.804 | 356.780 | — |
| physical-before | same | 3/3 | 331.150 | 331.955 | — |
| physical-before | scrub | 3/3 | 9.619 | 11.747 | — |
| physical-before | single | 3/3 | 1189.472 | 1196.476 | — |
