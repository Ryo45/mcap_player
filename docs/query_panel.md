# Native Query Panel / Query Service

Indexed MCAPに対して、連続再生から独立した固定log-time範囲を問い合わせる。既存Inspectionのcursor直前値と記録全体Plotに対して、範囲内メッセージのTableと固定範囲Plotを追加する位置づけ。今回の`--layout query`はその検証用presetで、UI入口の統合と取得Serviceの責務分離は独立して扱う。

```sh
cargo run --release -p viewer-native -- --mcap /path/to/recording.mcap --layout query
```

TopicsとQueryの配置で開く。開始・終了を**整数の絶対log time（ns）**で指定する。
範囲は `[start, end)`。初期値は記録全体で、再生・Seekによる自動追従はしない。
Start at cursor / End after cursorは押した時点の値を一度だけコピーする。

最大8 streamをチェックし、各入力のフィールドをカンマ区切りで指定する。
同じtopicに複数channelがある場合はschemaとstream番号で区別する。
Tableは空欄でもメタデータを取得でき、時刻を選ぶと既存Inspectorで遅延展開する。
数値・文字列・boolの選択には `twist.twist.linear.x` 等のROS field pathを使う。Tableは `objects[].id` 等の有限な配列射影にも対応。
Plotは各入力にscalar numeric fieldを指定する。例はOdometryの`twist.twist.linear.x, twist.twist.angular.z`。
Run queryで実行し、Idle / Loading / Error / Completedを表示する。条件を編集すると古い結果を消し、次のRunまで再実行しない。
Cancel / clearで実行・結果を破棄する。Source交換でも選択・結果・ページ履歴を破棄する。

TableはNext / Previousでページを移動でき、直近16個の継続位置だけを保持する。Run queryで先頭へ戻る。
Plotはmin/max bucket overviewで、全decoded履歴を保持しない。mainの再生位置はマーカーとして表示する。
query側のズームや入力・ページ操作はPlaybackCommandを出さず、通常再生・Seekを変更しない。
ROS 2 Liveは非対応と明示し、過去履歴のリングバッファは作らない。

## 小さい独自ROSメッセージのデモ

```sh
mkdir -p target/query-demo
cargo run -p ros-fixture --example query_fixture -- target/query-demo/recording.mcap
cargo run --release -p viewer-native -- --mcap target/query-demo/recording.mcap --layout query
```

fixtureは新規pathだけに書き、既存ファイルを上書きしない。
独自型`example_msgs/msg/RobotMetric`を2 topic（`/robot/speed`、`/robot/temperature`）へ各300件記録する。
Tableでは`value, status`、Plotでは両topicの`value`を選んでRunする。ROSインストールは不要。

## 責務と既存コードの再利用

```text
QueryPanel (条件・表示・ページ履歴)
   -> QueryRequest
QueryService (source所有・1 worker・generation・実行状態)
   -> range / numeric queries on independent McapSource forks
   -> QueryResult (bounded table rows / numeric overview)
QueryPanel (Table / Plot / Inspector)
```

- `apps/viewer-native/src/query_service.rs` はegui、Workspace layout、FeatureRuntime、Playback cursorに依存しない。同期`execute`も単独で使える。非同期Serviceは入力と同じ要求型を使い、他panelや将来のNative plugin hostからも利用可能。
- PanelがServiceを所有し、Workspace hostが新しいrecordingをbindする。Source交換では同じworkerを再利用して世代を進める。要求は最新のpending 1個、result 1個、実行中1個だけ。変更・取消・破棄時に古い世代を無効化する。
- readerは既存`McapSource::fork_reader`。再生の選択・位置・キャッシュを操作せず、query対象をSessionPlanへ追加しない。必要な索引がない場合も既存エラーに従い、全走査にfallbackしない。
- Tableは既存RangeQueryのordered traversalへ継続・取消を追加。`(log_time, stream, same-key ordinal)`の継続位置なので、同じtimestamp・sequenceのメッセージも欠落させない。既存の非paged queryも同じループを使う。
- Tableの選択fieldは既存RosMessageSchema / SelectedFieldExtractorをstreamごとに一度compileし、pageの全rowで再利用する。schema解析・extractorの跨要求cacheはまだ追加していない。
- 詳細は既存InspectorSchema / InspectorMessage / field pagingと描画を共用。InspectorMessageが小さいpayload copyを持つので、大きなChunk backingを画面の1 rowでpinしない。全fieldを巨大JSONへ変換しない。
- Plotは既存`query_numeric_overviews_cancellable`をそのまま使い、同じstreamの複数fieldを1 scanにまとめる。既存SignalOverviewReducerは任意のsigned時刻範囲を扱えるよう、差分をi128で計算するよう修正した。
- TableとPlotは同じService・検証・Source lifecycle・予算を使う。exact pageとoverviewでは意味が異なるため、下位の既存query APIは分けて維持する。

## 上限と実用上の制限

- 同じWorkspaceではQuery panelは1個まで。重複は既存のsingleton placeholder扱い。
- 最大8 stream / 合計8 field。Table pageは最大64 row。保持するInspector payload copyはpage合計4 MiBまで。大きなmessageでもmetadata・選択fieldは取得でき、詳細copyが予算を超えるrowは理由を表示してcopyを保持しない。Chunk backingはpageに残さない。
- field射影は1 row全体で128 value、array長64、各text 1 KiB、text総計2 KiB。過大・不正fieldはrowのerrorとして表示する。詳細Inspectorの既存ページ・走査上限も維持。
- Plotは1 field最大2048点。値は描画用f64。Tableの整数値・log-timeキーは整数精度を維持する。
- 索引から見積もる **compressed input + expanded dataの合計256 MiB** を1要求のwork上限にする。process RSS上限ではなく、既存Chunk parserのmetadata等は別途かかる。
- Tableも元の範囲全体で保守的に予算検査するため、最初のpageしか必要なくても広い範囲は拒否し得る。小さい範囲へ絞る。単一巨大Chunkが予算を超える場合は、範囲を狭めても開けず、rechunk等が必要。通常再生側の巨大Chunk許容は変更していない。
- Tableの継続時にskipする同時刻prefixを含め、1要求最大100,000 messageを走査する。極端に多い同時刻メッセージの後半にはこの上限で到達できない場合がある。
- Plotはstream単位の既存APIのため、複数topicが同じChunkへ混在すると展開を繰り返し得る。work予算はその再読分も計上する。全topicを1回のreadで処理する保証はない。
- cancelは既存reader同様に協調的。現在のfile read / Chunk展開自体は中断しない。Source交換でUIをjoin待ちにせず、panel破棄では取消後にworkerをjoinする。
- QueryにもI/O・CPUが必要で、通常再生とのresource競合はあり得る。再生状態の独立性と無負荷を混同しない。

## 構造上の見通し

他のquery利用者にはNativeの要求/結果APIを再利用できるが、汎用plugin ABIや別crateはまだ作っていない。
将来exactな複数topicの時刻整列・派生信号を作るなら、query実行側へ明示的なjoin/interpolation/変換方針とboundedな出力を追加する。
既存overviewは情報を集約しているため、exactな整列の入力には無条件に使わず、既存numeric detail等のbounded exact取得を拡張する必要がある。
XY Plotはその実行結果を表示する新しいpresentationにする。FeatureRuntimeへ解析履歴を持ち込む必要はない。

既存Inspector/Plot/Window query loaderとgeneration・mailbox処理に類似がある。
今回は固定範囲の明示実行、cursor追従、recent-window更新という要求の意味が異なるため、全loaderの強制移行やgeneric worker frameworkは避けた。
現時点の具体的な改善余地は、multi-topic Plotの重複Chunk readと、同じschemaのcompileを要求間で再利用する点。
それぞれ計測で問題になった時点で既存query executorを広げればよく、SQL・Arrow・全履歴storeは不要。

検証結果は[Query Panelの検証](verification/query-panel/README.md)を参照。
