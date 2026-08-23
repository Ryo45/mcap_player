# Inspector queryをsource-open pathから外す

- Priority: P2
- 規模: S-M
- 状態: 完了

## 背景・課題

実装前のNativeはsource open直後に同期RangeQueryを実行し、Inspectorの存在が最初のplayback frameを
random query I/Oでblockしていました。

## 解決案

- Inspector専用のbounded background workerを作る。
- source generation変更で古いresultを破棄する。
- max messages/max payload bytesを既存RangeQuery limitsで維持する。
- Plot用generic query frameworkとは統合しない。

## 完了条件

- source openはInspector I/O完了を待たない。
- playbackはinspection loading/failureから独立して進む。
- stale generation resultを表示しない。

## 実装結果

- `InspectorLoader`が専用のread-only mmap/McapSourceでqueryし、playback source/cursorを共有しない。
- 各requirementは既存の`maxMessages`（1..=256）と16 MiB payload budgetでboundedにした。
- source generationはatomic generationでworkerにも公開し、古いjobはrequirement境界で中断し、
  channelへ到着済みの古い結果もUI stateへcommitしない。
- Panelにはloading/ready/errorを明示し、同期`ViewerSession::inspect_topic/load_inspections` pathは削除した。
- worker pending/complete中にもplayback cursorが独立して進むscenario testを追加した。
