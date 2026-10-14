# 4K・8カメラ10 Hz・最大512 MiB Chunkの計測

8カメラ×3840×2160×10 Hzと、同じ入力枚数になる4カメラ×20 Hzを測った。
19 dataset、各条件3回、計126 run。57 runが成功、69 runが製品側の上限で拒否された。
外部RSS guard・timeout・予期しないerrorは0件。最大ファイルは約1.85 GiB、生成データ合計は約20 GiB。

現在の同期CPU decode経路で「8カメラの4K映像を高い表示fpsで扱える」ことは確認できなかった。
50 ms刻みで現行admissionを動かすheadless条件では、4秒分の8カメラ記録の処理に約8秒かかる。
Scanだけを使う構成やChunk分離には大きな余地がある。一方、512 MiB Chunkは現行の256 MiB上限で
拒否され、一部の128 MiB目標Chunkもoverlapの合計で拒否される。

[全条件の集計](summary.md)、[各runの生データ・失敗理由](runs.jsonl)、
[生成条件・実Chunkサイズ・SHA256](datasets.json)、[計測環境・binary/source hash](environment.json)。
本結果は`cada71c`にこの変更の計測ツールを加えたrelease binary。製品側のread／展開／active backing
上限、camera scheduling、rendererのdecode関数は変更していない。

## 条件

* 4K = 3840×2160、JPEG quality 85。固定seedのgradient＋noise、32種類の画像を循環させる。
  実映像の画質・動き・圧縮率を代表するとは限らない。
* 8カメラ×10 Hz / 4カメラ×20 Hzはいずれも80画像/秒。Scanは360点、10 Hz。
* JPEG記録は名目4秒、320画像＋Scan 40件。rawは名目1秒、80画像＋Scan 10件。
  最終message時刻までのspanは8×10 Hzで3.9秒、4×20 Hzで3.95秒。比較表の記録時間は名目4秒。
* 目標Chunkサイズは8 / 64 / 128 / 256 / 512 **MiB**。MBの10進表記と区別する。
  none / zstd、補助条件にlz4。圧縮level 3、stress生成はcompression thread 0。
* 同じ混在message集合をmasterからstreamingでrechunkし、Chunk・圧縮だけを変える。
  payload・log/publish time・sequenceが変わらないことをテストする。
  groupedは1秒のtopic境界でflushするため、実Chunkサイズは目標512 MiBより小さい。
* generatorは1秒分の4K payloadコピーを溜めず、descriptorだけ並べてpayloadを逐次書く。
  rawは2画像variantを保持。生成のmax RSSも保存する。
* playbackは50 ms tick、`FeatureRuntime`の現行admissionを使用する。
  フォーカス10 Hz、背景5 Hzなので、入力全画像をpixel decodeする条件ではない。
  8×10 HzではJPEG decode 179件、coalescing drop 136件、終了時未提示5件。
  4×20 Hzではdecode 100件、drop 216件、未提示4件。EOF後のpending flushは行わない。
* pixel decodeは本番`viewer-renderer::decode_jpeg`を呼び、最新RGBAをcameraごとに1枚保持する。
  初回のRGB保持probeとはpixel形式が違う。GPU texture・upload・描画は作らない。
* raw Imageは現在のcamera featureでdecode非対応。raw/allはtransportの検証であり画像表示性能ではない。

CPUはIntel Core Ultra 7 265KF、RAM約31 GiB。processは新規、逐次・固定seedで順序をshuffle。
生成／SHA256確認後に実行するが、OS cacheは制御しない。計測中の他process・CPU frequencyも固定していない。
read量はpositional readの要求byte数であり、SSD転送量と同一ではない。
GPUは使っていない。RSSはGNU timeのchild max RSSで採り、実行中も監視する。
RSS guardは1.5 GiB、timeoutは180秒。guardによる失敗と製品の上限拒否は別statusにする。

## 512 MiBと実Chunkサイズ

8カメラ×4K×10 Hz、mixed JPEG。同じmessage集合なのでcodecによる非圧縮サイズは同じ。

| 目標 MiB | 実最大 MiB | none/zstd Native Forward（Scan/all） |
|---:|---:|---|
| 8 | 9.049 | 成功 |
| 64 | 65.533 | 成功 |
| 128 | 130.795 | active backing合計上限で拒否 |
| 256 | 258.318 | 単一Chunkでも上限超過 |
| 512 | 513.837 | 単一Chunkでも上限超過 |

writerはmessage境界で切るため、目標サイズは実サイズの上限ではない。
単一Chunkが256 MiB以内でも、境界時刻のmessageを順序通りmergeするために2つのChunkを保持すると、
130.795 × 2 > 256 MiBとなる。Scan選択でも次のChunkのstart timeはcameraを含む全topicの
時刻幅なので、粗いChunk情報では不要なoverlapを避けられないケースがある。

512 MiB mixedのindexed queryも上限で拒否される。非圧縮では要求readサイズ、圧縮では
readサイズまたは展開サイズの上限が効く。Forwardはactive backingの予算検査で止まる。
失敗をthroughputの測定成功と数えたり、上限を引き上げて隠したりしていない。

grouped/512 MiB目標の実最大Chunkは29.784 MiBで、8cameraのoverlapを含めても受付可能だった。
これは「512 MiBの実Chunkを受け付けた」という結果ではない。
rawは実最大71.194 / 142.386 / 522.078 MiB。64 MiB目標でもallはoverlap上限で拒否された。
chunkの長い時間幅や残ったfuture messageもactive backingの寿命に関係する。

## 4Kの処理時間とメモリ

成功したall/Forward、名目4秒の記録。時間・RSSはいずれも3回の中央値。
`read_ms`にはread、展開、CRC、record parseを含み、純粋なSSD時間ではない。

| 条件 | read ms | JPEG→RGBA ms | workload ms | JPEG decode数 | peak RSS MiB |
|---|---:|---:|---:|---:|---:|
| 320×240、8cam×10 Hz、zstd/64 MiB | 6.27 | 74.80 | 80.73 | 179 | 16.51 |
| 4K、8cam×10 Hz、none/8 MiB | 676.87 | 7,081.17 | 7,749.40 | 179 | 450.70 |
| 4K、8cam×10 Hz、none/64 MiB | 685.99 | 7,066.99 | 7,766.69 | 179 | 501.39 |
| 4K、8cam×10 Hz、zstd/64 MiB | 1,287.35 | 7,011.49 | 8,325.87 | 179 | 565.79 |
| 4K、8cam×10 Hz、lz4/64 MiB | 1,528.20 | 7,056.42 | 8,602.56 | 179 | 565.80 |
| 4K、4cam×20 Hz、zstd/64 MiB | 1,322.92 | 3,984.50 | 5,332.90 | 100 | 439.93 |
| 4K、8cam×10 Hz、zstd/grouped | 1,134.74 | 6,931.67 | 8,092.36 | 179 | 821.82 |

8cam/zstdの平均pixel decodeは約39 ms/画像。現行admissionでも約45画像/秒を要求するので、
この同期CPU経路・機材・入力では50 ms tickの予定処理量を1xで処理し切れない。
1秒batchでさらにcoalesceして良い時間を出すような測定にはしていない。

8cameraと4cameraは同じ入力80画像/秒でも、admission後のdecode量が異なる。
「camera数だけによる速度差」や「8camera 10 fps描画」の証明ではない。
現在の実GUIは遅れに応じてbatchとcoalescingが変わり得るため、再生1xを保ちつつ表示fpsが
落ちる可能性もある。GUIの実用性は、cursor lag・表示fps・操作レスポンスを別に測る必要がある。

最新RGBAの物理量は1枚33,177,600 byte。8枚 **253.125 MiB**、4枚126.5625 MiB。
peak RSSはこれにserialized backing、staging、JPEG decodeの一時領域、allocator等が加わる。
grouped/Scanはpeak 4.18 MiBだがgrouped/allは821.82 MiB。必要topicが少ない場合と全画像を
使う場合の差は大きい。ただし256 MiBのactive backing予算はprocess全体のRSS上限ではない。
GPUを使うとtexture等の保持量がさらに加わるが、その量・転送性能は未測定。

## 選択I/Oの効果

4K/8cam×10 Hz、Scanのみ:

| 構成 | 要求read MiB | workload ms | peak RSS MiB |
|---|---:|---:|---:|
| none/mixed/8 MiB | 356.65 | 269.82 | 30.68 |
| zstd/mixed/64 MiB | 941.76 | 1,329.97 | 199.67 |
| zstd/grouped/512 MiB目標 | 約0.01 | 1.35 | 4.18 |

小さいmixed ChunkではScanを含まないcamera Chunkを除外できる。
64 MiB mixedではほぼ全Chunkを読み、cameraを表示しなくても約942 MiBを要求する。
groupedではScanだけのChunkを選べる。画像サイズの最適化より「読む必要がある画像Chunkを
減らすこと」が効く用途はある。一方、allの場合の主要な負荷はJPEG→RGBAだった。

## 次の課題

1. **pixel decodeの実測**: boundedな並列decode、SIMD/codec実装、縮小表示時のdecode方針などを比較。
   stale completionやsource/seek切替のgeneration、cameraごとの最新state、RAM予算を守る必要がある。
   今回は並列化・GPU decode・解像度変更を実装していない。
2. **128 MiB級でのoverlap**: 選択streamのMessage Index時刻を使い、粗いChunk startだけに基づく
   先読みを減らせるか。順序・同時刻tie・rollbackの保証を弱めない。
   単に上限を512 MiBへ上げる案はRSS・staging・多数cameraのpinと合わせて再検討する。
3. **GUIの実用性**: 実4K映像、長い記録、Native/Browser、GPU upload、各cameraの表示fps、
   frame/tick p95/p99、cursor lag、操作レスポンス。4秒の平均throughputだけで判断しない。
4. **記録条件**: 画像の品質・圧縮方式・entropy、topic/time partition、Chunkの時間幅。
   今回の繰り返す合成JPEGだけから実機の帯域やZstd効果を一般化しない。

## 再現

Linux、GNU time、Python 3.11以上（stdlib）、Rust release binary。
データ用に50 GiB以上空ける。実生成量は今回約20 GiB。元の小規模fixtureのCLIは引き続き同じ。

```sh
cargo build -p performance-probe --release --offline
python3 tools/performance-probe/measure_stress.py \
  --data-dir /tmp/mcap-4k-stress-data \
  --output /tmp/mcap-4k-stress-results
```

単独の生成／実行:

```sh
performance-probe generate /tmp/4k.mcap none 8388608 mixed 4 jpeg 3840 2160 32 10 8
performance-probe rechunk /tmp/4k.mcap /tmp/4k-512.mcap zstd 536870912
performance-probe run /tmp/4k.mcap all forward 0 50 rgba
```

最後の生成引数はwidth、height、variants、camera Hz、camera数。
4camera×20 Hzは末尾を `32 20 4` にする。None/codec/サイズ比較は同じmasterをrechunkする。
初回のビルド失敗後に旧binaryで始めた不完全な計測は、この結果に含めていない。
