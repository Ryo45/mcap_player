# Webの巨大Chunk処理と継続性能計測

変更前は `e8a6383`。Browser Localの巨大Chunkを現行版で調べ、必要な時間窓だけを保持しながら
読み進める経路を追加した。フォーカス／背景の表示ポリシー、JPEG worker数、1秒windowと
2秒先のbuffer目標は変更していない。Lichtblickとの比較ではない。
変更後はこのbaseへ差分を適用したworking treeで計測した。JSONの`git_commit`はbaseを表し、
実測した成果物は`web_assets`／`probe_sha256`、source hashと保存したsnapshotで識別する。

## 原因と変更

従来は各windowで選択Chunkを丸ごと `File.slice()` → WASMへcopy → bulk展開していた。
1秒分のメッセージしか使わなくても、その `Bytes::slice()` がChunk全体を保持する。
隣接windowも同じChunkを再び展開するため、大Chunkでは読み直し・展開・保持が重なる。
`spawn_local` は別threadではなく、同期展開中はUIも待たされた。

64MiBを超えるZstd／非圧縮Chunkでは次の経路を使う。

- Message Indexからwindow内の物理record位置を求める。非圧縮なら必要なMessageを直接読む。
- Zstdは最大4MiBの入力と128KiBの展開stepで進め、約8msごとにbrowserへ処理を返す。
- 同じChunkのdecoder・未消費入力・途中のrecord・indexを1個だけ引き継ぎ、次のwindowで同じprefixを展開し直さない。
- 物理順と時刻順が違い、次のwindowが以前の位置を必要とする場合は巻き戻す。同時刻のtieも保持する。
- 大Zstdでは必要なCDR payloadだけcopyして保持する。Chunk全体をpinする方式との明示的な交換条件。
- generationをread後／step間で検査し、必要なindex位置とchannel/log timeを確認してからwindowを公開する。

decoder自体は [viewer-mcap](../../../crates/viewer-mcap/src/recording/chunk_stream.rs)、
File API・window選択・継続位置は [Web loader](../../../apps/viewer-web/src/local/loader.rs) が持つ。
feature controller、Camera admission、rendererは変更していない。
別Worker／別WASM heapを追加するより、今回の停止と巨大backing保持を少ない常駐量で解消できた。
処理は現在もmain threadであり、展開を別threadへ移した結果ではない。

## 条件

Intel Core Ultra 7 265KF、RTX 5070 Ti、NVIDIA 610.57.04、Chrome 143.0.7499.192、Linux/X11。
`--enable-features=Vulkan --use-vulkan=native` を指定し、アプリが取得したWebGPU adapterの
`vendor=nvidia / architecture=blackwell / fallback=false` を確認した。
Canvas/WebGLがNVIDIAでも、WebGPUはSwiftShaderの場合があったため、GPU名だけでは判定しない。

1920×1080、DPR=1、focus960×540、thumbnail112×63、Scene非表示。
4K JPEG・8カメラ×10HzとScan、mixed。64MiB／1GiB設定は同じ60秒記録（span59.9秒）から10秒間、
512MiB設定は4秒記録（span3.9秒）をEOFまで測った。
**行をまたいだ比較はChunkサイズだけの実験ではない。変更前後は同じ入力で比較する。**

各条件3回、新規Chrome/profile。反復ごとに条件順を反転。入力SHA256を起動前に読み、cache-allowedとする。
Chrome treeのPSSをCDPとは独立に約200ms間隔で採取し、4500MiBを超えた試行を停止する。
これは実験用の監視値で、製品上限や厳密なpeakではない。GPU VRAMの独立計測でもない。
20ms timerの間隔と、canvasの16×9内容hashの変化を記録する。
表示Hzはcanvas内容の更新であり、decode数・RAF数・物理scanoutの頻度ではない。
hash観測のreadback負荷も含む。

## 変更前後

各processの値を求め、その3回の中央値。UI停止は**各試行の最大heartbeat間隔**の中央値。
初期表示は最初のfocus画像までで、50ms pollingの観測誤差を含む。

| Chunk設定 | 再生進行 log秒/実秒 前→後 | focus Hz 前→後 | 最低背景Hz 前→後 | UI最大間隔 ms 前→後 | peak sample PSS MiB 前→後 |
|---|---:|---:|---:|---:|---:|
| 64MiB | 1.00→1.00 | 9.30→9.85 | 1.88→2.20 | 94→36 | 1994→1749 |
| 512MiB | 0.73→1.00 | 5.82→9.71 | 1.30→2.56 | 798→31 | 4318→1613 |
| 1GiB | 監視停止→1.00 | 未完了→9.88 | 未完了→2.39 | 未完了→31 | 監視停止→1709 |

1GiBの変更前は3回とも外部PSS監視で停止した。観測peakの中央値は4693MiB。
成功試行の速度比や製品自身の拒否／OS OOMとして扱わない。変更後は3回とも10秒区間を再生できた。

初期表示は64MiBで649→722ms、512MiBで915→621ms、変更後1GiBで724ms。
64MiBでは初期表示が約73ms遅くなった。小さいreadの増加・index処理・payload copy・yieldには費用があり、
全条件のすべての指標が改善したという結果ではない。

背景JPEGは約2.2〜2.6Hzで、上限5Hzには届かない。今回の変更はJPEG decoderの総処理能力を上げない。
大Chunkによる停止を取り除いても、この入力の画像処理能力の制約は残る。

## 実用上の位置付け

GiB級Chunkの出現頻度を示す実データ統計は取得していない。既定値だけから頻度は断定しない。
現行の[C++ SDK](https://github.com/foxglove/mcap/blob/main/cpp/mcap/include/mcap/types.hpp)と
[Python SDK](https://github.com/foxglove/mcap/blob/main/python/mcap/mcap/writer.py)は目標1MiB、
[ROS 2のzstd_small](https://github.com/ros2/rosbag2/blob/jazzy/rosbag2_storage_mcap/README.md)は4MiB。
GiB級を一般的な録画条件として扱う根拠にはならない。

ただし目標値は厳密な上限ではなく、[1 recordを収めるため超過する](https://github.com/foxglove/mcap/blob/main/cpp/mcap/include/mcap/writer.hpp)。
3840×2160 rgb8は画像部分だけで24,883,200 bytes（約23.7MiB）なので、数十MiBは自然に生じ得る。
1メッセージ自体がGiB級なら、その例外もある。20GiB・1Chunkは今回の境界試験と位置付ける。
通常性能の評価には既定に近い小Chunkも必要で、このレポートの巨大Chunk試験で代用しない。

ROS 2の`fastwrite`は`noChunking: true`であり、「全体が1Chunk」とは別形式。
公式にもindexを復元する後処理が案内されている。無断の全走査を追加する理由にはせず、
明示的な変換／インデックス生成という別の利用経路として扱う。

[集計](summary.json)と[manifest](matrix-manifest.json)に成功／失敗をすべて含めた。
`runs/before` と `runs/after` が主比較。`runs/prototype` は最終のindex検証・境界テスト追加前の試行で、主集計に混ぜない。
正確な観測scriptは `harness/*.gz` に保存した。後の回帰計測用scriptとは区別する。

## 20GiB超・1Chunk

[入力事実](sparse-source.json): file **21,480,765,943 bytes**、単一の非圧縮Chunk、
3453メッセージ、FHD rgb8・4カメラ10Hz、span86.3秒。
中央の画素がframeごとに変わり、その他のゼロ領域はfilesystem holeとして作る。
実ディスク使用量は4,101,873,664 bytes（約3.82GiB）。Chunkの末尾がEOF以内であることも生成時に検査する。

これは20GiBのファイル長・大きな物理offset・保持量を調べる試験で、
実データ20GiBのcold disk転送速度や20GiB Zstdの性能を示すものではない。
入力SHA256を読む準備は再生計測の外で行う。

3回の独立processで10秒再生と前後Seekを実行し、再生進行約1.00倍、focus約9.98Hz、
最低背景約4.99Hz、peak sample PSS中央値約1406MiBだった。
20GiB全体をWASMへ入れず、indexで指定されたMessageだけを読む経路が動いた。
記録86.3秒を全区間完走した試験ではない。

さらに別の機能検証で1.03→70.03→40.03→85.93→1.03秒へSeekした。
generatorのsequenceから求めた灰色値とfocus／背景3枚の中央画素を独立に照合し、
5地点すべてで正しいpredecessorを確認。各Seek後300ms、古い完了結果に戻らないことも確認した。
単なるcursor更新や「前と違う画像」だけを正解にはしていない。

## 継続する回帰計測

入口を [tools/performance-probe/README.md](../../../tools/performance-probe/README.md) にまとめた。
`regression.py prepare` が固定fixtureを生成／SHA256検証し、`run --baseline` が3反復の中央値を比較する。
ソースpatch・日付付きレポート・固定portに依存しない。失敗・欠落・反復不足・異なる入力／環境／計測器はexit 1。
時間・メモリだけでなく、Webはfocusと最低背景の実内容更新、再生進行、UI停止も確認する。

FHD Raw30Hz、4K JPEG8台、512MiB、選択Scan、PointCloud2 6万点、往復Seekを対象とする。
Nativeは本番データ／decoder／feature復元のCLI計測で、Native GUIのFIFOや可視FPSは含まない。
Webは通常releaseを操作し、decodeしたbitmapがcanvasへ反映されたことも追跡する。
背景の初期decode完了を待ち、初期化途中のcanvas寸法を環境差と誤認しない。
同一画素のframeへSeekしても、単なる内容hashの一致でtimeoutさせない。

[Native baseline](native-baseline.json)／[比較結果](native-current.json)、
[Web baseline](web-baseline.json)／[最終製品の比較結果](web-current.json)を保存した。
Nativeは15試行ずつ、Webは9試行ずつがすべて成功し、baseline付き実行もexit 0だった。
[50%遅延を注入したchecker検査](checks/injected-slowdown.json)は期待どおりexit 1。
観測器の初期版で発生したRaw Seek timeout・初期化中の寸法不一致・UI停止の試行は
`diagnostics/web-release-*`へ分け、採用したbaselineへ混ぜていない。単発のUI停止の原因は未特定。
主比較の後、空windowの古い世代をcacheへ返さない検査を追加し、最終Web buildでも回帰suiteを通した。
主比較の入力には空windowがなく、測定sourceのsnapshotと最終差分はmanifestに明記した。

常設GPU runnerの設定はしていない。専用環境の定期実行／CIではbaseline付きコマンドの終了コードを使える。
Native GUIの描画遅延など、このsuiteが覆わない経路は既存の個別計測で確認する。

## 残る範囲と検査

大Zstdの圧縮prefixへのランダムアクセスは無料にはならない。Grouped配置は巻き戻しを増やし得る。
大圧縮ChunkのWeb restoreには既存256MiBのwork制限が残る。今回の20GiB Seekは非圧縮の直接読取。
LZ4は既存の一括展開を維持し、Nativeの巨大Chunk上限も変更していない。
[NativeのSeek data path](diagnostics/native-20gib-seek.json)は、この20GiB fixtureを既存8GiBのwork上限で拒否した。
indexなし／Chunk Indexなしの記録を黙って全走査するfallbackは追加していない。

開発コンテナで製品のfmt、workspace Clippy全target、全テスト、関連WASM check、Trunk release build、
ROS2 Live有効のNative単体テストが成功した。DDS実機smokeをしたという意味ではない。
時間境界・同時刻tie・非時系列Chunkの巻き戻し／継続・truncation・サイズ不一致・大offset・
record上限を追加検査した。疎ファイルのEOF/backpatchも単体テストで検査した。
[workspace log](checks/workspace.log.gz)を保存した。
