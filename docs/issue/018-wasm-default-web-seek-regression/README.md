# Wasm統合候補でプラグイン未設定のWeb Raw Seekが遅くなる

- Priority: P1 / Wasm branchをmainへ統合する前のgate
- 状態: 未対応、原因未特定。mainへのmergeは撤回済み。
- 対象: `feature/scene-visualization-extension` と現mainの統合候補。通常ビルド、プラグイン未設定。

## 課題と確認したこと

[測定記録](../../verification/wasm-default-merge/README.md)のWeb FHD rgb8 / 30 Hzで、Seekからcamera canvas反映までの中央値が最初92→115 ms、交互の追加計測でも88→120 msになった。
追加計測の+31 ms / +35%は既存の許容閾値を超える。
通常Native/Webのbuild・機能テストは通り、Wasm VMは通常依存に含まれず、NativeのI/O/ROS/JPEG decode数も増えていない。
「プラグインを実行しているから遅い」とはまだ言えない。

Web 512 MiB Chunkの背景最低更新2.30→2.03 Hz、4K時PSS +31 MiBも観測。これらは既存閾値内だが次回も併記する。
Native処理時間・再訪Seekは今回の代表ケースで明確な退行なし。

## 調査候補（原因とは未確定）

通常buildにも入る変更から切り分ける。

1. shared CDR / Path materialization / FeatureRuntime restoreのCPU時間・allocation。
2. Scene Overlayのrevision / needs_render / 空bufferによる更新と、Scene非表示時の描画。
3. Web loader完了→restore commit→画像job→canvas反映の待ち。Rust処理とevent-loop / RAF待ちを分ける。
4. Wasm code量・最適化差、および試行ごとのGPU/ブラウザの揺れ。前後を交互に測る。

## 対応方針

- mainへは入れず、このbranchで変更群を切り分けて計測する。依存追加を理由に全体を再設計しない。
- 最初に同じfixture・tool・表示サイズの交互比較で再現し、段階別時間を取ってから該当箇所を修正。
- 原因と改善量を記録し、通常buildのCamera・PointCloud・Seekと無効時の追加処理ゼロを再確認する。
- 計測閾値を緩めたり、都合のよい試行だけを採用したりしない。
- mainのWorkspaceBindings対応を落とさずに次の統合候補を作る。今回の競合解消はsource snapshot / ローカルpatchへ退避済み。

## 完了条件

- Raw Seekの遅延が既存基準内に戻ることを交互比較で確認し、最初の失敗結果と合わせて報告する。
- Native/Webの既定build、必要Stream、decode数、追加Queryなし、点群・カメラ・transactional Seekを維持。
- 背景最低更新とメモリも併記し、改善のために他の指標を悪化させていないか確認する。
- 未解決ならmainへの統合を保留し続ける。
