# Native ForwardのChunk pruningとGrouped時間順序

初回計測で見つけたNativeの選択外Chunk読み込みを修正した。
同時に、Grouped・時間範囲の重なるChunk・非時系列のChunk配置を、細かなtickとseekで検証した。
これは[初回baseline](../2026-10-14-headless/findings.md)の後続変更であり、indexed windowの
再展開cacheを実装したものではない。

## 実装

* 選択streamとChunk Indexのchannel情報から、読むChunkの集合を決める。
  未選択Chunkはpayload read・展開・CRC検査を実行しない。
* Chunkを開始log time／物理index順で走査する。activeなChunkの先頭messageと、
  次のChunkの開始時刻を比較し、必要なChunkだけ追加する。
* log time → StreamId → 物理Chunk index → 同Chunk内record順でmergeする。
  大きなChunk内の将来messageによって、別Chunkの現在messageが止まらない。
* seekは開始時刻順のChunkに対する終了時刻のprefix maximumを使う。
  終了時刻が単調でない記録でも候補を落とさず、不要な古いprefixを除外する。
  target未満をskipし、すべてのreadに成功してからforward位置をcommitする。
* RangeQueryは同じ順序の走査を使い、`[start,end)`、message数／payload上限と
  `complete`判定を維持する。forkしたreaderは元の再生位置を変えない。
* active Chunk backing合計を256 MiBで制限し、超過は明示的なエラーにする。
  消費済みChunkは解放する。staging中の旧状態、返却batch、featureのpin、metadata、
  allocatorを含むprocess全体の256 MiB保証ではない。

順序問題を単に返却Vecのsortで隠さず、tickに届く時刻そのものを正しくした。
全記録をdecoded historyへ取り込む変更や、すべてのChunkを常駐させるcacheは導入していない。
通常の時系列Chunkでは1つずつ読み、重なる場合だけ複数を保持する。
未選択Chunkの破損は再生を止めない。選択されたデータのread/CRCエラーは従来通り失敗する。

## 追加した正しさ検証

`viewer-mcap/tests/forward_order.rs`:

* Chunk終了時刻が100 → 20 → 200 → 30のように非単調な配置。
* 複数topicのGroupedと、同topicが複数の重なるChunkに現れる配置。
* 0 / 1 / 5 / 9 / 10 / 11 / 20 / 30 / 99 / 100の細かなtick。
* 同一時刻・同一streamの複数recordを含むdeterministic tie。
* forward/backward/repeated seek、EOF後のseek、targetそのものの包含。
* bounded RangeQueryの時刻順prefixと`complete=false`。
* 未選択Chunkの破損をpruneし、選択Chunkの途中失敗で既読位置を消費しないこと。
* failed seek後も、失敗前のforward位置が維持されること。

Sourceのunit testでは、256 MiBを実allocationせずmetadataと既存cacheの占有をmodel化し、
overlap budget超過が追加read前に拒否され、位置が維持されることを検査する。

`performance-probe/tests/probe.rs`では、実ROS CDRのGrouped fixtureを100 ms tickで読む。
messageが各tickの `[start,end)` 内にあり、時刻／stream順で、全30件を重複なく受け取る。
FeatureRuntimeではScan 10件＋camera 20件をdecodeし、JPEG decodeも20件になる。
probe自体にもtick窓内包・順序の検査を追加し、件数一致だけでは成功にしない。

## 再計測

初回の20秒JPEG記録18種類をSHA256照合し、Scan/all、100/1000 ms tickで各3回。
216 run。新規process、逐次・固定seedで順序をshuffle。cacheは未制御。
readはpositional readの要求量であり、SSD転送量を示す数字ではない。
生データは[runs.jsonl](runs.jsonl)、全条件は[summary.md](summary.md)、
binary/source/datasetのhashは[environment.json](environment.json)に保存した。

zstd、目標1 MiB、20秒:

| 構成／選択 | tick ms | 要求read MiB | 展開 MiB | workload中央値 ms | CDR / JPEG decode |
|---|---:|---:|---:|---:|---:|
| grouped / Scan | 1000 | 0.030 | 0.293 | 0.97 | 200 / 0 |
| grouped / Scan | 100 | 0.030 | 0.293 | 1.86 | 200 / 0 |
| grouped / all | 1000 | 10.797 | 11.419 | 44.99 | 280 / 80 |
| grouped / all | 100 | 10.797 | 11.419 | 145.07 | 600 / 400 |
| mixed / Scan | 1000 | 9.009 | 11.420 | 6.07 | 200 / 0 |
| mixed / Scan | 100 | 9.009 | 11.420 | 6.30 | 200 / 0 |

grouped/Scanの修正前Nativeはread 10.797 MiB、展開11.420 MiBだった。
修正後はread 0.030 MiB、展開0.293 MiB。選択外のcamera Chunkを読む必要がなくなった。
mixed/1 MiBではすべてのChunkがScanを含むので、read/展開は減らない。
mixed/64 KiBではScanのないChunkもあり、Scan選択のreadは8.349 MiB、allは11.023 MiB。

100 ms/allでは5 Hz camera入力400件を全件処理し、1秒batchでは80件へcoalesceする。
細かいtickの処理時間増加はdecode量の違いを含む。時間順序を守ってもGUIの実描画、
GPU負荷、実NAS、数十GB以上のメモリとfull Seekレイテンシーは未測定。

再現:

```sh
cargo build -p performance-probe --release --offline
python3 tools/performance-probe/measure_forward.py \
  --baseline docs/performance/2026-10-14-headless \
  --data-dir /tmp/mcap-perf-20261014-final-data \
  --output /tmp/mcap-forward-pruning-results
```

baselineデータがない場合は[初回手順](../README.md)で生成し直し、生成先を指定する。
同じSHA256になるdatasetのみ受け付ける。100 ms単独実行は
`performance-probe run FILE all forward 0 100`。

## 残る制約

* mixedな圧縮Chunk内の選択外topicは依然として展開・parseされる。
* indexed historyのwindow跨ぎ再展開はこの変更では改善していない。
* 256 MiBを超える同時overlapはerror。多数topicを長時間幅の別Chunkへ詰めた記録では
  記録側のChunk時間幅を短くするか、別のbounded読み込み方式の検討が必要。
* metadataはChunk数に比例して増える。sortingとprefix配列のopen時コストも加わる。
* この計測はheadlessの共有Rust経路。Native GUIのFPSやBrowserの性能と同一視しない。
