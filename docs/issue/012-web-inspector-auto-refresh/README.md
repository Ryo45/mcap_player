# Web Inspector をカーソルに追従させる

- 種別: 機能追加
- Priority: P2。使い勝手を改善する次の機能として推奨
- 規模: M
- 状態: 対応済み
- 調査日: 2026-10-12
- 調査対象: HEAD `5fbfac2` ＋未コミットの `viewer-mcap` 分離後の作業ツリー

## 課題

Web では topic を選んでも「Read message at cursor」を押すまで取得しない。
再生中は表示メッセージが固定され、seek 成功後も再取得にボタン操作が必要。
Native の自動追従 Inspector と操作が異なり、3D/camera とメッセージを照合する手間が残る。

手動取得は現行の明示的な仕様で、不具合として扱わない。自動化を16 ms tickから直接行うと、
I/O の多重発行、毎フレームの取り消し、schema 再コンパイルによる無駄が増えるため、先に
小さな取得スケジュールを設計する必要がある。

## 調査したこと

- [TopicBrowser](../../../apps/viewer-web/src/topics.rs#L98) は DOM、選択 topic、loader、
  pending request、decoded message、container page を一つの型で保持する。
- [request](../../../apps/viewer-web/src/topics.rs#L182) は pending がある間は新規要求を受けず、
  更新時には現在の message/pages を消去する。手動操作には合うが、これを自動更新へ流用すると
  点滅や、読み取り中に到着した最新 cursor demand の欠落につながる。
- `poll` は取得時の cursor と metadata を表示し、`InspectorSchema::for_stream` をその都度呼ぶ。
  schema-driven decode とページ取得自体は既に `viewer-core` の `InspectorMessage` を使っている。
- [Web tick](../../../apps/viewer-web/src/lib.rs#L879) は committed cursor を使い、seek 成功時に
  `clear_message` する。新要求をこの位置で組み立てれば、未確定の seek target や buffering 中の
  requested cursor を採用せずに済む。
- Native の [InspectorLoader](../../../apps/viewer-native/src/inspector_loader.rs) は実行中と
  pending demand を分け、cursor 更新を coalesce する。thread/mailbox そのものではなく、この
  starvation を避ける方針を参考にできる。
- Browser Local loader は generation token と要求予算を持ち、Remote loader は generation と
  `AbortController` を持つ。Native の同期 reader を Web に移植する必要はない。

## 解決方針

1. `TopicBrowser` から、DOM を持たない具体的な Web Inspector query state を分ける。
   選択 topic、desired cursor、実行中の一要求、最新 pending demand、完成 snapshot を持たせる。
2. 自動更新は初期案として最大250 ms間隔。実行中の通常の前進要求を毎 tick キャンセルせず、
   pending 一枠を最新 cursor で置換し、完了後に最新 demand を処理する。停止中は同じ要求を再発行しない。
3. topic/source 変更と seek 成功時は旧世代を無効化し、古い結果を採用しない。
   seek 失敗時は committed cursor の snapshot を保つ。未来の log time を持つ結果は表示しない。
4. 前進更新中は最後に完成した snapshot と取得時刻を表示し、loading を併記する。
   source/topic が変わった場合は別の topic/録画の snapshot を引き継がない。
5. 選択中 stream の compiled schema を再利用する。Remote には Native の物理 record position が
   ないので、log time/sequence/publish time だけを一意な message ID として decode cache に使わない。
   第一段階では schema reuse と要求 coalescing に限定してよい。
6. 手動 Refresh を残す。自動更新の on/off、snapshot の取得 cursor と現在 cursor の違いを明示する。

取得スケジュールは Web 固有の一つの具体型で十分。Native/Web の worker、再生状態管理、
全クエリを共通 manager に統合する作業は含めない。純粋な選択・検証処理に実際の重複があれば
別途 core へ抽出できるが、feature 実装の前提として全面共通化を要求しない。

## 完了条件・回帰確認

- topic 選択後と seek 成功後に、追加クリックなしで exact latest-before を取得する。
- Local/Remote の両方で、再生・buffering と Inspector の I/O が独立して進む。
- 遅い loader に大量の前進 demand を与えても、実行中一件＋pending一件を超えず、結果公開を飢餓状態にしない。
- 同一 paused cursor の tick を繰り返しても、余分な要求を発行しない。
- 遅れて到着した旧 source/topic/seek generation の成功・失敗結果を採用しない。
- 同時刻・複数 channel の選択、直前値不在、壊れた CDR、未知 schema、要求上限超過を明示する。
- compiled schema、decoded message、ページ状態、retained backing が要求回数に比例して増えない。
- schedule は fake clock と具体的な fake loader で決定的に検証し、実ブラウザで Local/Remote の通し確認をする。

## 調査時点で残る判断

250 msは既存 window query の更新間隔を参考にした初期案で、性能測定結果ではない。
自動更新の既定 on/off と、container 展開・ページ位置を新しい message へどこまで引き継ぐかは要決定。
Web の通常機能としての価値があるため、[011](../011-native-latest-query-bounds/README.md) と
独立して進められるが、全体の実装順としては011の小さいキャンセル修正を先に行うことを勧める。

## 実装結果

DOMを持たない `InspectorQueryState` に取得スケジュールを分離した。自動追従は既定 on、
開始間隔は250 ms。前進中の一要求を取り消さず最新 cursor を一枠へまとめ、paused で
完成済みの同じ cursor を再取得しない。手動 Refresh は間隔を待たず、実行中なら一枠にまとめる。

source/topic 変更と seek 成功は scheduler と既存 loader の generation を無効化する。
失敗した seek はその経路を通らない。更新中は前 snapshot/pages を保ち、取得 cursor・現在 cursor・
loading を併記する。新しい結果ではページを root に戻す。compiled schema は一 stream 分だけ
成功／エラーを再利用し、payload identity に基づく推測キャッシュは導入しない。

fake clock と具体的 fake loader で遅い要求・coalescing・paused reuse・世代変更・on/off・
手動更新を確認した。既存の exact/tie/不正応答テストも維持した。Native上のWebテスト、
Native/WASM Clippy、Trunk release が成功。実ChromeでLocal/Remoteの自動取得、seek後の
自動再取得、auto off＋手動Refresh、再生中の値更新、source replacement、壊れたschemaの表示、
nested CDRの展開、Scene/BEVの通し確認が成功した。
