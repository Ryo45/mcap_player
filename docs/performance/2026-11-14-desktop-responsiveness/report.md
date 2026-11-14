# Desktop操作レスポンスの調査と第1回改善

2026-11-14。スマホ固有の最適化から離れ、Native/Webの通常操作で重複処理や待ちを調べた。今回は次の2点を変更した。単に処理を別の層へ移すための共通manager、query broker、無制限cacheは導入していない。

## 改善1：Nativeの記録オープン時に未選択トピックまで解析しない

`NativeSession`が作る`TopicTree::from_catalog`は、全ChannelのROS schemaを解析し、数値field一覧を生成していた。実際に数値fieldを使うのは、選択されたトピックのPlot追加UIと明示的なseries要求である。

`TopicTreeEntry::numeric_fields()`で初回要求時だけ解析し、結果とエラーをSource entryごとのOnceLockへ保持する形にした。UI/APIで必要なChannelだけを探索し、全体一覧はtopic/schema/countなどのmetadataだけで構築する。Web専用の別constructorはなくし、両方が同じmetadata構築を使う。標準Rust型のみで、依存や実行threadは増やしていない。Source交換では新しいentryになる。

各schemaが64個の数値fieldを持つ代理catalogで、release binary・各7回の中央値：

| トピック数 | 変更前 | 変更後 |
| --- | ---: | ---: |
| 500 | 約6ms | 約0.016ms |
| 5000 | 約50.6ms | 約0.25ms |

これは**TopicTree準備だけ**の比較で、ファイル読み込み・MCAP summary取得・ウィンドウ起動時間を含まない。選択したトピックの初回解析自体が高速化したという主張ではない。未使用の解析・field配列を作らなくなった効果である。

## 改善2：停止中のCamera切替で、配送済みの最新入力を使う

Webの`select_camera`は停止中なら毎回全Featureのrestoreを要求していた。直前のSeekで全planned cameraの正確なpredecessor（またはその時点でのメッセージ欠損）を既に保持していても、Chunkを再読み込み・再展開し、Camera/Sceneの表示cacheを再初期化していた。

既存の`FeatureRuntime::present_pending_camera()`をNativeと同様に使い、現在cursorまで配送済みの最新RawMessageを必要なときだけdecode/presentする。Controllerは全選択cameraの最新入力をdecode前coalescingで保持するため、背景用の表示周期で古くなったframeも、全体Seekなしに最新化できる。Seek進行中はそのtarget・generationを変えず、後続restoreに表示priorityを引き継ぐ。追加のcursor/cache stateは持たない。

この共通処理はdecodeが成功してからpending/visible stateを更新する。破損Camera入力ではエラーを明示し、以前の画像・priority・cursorを保つ。入力planにないcameraは引き続き拒否する。

サンプルの全planned cameraを復元した後、実際のeguiプルダウンで6回切り替えた。camera/Sceneを左右に並べた固定desktop layout、1920×1080/DPR 1、NVIDIA Chrome。保持済みthumbnailからの初回表示があるため、初回GPU importと、読み込み・全7カメラのtexture準備が終わる時点を区別した。

| 観測対象 | 変更前 | 変更後 |
| --- | ---: | ---: |
| 通常の初回focus image import | 約44ms | 約44ms |
| Source Readyかつ全7 camera texture準備完了 | 約106ms | 約54ms |
| 同上、File readごとに100msを付加 | 約1174ms | 約54ms |
| 切替から上記完了までのFile read中央値 | 11回 | 0回 |

**初回表示が速くなったとは主張しない。** 不要な再Seek、その後の読み込み・texture再生成をなくす変更である。Source完了時刻は50msごとのread-only status観測で、compositorの画面提示時刻ではない。100msは計測時のBlob readだけに加える代理I/O遅延で、製品や実Androidの読み取り時間ではない。未planned cameraの購読や不正確な背景frameの流用は追加していない。

## 通常再生への回帰確認

同じ実録FHD 7カメラを5秒×各3回、before/afterの順を交互に実GPUで計測。compileや別GPU実験は同時に実行しない。

| 中央値 | 変更前 | 変更後 |
| --- | ---: | ---: |
| CPU cores | 0.685 | 0.666 |
| Chrome process tree PSS | 469.1MiB | 469.9MiB |
| camera imports/s | 40.22 | 40.16 |
| cursor seconds/s | 1.005 | 1.004 |

この短い条件で目立つ劣化は出ていない。汎用的な速度向上や全workloadの同等性を保証する値ではない。生traceはignoredの`target/responsiveness-review/`に置き、小さな[集計](summary.json)を保存する。

## 全体を見た範囲と残る候補

| 経路 | 今回の評価 |
| --- | --- |
| Native記録オープン、Topic catalog、数値field追加 | 全schemaの先行解析を改善。数値seriesの検証・選択schemaのエラー表示は維持 |
| Web Local/Remoteの停止中camera選択 | exact restoreの重複を改善。入力planとqueryの分離は維持 |
| Inspector | Native workerとWebの最新要求coalescing、schema/page cacheを確認。全履歴を保持しない既存方針を維持 |
| Plot | 共通UIのpoint cacheとborrowed plot points、独立queryを確認。明確な改善根拠がない箇所は変更しない |
| Scene/PointCloud | revisionによるgeometry/GPU再利用を確認。未変更の点群を毎tick変換し直す修正は不要 |
| Nativeのcamera切替 | 選択streamが同じならpending-camera表示処理を既に利用していた。今回Webも再利用 |
| Native reader fork | catalog・summary・chunk factsはArc共有済み。重いcopyと決めつけず維持 |
| Web compressed Chunkの同期展開 | 次の計測候補。比較的小さいChunkのbulk展開や大きいrestoreでevent loopがどれだけ塞がるか、long-taskと操作遅延を測る必要がある |
| Native worker破棄 | Source交換では既存workerを再利用済み。再利用されず破棄される経路のjoinが操作待ちになるか未計測で、所有・取消を変える前に実測する |

後ろ2つは未検証の候補で、今回発見した確定不具合とは区別する。大きなworker/async再設計をこの変更に混ぜていない。

## 再現

```sh
cargo run --release -p performance-probe --example topic_catalog
python3 tools/performance-probe/measure_web_camera_switch.py \
  --before target/responsiveness-review/before \
  --after target/responsiveness-review/after \
  --data target/sample-data/release/turtlebot3-7cam-fhd-10s.mcap \
  --output target/responsiveness-review/new-camera-check
```

I/O遅延条件は別outputで`--read-delay-ms 100`を追加する。通常再生の比較は既存`measure_panel_sharing.py --kind web`を使う。初回topic探索、解析エラーの再利用、カメラのknown absence、通常再生で溜まった未表示Cameraの最新化、破損入力時の状態保全、進行中/失敗Seekの扱いを回帰テストで確認する。

最終検証：Workspace Clippyと全546テスト（16 ignored）、strict egui wasm32 Clippy、共通crate wasm32 check、標準Trunk release buildが成功。Native拡張構成92 passed/8 ignored、ROS 2 Live 88 passed/5 ignored、NativeのNVIDIA/Vulkan実再生、Web Local/Remoteの再生・Seek・Inspector・Plot・Source交換も確認した。ignored testを実行済みとは扱わない。
