**訂正: CPU・PSSの比較は無効です。** 当時のprocess_sampleはLinuxのmain threadのchildrenだけを辿り、Electronのrenderer / utilityを取りこぼしていました。保存された値は全アプリの資源量ではありません。以下のCPU・PSSに基づく比較と結論を撤回します。元データは監査用に残し、[修正した計測](../2026-10-20-desktop-workloads/README.md)で取り直します。画素観測と入力・GPU選択の記録とは区別してください。

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
