# 3反復の中央値

P95はrunごとのP95の中央値。CPUは1 core=100%。PSSはsample最大値の中央値。

| case | app | image Hz | lag median / P95 ms | Scene変化 Hz | CPU % | PSS MiB |
|---|---|---:|---:|---:|---:|---:|
| default | native | 9.86 | 136 / 167 | 0.00 | 36.1 | 250.4 |
| default | lichtblick | 9.86 | 114 / 166 | 0.00 | 34.1 | 296.4 |
| scan | native | 9.86 | 155 / 167 | 9.94 | 36.8 | 262.5 |
| scan | lichtblick | 9.86 | 107 / 165 | 9.95 | 36.4 | 297.1 |
| raw | native | 9.86 | 154 / 168 | 9.94 | 40.5 | 284.9 |
| raw | lichtblick | 9.94 | 106 / 163 | 9.95 | 33.8 | 293.5 |

各反復・範囲・観測負荷・Seek検証は[summary.json](summary.json)を参照。
