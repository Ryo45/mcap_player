# Query Service / Panel 検証（2026-11-04）

[使い方・設計と制限](../../query_panel.md)。作業branchは`feature/query-panel`、Wasm統合済みmainを基点にした。

## 実画面

同じ独自ROS型の2 topicを選び、固定範囲で64 rowのTableと2系列Plotを表示した。
通常Native release（features指定なし）。再生カーソルは動き続け、Query範囲は変わらない。

![Query Table](table.png)

![Query Plot](plot.png)

Next pageでは最初のlog timeが`1700000003200000000 ns`になり、page 1の続きから表示された。
各Plotは300入力、decode error 0。Inspectorの遅延展開は既存描画を再利用し、Query専用の別デコーダは作っていない。

[撮影情報](capture.json)にbinary / fixture / image hashを保存。
既存デスクトップでの自動入力がfocus拒否となったため、画面ロックを解除せず専用Xephyrで操作した。
Vulkanはllvmpipeで、GPU性能を測った試験ではない。NativeのNVIDIA起動自体も確認したが、その画面上でQuery操作が完了したとは主張しない。
画像は読込前cancel guard・大きいmessageの詳細cache制限・topic一覧の表示行制限を含む最終releaseで撮影した。最終変更後の対象testsとClippyも成功した。

## 自動確認

- `docker/check.sh` 全体が成功：Python tests、storage policy、fmt、workspace Clippy / tests、features有効時のClippyとNative tests、独立ビルドしたWasm guest、Path/Detection/PluginPanelのfixture、Bindings/Seek/Camera、WASM check、Trunk release、ROS Live。
- 最終変更後の `cargo test -p viewer-native query`、`cargo test -p viewer-mcap --lib`、workspace Clippy、関連WASM checkも成功。
- Queryに追加したNative regression 8件：複数topic、固定範囲・exclusive end、同一timeのpage継続、worker再利用・世代無効化・Source交換、Live非対応、budget・field error・cancel、UI条件変更、時刻差分、実録画のroute / cursor / Seek維持（複数項目を同じtestで検証）。
- coreではsigned ns全域のoverview bucket分割、MCAPでは巨大Chunkの索引work見積りとoverflow、取消済み要求がChunkを展開しないことを検証。
- Tableの64 row / 4 MiB詳細cache、Plot最大2048点、射影budget、256 MiB input+expansion budgetを維持。大きいmessageはmetadata・選択fieldを返し、予算外の詳細copyは保持せず理由を表示。不正field・work超過・cancelは明示エラー。
- 実録画で通常再生の対象外のカメラtopicをQueryした後も、SessionPlanの選択とcursorが不変。続くSeekで既存CameraとScan stateを正常復元。
- 新規UI/Serviceはmainへの再統合や既存Loader全体の移行を行っていない。

ログは作業環境の`target/query-panel/full-checks.log`、`final-native.log`、`final-safety.log`、`final-ui-check.log`に保存。
今回Query負荷と通常再生のFPSの比較はしていない。状態の独立性は確認したが、QueryのI/O・CPUコストがゼロという意味ではない。

## 残る制限

QueryはIndexed Recording専用。Tableの元範囲全体を保守的に予算検査するため、広い範囲・巨大単一Chunkは初期実装の予算で拒否し得る。
複数topicのPlotは既存のstream単位APIを使うので、混在Chunkの再読があり得る。query条件・UIの永続化、topic間のjoin / interpolation、XY、派生信号、plugin ABI、Web UIは未実装。
既存Inspectionと機能が重なるため、`--layout query`は固定範囲Queryの検証用presetとして置き、将来の入口統合とServiceの独立性を分けて扱う。
