# Plot queryをrecording sizeに比例しないbounded architectureへ変える

- Priority: P1
- 規模: M-L
- 状態: 完了

## 背景

Plotはcontinuous feature stateと分離されていますが、現在はwhole MCAP scanでspeed/yaw-rateの
full-resolution `Vec<SignalSample>`を作り、`LoadedSignal.samples`として常駐させます。displayだけを
min/max downsampleしてもexact RAMはrecording durationに比例します。

progress時には両signalのsample全量を`to_vec()`しており、長時間recordingでは累積copy量も増え続けます。

## 目的

100GB〜multi-TB recordingでもPlotを開いただけでfull-resolution historyを常駐させないことです。
I/Oとdeterministic reductionの分離はこの目的の手段です。

## 解決方針

1. Plot overview
   - recording time rangeをfixed bucketへ分けるstreaming min/max accumulatorを使う。
   - stateはsignalごとに`O(max_display_points)`とする。
   - progress snapshotもbounded envelopeだけをcloneする。
2. current value
   - playback cursorではcontinuous OdometryControllerのexact current stateを使う。
   - preview cursorにexact値がない場合、overview sampleをexact値として表示しない。
3. visible-range detail
   - 必要になった時だけbounded Exact Range Queryで取得する独立拡張点とする。
   - 今回generic QueryManagerやfull-history cacheは作らない。
4. Query I/O
   - worker/file/mmap/generation/progressはNative adapterが所有する。
   - reducerはordered decoded Odometryとrecording range/max pointsだけを受ける。

## 契約テスト

- input count Nを増やしても各overviewがmax pointsを超えない。
- first/last bucketと各bucket min/maxの時間順を維持する。
- progress publicationのpayloadもmax points以下である。
- speed/yaw-rateは一scanで生成する。
- source generation replacement後に古いresultを表示しない。
- current valueはcontinuous controller state由来で、preview中に誤ったexact値を表示しない。

## Memory complexity

- reducer working set: `O(max_display_points)` per signal
- published overview: `O(max_display_points)` per signal
- full-resolution exact history: `O(1)`（常駐しない）
- MCAP mapping: OS-backed source I/Oであり、decoded sample heapへ展開しない

## 完了条件

- `LoadedSignal`にrecording全体のexact sample Vecがない。
- progressで過去全sampleをcloneしない。
- pure accumulatorのbounded invariantがtestで固定される。
- Plot/Preview/continuous runtimeの経路分離を維持する。

## 実装結果

- full-resolution `LoadedSignal.samples`を削除し、`SignalOverviewReducer`のfixed time bucket min/maxだけを
  常駐させた。保持点数は各signalで`max_display_points`以下になる。
- progress publicationはbounded reducer snapshotだけをcloneし、過去全sampleの累積cloneを削除した。
  配送もcapacity 1のchannelで途中結果をcoalesceし、slow UI時のsnapshot backlogを防ぐ。
- current valueはFeatureRuntimeのOdometry current stateから供給し、preview中はexact currentを供給しない。
- query worker/mmap/generation cancellationはNative adapter、deterministic reductionはviewer-coreに分離した。
- overview生成のsource scan I/Oはrecording message数に比例するが、decoded heap working setは
  `O(max_display_points)`、full-resolution exact常駐は`O(1)`である。
- visible-range exact detailは既存Exact Range Queryを使う独立した将来拡張点として残し、generic managerは
  導入していない。
