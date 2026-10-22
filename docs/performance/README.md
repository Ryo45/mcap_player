# 性能検証

GUI比較の旧process_sampleには、別スレッドから起動した子プロセスを取りこぼす不具合があった。特に2026-10-20のDesktop比較ではrenderer欠落を確認し、CPU・PSS比較を撤回した。過去のGUI資源量は再監査が必要で、[修正後の表示パターン別比較](2026-10-20-desktop-workloads/README.md)と混ぜて集計しない。

目的は性能上の特徴・限界・トレードオフを把握すること。高機能ビューアの
機能数・優劣を比較するベンチマークではない。製品の経路を変更せず測り、
仮説が成立しない条件も保存する。

[2026-10-14の初回結果とH1〜H5の評価](2026-10-14-headless/findings.md)を参照。
27 dataset、432 run、最大約2.6 GiB。生データと全条件の集計も保存している。
[Native Forward pruningとGrouped時間順序の後続検証](2026-10-14-forward-pruning/findings.md)では、
初回の記録を再利用して100 ms / 1秒tickを比較する。初回結果は修正前のbaselineとして保持する。
[4K・8カメラ10 Hz・最大512 MiB Chunkの計測](2026-10-14-4k-large-chunks/findings.md)は、
本番RGBA decode、4camera×20 Hzとの比較、製品側上限による拒否も含めて記録している。
[Webのfocus優先とbackground更新の比較](2026-10-16-web-focus-scheduling/README.md)では、
60秒・4K/8カメラで表示頻度・画像年齢・最低background頻度とメモリを比較する。
[Nativeの実GPU環境確認](2026-10-19-native-hardware-gpu/README.md)では、
ホストのNVIDIA VulkanによるCamera・BEV・Scene描画と、画面電源Off時のFIFO待ちを記録する。
[Native / Lichtblick Desktopの実GPU比較](2026-10-20-native-lichtblick-gpu/README.md)では、
同じ4K画像を表示し、画素で確認した更新・鮮度・Seekとプロセス全体のPSS / CPUを測る。
[Native Cameraの固定周期化](2026-10-20-native-camera-cadence/README.md)では、
画像受信から描画までの待ちを診断し、周期のずれを直した前後を実GPUで各3回比較する。
[Lichtblickのデフォルト構成に合わせた比較](2026-10-20-desktop-default-layout/README.md)では、
同じ表示領域・視点でgrid / Image / Scan / Raw Messagesの負荷を分け、比較スクリーンショットも保存する。

[Raw Image・複数MCAP・巨大ChunkのDesktop比較](2026-10-22-raw-storage/README.md)では、
FHDのRaw画像を1台／4台表示し、none / Zstd / LZ4、3分割、最大約1GBの単一Chunkを比較する。
Nativeの固定256 MiBゲートを見直し、Chunk先読みとSeek内の展開共有による更新頻度・メモリの変化も記録した。Chunkレコードなしの補助検証は分けて保存する。

[同じシーンを往復するSeekの計測と改善](2026-10-22-seek-revisit/README.md)では、大Chunkの再訪で残っていた再展開を解消した。画面反映の改善、連続再生時の追加メモリ、小ChunkでLichtblickに残る差を分けて記録する。

## 再現

Linux、Rust toolchain、Python 3.11以上（stdlibのみ）。計測中は他のビルドや
重い処理を止める。生成データはGitへ追加しない。

```sh
cargo build -p performance-probe --release --offline
python3 tools/performance-probe/run_experiments.py \
  --data-dir /tmp/mcap-perf-data \
  --output /tmp/mcap-perf-results \
  --seconds 20 --repeats 3 --seeks 64 --large-seconds 600
```

データ／結果ディレクトリは新規のものを指定する。途中で失敗したrunへ別条件を
継ぎ足さない。まず `--seconds 2 --repeats 1 --seeks 8` でスモーク確認できる。
保存物は `environment.json`（機材・compiler・commit・dirty diff hash・binary hash・
dataset SHA256）、`runs.jsonl`（各反復の生データ）、`summary.md`（集計）。
`--large-seconds 600`は非圧縮raw 60/600秒とScan-onlyの記録を追加し、データ領域を
約3 GiB多く使う。指定しなければ小規模matrixだけ。数十GB以上へ広げる場合は
生成先の容量と共有machineへの負荷を先に確認する。
中央値で集計し、固定seedで各反復の実行順をシャッフルする。各測定は新規process。
CPU時間はPythonのchild rusage、RSSはprobe自身のLinux `/proc/self/status`。
再集計は `--summarize-only --output DIR --data-dir DIR`。

## 条件と比較対象

共通データはLaserScan 360点・10 Hzと、4台の320×240 RGBカメラ・各5 Hz。
画像は固定seedのgradient＋noise、32種類を循環させる合成入力。実カメラの
エントロピー・動き・JPEG品質・帯域分布を代表するとは限らない。
JPEG quality 85と同じ元RGBのraw Imageを比較する。MCAP圧縮levelは3。
ROS 2 CDRのlog/publish/measurement timeを一致させる。TF、Path、Odometry、
PointCloud2、圧縮動画、静的TFの巨大なarchiveはこのfixtureに含めない。

| 軸 | 条件 | 検証する仮説 |
|---|---|---|
| トピック | Scanのみ / 全トピック | H1 |
| MCAP圧縮 | none / zstd / lz4 | H2 |
| 目標Chunkサイズ | 64 KiB / 1 MiB / 8 MiB | H2、H4 |
| Chunk混在 | 時系列で混在 / 1秒内でトピック別にflush | H2 |
| 長さ | 20 / 120 / 600秒（defaults） | H3 |
| 履歴アクセス | 固定seedのrandom / 同一点を反復、bounded cache有無 | H4 |
| 記録側 | JPEG / raw、Scanだけを別ファイルへ分離 | H5 |

目標Chunkサイズは実サイズではない。groupedではトピック境界・1秒境界でflushし、
Chunkが小さくなる。混在度とサイズは完全に直交しないため、実際のChunk数・最大サイズ・
展開総量・Message Indexのbyte数を同時に記録する。これは記録側の構成変更の比較であり、
混在度「だけ」の因果効果を証明するものではない。メッセージ集合は同じ。

比較対象は同じ製品経路のScan-onlyとall。allは選択を広げた条件であり、
全履歴常駐する仮想的な競合製品ではない。いずれも同じbounded feature stateを使う。
2026-10-14の初回計測時はraw Imageのfeature decodeが未対応だったため、そのraw/allは
読み込み・保持の比較であり、JPEG/allとのdecode時間の比較には使わない。unknown counterを残す。
現在はRaw表示に対応しており、後続のRaw Image Desktop比較はpixel変換・描画も含む。

## 測る経路

`performance-probe`は `viewer-mcap` / `viewer-core` の本番APIを直接呼ぶ。
描画・ウィンドウ・networkを作らず、実時間のsleepも行わない。

* `forward`: Nativeの`McapSource::select_streams` → `read_through`。
* `indexed`: `McapSource::indexed_range`で1秒の `[start,end)` を順に読む。
  Browserの実I/OでもRecording ServerのIndexedReaderでもなく、共有MCAP層の
  bounded history query。Web全体の性能と同一視しない。
* どちらも1秒batchを`FeatureRuntime`へ渡し、admissionされたカメラだけJPEGを
  CPU decodeする。最新RGBを各カメラに1枚保持する。通常のGUI tickとは異なり、
  5枚/秒のcamera入力を1枚/秒へcoalesceする。CDR decode数・drop数・JPEG decode数を
  別々に保存し、フレームレートやGUI負荷をこの測定から推定しない。
* `seek`: production `latest_before`（queryごとにcache新規）。TF history restore、
  feature restore、次のforward位置、GPU更新を含まない「Seekの読み込みprimitive」。
* `seek-cached`: Inspector用`LatestMessageReader`の1 Chunk/1 index span cache。
  Scan 1streamで同一target列を比較する。全Chunkを常駐させる比較は作らない。
* `repeat` / `repeat-cached`: 記録中央の同一targetへ反復。

全件通過の測定はSummaryのmessage countとdelivery countの一致、feature decode
errorが0であることを検査し、失敗時に停止する。seekはpredecessorの時刻境界を検査する。
groupedは1秒内で物理書き込み順が非時系列になる。初回は1秒batchのcount確認だけであり、
時間順序の検証は後続修正で追加した。現在のprobeは各batchの `[start,end)` 内包と
時刻/stream順序を検査する。`run PATH all forward 0 100`の最後の引数はtick ms。
省略時は1000 ms。100 msでは元の5 Hz camera入力を1秒batchのようにcoalesceしない。

## 指標の意味

* `requested_bytes`: 実際に成功したpositional readのbyte数。Summary・index・CRC header・
  同一範囲の再読み込みを含む。OS page cacheからのコピーも含み、SSD/NAS転送量ではない。
  `os_storage_read_bytes`も保存するが、cache hitが多い場合0になり得る。
* `expanded_bytes`: 実際に処理したChunkの非圧縮byte数を累計。再処理を重複計上する。
  `decompressed_bytes`はそのうち圧縮Chunk由来のみ。非圧縮のparse負荷もゼロではない。
* `read_ms`: ファイルread、index探索、展開、CRC、record parse、message構築を合わせた
  時間。純粋なdisk I/O時間やcodec時間へ分離した数字ではない。
* `feature_ms` / `jpeg_ms`: feature処理とJPEG画素decodeを別計時。
* RSS/HWMはprocess全体。`seek_live_memory`はbounded query cacheを解放する前に採る。
  `final_memory`はその解放後なので、cache保持量の比較には前者を使う。
  共有Chunk backing、index/catalog、allocatorの再利用領域、
  decodeの一時領域も含む。HWMは終了時のlive heapではない。GPU・browser memoryは含まない。
* `source_cache_backing_bytes`は初回baselineではNative cacheの1 Chunk、後続修正では
  時間範囲の重なるactive Chunkの合計サイズ。
  camera等のfeatureが別Chunkをpinする量を含まず、logical JPEGサイズをtotal retained
  backingサイズと読み替えない。実測RSSとlogical/pixel/Scanの保持量を併記する。
* `open_memory`と再生progressごとのRSSを保存し、metadata増加とfeature履歴を分ける。
  Summaryに保持するChunk Index等は記録長で増える。Message Index総byte数はオンディスク量で、
  全量が常駐しているという意味ではない。

OS cacheは制御していない。生成・SHA256計算後でcacheが効きやすい。root権限で
drop_cachesは実行しない。これはcache-allowedのCPU/読み込み要求量の測定であり、
cold SSD/NAS性能の証明ではない。初回openと2回目のSeekの差だけをcold/warmと呼ばない。
percentileは各run内のnearest-rankを出してから反復間の中央値を取る。64 Seekのp99は
ほぼ最大値なので、大規模なtail latency推定には不足する。

## 記事へ進むための追加検証

* 数十〜数百GBの実記録と、画像・LiDAR中心／多数小トピック中心など異なる構成。
* 実Nativeのフレーム周期、Browser Local、Remote Server＋browserで同じ操作列。
  JS/WASMコピー、prefetch、continuation、stale generation、GPUも個別に計測。
* SSD／NAS／遅延のあるremote、cache状態を明示したcold/warm条件。
* 同じmessage集合の異なるChunk構成をさらに作り、混在度とサイズをできるだけ分離。
* 復元を含むfull Seekと、操作頻度／working setに対するbounded cacheの効果。
* PointCloud2のさらに大きい点数・画像動画encodingの条件、ROS bag分割と再結合の操作。

外部仕様の根拠: [MCAP specification](https://mcap.dev/spec)のChunkはレコード集合ごとに
圧縮され、Chunk IndexにchannelごとのMessage Index位置を持つ。
[ROS 2 recording guide](https://mcap.dev/guides/getting-started/ros-2)では記録側の圧縮・
Chunk設定をstorage optionsで変更できる。結果の解釈は仕様と実装、実測を区別する。

[Native JPEG・FHD表示の改善](2026-10-14-native-jpeg/README.md)では、同じ4K入力をTurboJPEG、IDCT縮小、2 workersで比較し、速度・メモリ・実時間処理の限界を記録した。

[Web JPEG / FHD表示の比較](2026-10-15-web-jpeg/README.md)では、旧WASM RGBAと2本のWeb Workerを同じ4K JPEGで比較し、Canvas保持量とmain-thread停止時間も確認した。

[Lichtblickとの最初のGUI比較](2026-10-15-lichtblick-pilot/README.md)では、同じ4K・1camera・20秒のMCAPを
Web Localで各3回再生し、Chrome process treeのPSSを比較した。再生後は両者ほぼ同じだった。
表示FPS・画像の鮮度・random Seekはまだ比較していない。

[Webの4K・8camera再生と連続Seek](2026-10-15-web-camera-stress/README.md)では、実際に描いた画像を識別して
8台すべての復帰を検証した。非圧縮8 MiB条件は完走し、zstd64 MiB条件は外部PSS 2 GiB guardで打ち切った。
カーソル1xと画像更新頻度の違いも記録している。

[Desktopの表示パターン別比較](2026-10-20-desktop-workloads/README.md)では、FHD画面でカメラ1／4／8台とフォーカス配置、Raw配列開閉、Scan 360〜36,000点、RGB PointCloud2 1万／6万点を比較する。背景更新頻度と縮小decodeの差も明示する。
