# Wasmブランチのmain統合（2026-11-04）

`main d3a851f` と `feature/scene-visualization-extension 13fd345` を統合。
WorkspaceBindingsの実行時設定と、任意のScene Extension / PluginPanelを両方維持した。
プラグインは既定で無効。既存のNative/Webを使うためにWasmプラグインを用意する必要はない。

## 性能の根拠

統合後のapps/crates/toolの全Rust source hashは、[前回の統合候補](../wasm-default-merge/candidate-suite.json)の`product_sources`とすべて一致。
[修正版計測器の18試行](../wasm-seek-timing/README.md)はその候補と同じ成果物を検証しており、既存の回帰閾値内だった。Raw Seekはmain 70.65 ms、候補73.30 ms。
この統合で製品Rustコードの追加変更はしていないため、同じ性能計測の再実行はしていない。

## 統合時の変更

- NativeのCLI、Workspace設定、Sessionテスト、Docker checkの競合を両方の機能を残して解消。
- 性能ツールの全単体テストをDockerでも実行したところ、時刻計算のテストが画像保存用Pillowをimportする問題を検出。Desktop用の3 scriptでPillowを実際の画像処理時にimportするよう変更し、標準ライブラリのみのテストを維持。
- 過去の統合保留・計測誤差訂正は各日付のレポートへ残す。

## 検証

`docker compose -p mcap_player run --rm dev bash docker/check.sh` で以下を検証する。

- storage / performance toolの単体テスト、storage policy、fmt、workspace Clippy / tests。
- プラグイン有効時のClippy、Native tests、独立ビルドしたWasm guestの実行。
- Path / Detection3DArray / RobotStatus / 複数入力・可変長配列のfixture統合テスト。
- 実行時WorkspaceBindings、Seek、Camera再選択。
- WASM check、Web release build、ROS Live tests。

検証ログは作業環境の `target/wasm-merge-verification/main-integration-20261104.log` に保存。
