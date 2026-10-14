# 初回性能検証の結果（2026-10-14）

この文書は修正前のbaseline。[後続のForward pruning・時間順序検証](../2026-10-14-forward-pruning/findings.md)
でNative forwardを修正した。以下の測定結果を現在の挙動と混同しない。

27種類の合成ROS 2記録で、Native forward / indexed history query / latest-before
queryを各3回測定した。計432 run。最大記録は2,647.295 MiB（約2.585 GiB）、600秒。
結果は「省メモリと選択decodeは確認できるが、選択I/Oは経路とChunk構成次第」。
現時点では数十〜数百GBの実記録、Browser、NAS、GUI全体の性能は未検証。

再現・指標・条件の詳細は[計測手順](../README.md)。
[全条件の集計](summary.md)、[各runの生データ](runs.jsonl)、
[機材・compiler・binary/dataset hash](environment.json)を保存した。
計測対象は `e2baa17` を基点に、この変更のprobeとカウンタを加えたrelease binary。
データファイル本体はGitへ含めず、generatorとSHA256で再現する。

CPU: Intel Core Ultra 7 265KF、Linux x86_64、Python 3.12.3。
新規process、逐次実行、順序を固定seedでシャッフル。CPU affinityは固定pinしていない。
cacheは未制御であり、OSの実storage read counterはrunごとに0の場合も非ゼロの場合もある。
したがって以下のread量は主に「アプリケーションが要求したpositional read量」。
SSD帯域やcold/warmレイテンシーを比較した数値ではない。
時間は反復の中央値。MiB = 2^20 byte。RSSはprocess全体で、live heapだけではない。

## H1: 少数トピックによる削減

**decodeは削減できた。I/Oとメモリは一律に減らない。**

20秒、zstd、目標1 MiB Chunk、JPEG 4台とScanの条件:

| Chunk構成 / 経路 | 選択 | 要求read MiB | 展開 MiB | CDR decode | JPEG decode | workload ms |
|---|---|---:|---:|---:|---:|---:|
| mixed / Native forward | Scan | 9.009 | 11.420 | 200 | 0 | 6.14 |
| mixed / Native forward | all | 9.009 | 11.420 | 280 | 80 | 44.25 |
| mixed / indexed | Scan | 23.394 | 29.726 | 200 | 0 | 15.87 |
| mixed / indexed | all | 23.394 | 29.726 | 280 | 80 | 49.37 |
| grouped / Native forward | Scan | 10.797 | 11.420 | 200 | 0 | 13.37 |
| grouped / Native forward | all | 10.797 | 11.420 | 280 | 80 | 46.48 |
| grouped / indexed | Scan | 0.030 | 0.293 | 200 | 0 | 2.37 |
| grouped / indexed | all | 10.797 | 11.419 | 280 | 80 | 53.58 |

allのcamera入力は400件だが、1秒batchのadmissionで80件へcoalesceされる。
280はScan 200＋camera CDR 80であり、全メッセージのdecode数ではない。
GUIのフレーム周期で同じ倍率になるとは言えない。

コード上、Native `read_through`は物理Chunk順に進み、`read_chunk`が展開後に選択を
適用する。選択外だけのChunkでもread/展開を実行する。indexed queryはChunk Indexの
channel情報で除外できる。混在Chunkを展開する際には選択外のrecordも通過する。
64 KiB条件等ではmixedでもScanのないChunkを除外できるため、mixedという名前だけで
削減不能とは断定しない。

RSSも単純なall > Scanにならない。mixed/forwardの最終RSS中央値はScan 6.98 MiB、
all 5.76 MiB、HWMはそれぞれ6.98 / 8.96 MiBだった。最新cameraのChunk pinとsource
cacheの寿命、allocatorの再利用によって変わる。論理payload削減量とRSS削減量は別。

## H2: 圧縮・Chunkサイズ・混在への依存

**強い依存を観測した。ただしサイズと混在度は完全に独立した実験ではない。**

20秒、mixed、Scan-only indexed query:

| 圧縮 | 目標Chunk | 実Chunk数 | 要求read MiB | 展開 MiB |
|---|---|---:|---:|---:|
| none | 64 KiB | 134 | 10.334 | 10.313 |
| zstd | 64 KiB | 134 | 9.938 | 10.313 |
| lz4 | 64 KiB | 134 | 9.900 | 10.313 |
| none | 1 MiB | 12 | 29.730 | 29.726 |
| zstd | 1 MiB | 12 | 23.394 | 29.726 |
| lz4 | 1 MiB | 12 | 28.444 | 29.726 |
| none | 8 MiB | 2 | 140.745 | 140.742 |
| zstd | 8 MiB | 2 | 22.257 | 140.742 |
| lz4 | 8 MiB | 2 | 134.571 | 140.742 |

同じmessage集合でも1秒windowをまたぐChunkが大きいと繰り返し展開が増える。
8 MiB/zstdでは記録全Chunkの非圧縮量11.420 MiBに対し140.742 MiBを処理した
（約12.3倍）。I/Oが小さくても展開・parseコストは小さいとは限らない。
圧縮の改善は32種類を循環する画像の重複にも依存する。実映像へ一般化しない。

groupedは1秒内でトピック境界にflushするため、1 MiB目標でも101 Chunkになる。
混在を減らした効果と実Chunkサイズ／時間幅が変わった効果を分離できていない。
Chunkはメッセージ境界で切れるので64 KiB目標の実最大値も約0.088 MiBになる。

## H3: 記録長と常駐メモリ

**この条件では、記録サイズに比例するdecoded historyの保持は観測しなかった。
metadataの増加は観測した。普遍的な定数メモリ保証ではない。**

非圧縮raw画像＋Scan、目標1 MiB、mixed、Scan-only Native forward:

| 記録長 | ファイル MiB | Chunk数 | open RSS MiB | 最終RSS MiB | HWM MiB | 保持Scan点数 |
|---|---:|---:|---:|---:|---:|---:|
| 60秒 | 264.732 | 241 | 3.41 | 4.96 | 10.28 | 360 |
| 600秒 | 2,647.295 | 2,401 | 4.46 | 4.96 | 10.36 | 360 |

ファイル10倍に対してRSSのピークはほぼ同程度。open RSSは約1.05 MiB増えた。
Message Indexのオンディスク総量は46,815 → 468,015 byteだが、全Message Indexを
開いた時点で常駐させているわけではない。Summary/Chunk Index/stream-to-chunkの
補助構造が増える。progressごとのRSSは生データの`samples`で追える。

JPEG/all Native forwardでも、20 / 120 / 600秒のHWM中央値は8.96 / 9.07 / 9.28 MiB。
最新RGBは4枚計921,600 byte、Scanは360点を保持する。累計decode件数が増えても
全履歴の画素を保存してはいない。

raw/allはraw画像decode非対応なので、低RSSをraw画像表示の実績として扱わない。
多数camera、巨大な個別message、巨大Chunk、長いTF archive、Scene蓄積、Webのwindow
prefetchを含めれば保持量は変わる。RSSだけでbacking retentionの内訳は証明できない。

## H4: 再読み込みとquery latency

**bounded cacheは局所性がある操作を改善するが、miss時のtail costは残る。**

20秒、zstd、8 MiB、Scanのlatest-before読み込みprimitive、64回/run。
`seek`はrestore用のquery単位cache、`seek-cached`はInspectorの1 Chunk cache。
両者とも同じtarget列を使う。全GUI Seekは測っていない。

| 操作 | 展開 MiB/run | p50 ms | p95 ms | p99 ms |
|---|---:|---:|---:|---:|
| random、queryごとcache新規 | 402.479 | 0.665 | 1.293 | 3.003 |
| random、1 Chunk cache | 151.860 | 0.003 | 1.279 | 3.140 |
| 同一点、queryごとcache新規 | 513.566 | 0.698 | 1.599 | 3.429 |
| 同一点、1 Chunk cache | 8.024 | 0.003 | 0.003 | 3.307 |

同一点cache条件でも初回fillを含めるためp99は改善しない。cache保持中RSS中央値は
約11.96 MiB、解放後は約3.93 MiB。メモリに1 Chunkを残すコストも併記する。
小さいmetadata index cacheの量はRSSから個別に分離できない。

この8 MiB条件は全記録が2 Chunkしかなく、random操作でも局所性が強い。
1 MiB条件は12 Chunkで、random操作の展開量は61.950 → 55.843 MiBと削減が小さい。
長い記録でrandom Seekのworking setが大きくなる場合を追加測定する必要がある。
64点/runのp99はほぼ最大値で、tail latencyの精密な推定には不足する。

## H5: 記録側の構成

**画像encodingとトピックの分離はread量を変える。ただし分離だけで最適にはならない。**

20秒、zstd、1 MiB、mixed、Scan-only Native forwardの要求read量は
JPEG画像背景で9.009 MiB、同じ元RGBのraw背景では87.490 MiB。
画像を表示しなくても、その画像と同じChunkを読む経路では記録側のencodingが効く。
rawはfeature decode非対応なのでdecode時間の比較には使わない。

非圧縮600秒で、同じScanを別ファイルへ出した場合:

| 背景 | ファイル MiB | Native要求read MiB | indexed要求read MiB |
|---|---:|---:|---:|
| raw画像4台＋Scan | 2,647.295 | 2,646.848 | 3,308.702 |
| Scanのみ | 8.878 | 8.787 | 596.758 |

別記録にするとNativeは約301倍少なく読める。ただしindexed側のScan-onlyでは
1 MiB Chunkが約68秒分を含むため、1秒queryで同一Chunkを繰り返す。
展開は全Chunk量8.784 MiBに対し596.727 MiB（約68倍）。
疎な記録では時間幅によるflushも検討対象になる。「トピックを分ければ常に速い」
とは結論しない。ここでの分離はtopic partitionした別MCAPを単独で開く条件であり、
rosbag2のsplit recordingを連続再生できるという意味ではない。

## 次の調査と改善候補

1. Native forwardのChunk pruning: 選択外Chunkを読まないようにできるか。
   seekの位置、同時刻tie、重なる時間範囲、非時系列Chunkを保ったテストが先。
2. indexed windowの重複展開: 1 Chunk等のbounded cacheで局所性を使えるか。
   logical payloadとpinするbackingを別々に予算へ入れ、全履歴cacheにはしない。
   Native history queryとWeb loaderの実測を分け、必要な経路にだけ対応する。
3. 記録側の時間幅flush: 小さなScan-only recordingでbyte目標だけに頼らない条件を
   作り、metadata増加／圧縮率／Seekレイテンシーのトレードオフを測る。
4. 実記録、数十〜数百GB、full GUI Seek、Browser Local / Remote / NASへ展開する。

今回の変更は計測と文書化のみ。これらの性能改善はまだ実装していない。
この記事の第一段階で言えるのは、必要トピックへのdecode抑制とbounded stateの効果、
そしてChunk単位のread/展開がその効果を制限する条件まで。
