# Webの4K・8カメラ再生と連続Seek

2026-10-15。mcap_player Web単体で既存の極端な入力を実GUIへ渡した。
画像の実際の表示内容を識別して、カーソルだけでは分からない更新頻度とSeek復帰を調べた。
製品コード・resource limitsは変更していない。Lichtblickとの比較ではない。

## 結果

各条件3回、新しい専用Chrome/profileで実行した。成功runの中央値。

| 条件 | 完走 | カーソル終端 | focusの画像変化 | 背景の画像変化／台 | 個別Seek復帰 | 連続Seek復帰 | 観測peak PSS |
|---|---:|---:|---:|---:|---:|---:|---:|
| none・8 MiB目標Chunk | 3/3 | 3912 ms | 12回 | 約11.4回 | 424 ms | 451 ms | 1763 MiB |
| zstd・64 MiB目標Chunk | 0/3 | — | — | — | — | — | 外部2 GiB guardで停止 |

**カーソルはほぼ1xでも、画像は入力10 Hzどおりには表示されなかった。**
約3.91秒の再生中にfocusで約3.1回/秒、背景で約2.9回/秒の表示内容の変化を観測した。
初期の1枚は数えず、終端の未完成decodeを待たずに次のSeekへ進む。
本番のadmission目標はfocus 10 Hz、背景最大5 Hzであり、もともと8台×10 Hzの全画像表示を要求する設定ではない。
さらにこの計装・環境ではその目標より少ない更新だった。

noneの個別Seekは8台すべてで要求先の画像を確認できた。
再生中の前後連続Seekも、最後の要求後に8台の正しい画像へ戻り、その後1秒間で別画像への差し戻しは観測しなかった。
個別Seek後も0.5秒間、別画像が描かれないことを確認した。
これは短い固定操作列の結果であり、任意のSeek／任意入力に対する保証ではない。

zstd条件は3回ともPSS watchdogで打ち切った。
観測時点のPSSは約2066〜2116 MiBで、1回は4つ目の個別Seek中、2回は通常再生中だった。
これは**Chrome process treeの外部guard**であり、MCAPのChunk上限による製品の拒否やOOMではない。
約200 ms間隔で検査するため2 GiBを超えてから停止する。2 GiBの厳密なOS強制上限ではない。
打ち切ったrunを成功の時間／メモリ中央値へ含めていない。

[summary.md](summary.md)、[environment.json](environment.json)、各`run-*.json`に条件と全samplesを保存した。
[画面](camera-layout.png)も確認した。全6 runでglobal JS例外はなく、観測した画像fingerprintはすべて参照32種類のいずれかに一致した。

## 条件と操作列

- 既存の3840×2160、JPEG quality85、8 cameras×10 Hz、Scan 360点×10 Hz。
  名目4秒、画像320件。最終log timeまでのspanは3.9秒。
- 固定seedのgradient/noiseを32種類循環する合成入力。実カメラの圧縮率や動きを代表しない。
- none/8 MiBとzstd/64 MiBの2条件。同じmessage集合だが、**圧縮とChunkサイズの2軸を同時に変えている**。
  zstdだけがメモリ増加の原因だとは結論できない。実Chunk情報とdataset SHA256はenvironmentに保存した。
- viewport 1920×1080、DPR=1。focus 960×540、各thumbnail 112×63。
  focusそのものをFHDにした試験ではない。8枚のthumbnailが同時に見えるCSS配置をharnessで指定した。
- Browser Localのfile inputへMCAPのpathを渡す。MCAP本体はHTTP fetchしない。
- 既知のheadless Chrome＋SwiftShaderを使い、GPU環境の切り分けは行わない。
  SceneはCSSで非表示にしてsurface描画をskipする。初期GPU資源とScanのmaterializationは残る。
- 計測前にファイル全体のSHA256を計算するcache-allowed条件。cold storageの性能ではない。
- Chrome/profileを毎回新規にし、条件の順番を反復ごとに入れ替える。同時に測らない。

操作は以下で固定した。

1. 8台の初期画像を確認する。
2. 先頭から終端まで1x再生する。終端で時計が自動停止する。
3. 停止中にsequence 28 → 10 → 26 → 12へ個別Seekし、各回の8画像復帰を待つ。
4. 再生を開始し、28 → 10 → 26 → 12 → 24 → 14 → 22 → 16へ連続Seekする。
   最後のSeekと同じJS taskでPauseし、最終targetの画像を検証できるようにする。
   「再生したまま永久に同じtarget画像を待つ」という測定にはしない。
5. 最後の要求から8台の画像復帰までを測り、その後1秒観察する。

Seek時刻は`sequence / 10 + 0.04`秒。message境界から40 ms離し、最新の`<= target`を使う。
burst間隔は50 ms sleep＋計測・コマンド処理で、成功runの実間隔は約51〜93 ms。
固定50 msの厳密なリアルタイム発行ではない。各要求の`performance.now()`をrawに残した。

## 画像をどう識別したか

画像生成式は`variant = (sequence + camera_id * 7) % 32`。
元記録の`/camera/0`先頭32 JPEGから、focus用960×540と背景用112×63の参照値を作る。
準備は**別Chrome process**で行い、参照画像を展開した際のメモリを計測processに持ち込まない。

製品と同じ`createImageBitmap`オプション（resizeQuality low、orientation/color/premultiplyの指定）で展開し、
16×9のnearest pixel sampleから32-bit fingerprintを作る。
32候補内で各参照値が一意であることとJPEGのSHA256を確認する。
[references.json](references.json)に値とJPEG manifestを保存した。

実GUIの`CanvasRenderingContext2D.drawImage`をharnessで観測し、focusと背景7台の可視canvasを同じ方法で識別する。
focus自身のthumbnailは重複して数えない。
同じfingerprintの再描画は新しい画像変化として数えない。
これは有限の合成fixture内の識別であり、全画素の完全一致や任意の入力に対するframe ID判定ではない。

4秒の40 frameではvariantが途中で循環する。
sequence 0〜7と32〜39は区別できないので、Seekの正解判定は記録内で一意になるsequence 8〜31だけを使った。
初期表示と通常再生の全frameの正確なtimestamp一致を証明するものではない。
正しいSeek復帰には、8台の識別値、committed cursor、Ready表示の一致を要求する。
復帰後は描画イベントを観察して別の画像が現れないことを確認する。

**計測には縮小Canvasとpixel readbackのコストが含まれる。**
表示内容を観測した実測だが、未計装の製品の上限fpsと同一視しない。
CPU decode数ではなく可視canvasの内容変化を数えるため、coalescingで読み飛ばした画像を表示成功と数えない。

## メモリと応答性

専用Chromeとその子孫processの`smaps_rollup`からPSSを合算した。
process間で共有するpageの二重計上を抑える。HTTP serverと計測Pythonは含めない。
GPU VRAMや全機のpage cacheを含む総メモリではなく、WASM feature stateだけの保持量でもない。
UI、WASM linear memory、DataPlane backing、JS/Blob/Bitmap、browser/GPU process、allocator/GCを含む。

約200 ms間隔のsampleなので真の瞬間peakではない。各sampleの実時刻・phaseを保存した。
長い記録でメモリが頭打ちになるか、今回の増加がlive allocationかhigh-water markかは未判定。
4秒試験からメモリリークや記録時間比例を断定しない。

10 ms heartbeatの通常再生中の最大間隔は、noneの反復間中央値で約86.7 ms。
これはタイマー発火の間隔であって、main-thread CPUを連続で占有した時間そのものではない。
個別Seekは各run内4要求の中央値を取り、反復間の中央値を出した。
要求数が少ないのでtail latencyの推定には使わない。

## 見つかった課題と次の切り分け

1. **画像更新が目標に届かない。** まず未計装controlと計装ありを比べてobserver overheadを確認する。
   次にMCAP read/展開、CDR/admission、worker待機、JPEG、Canvas uploadのどこが支配的か調べる。
   今回の結果だけでJPEGだけを原因とはしない。
2. **大きい圧縮Chunk条件でChrome PSSが高い。** 同じChunkサイズでcodecを変えるなど軸を分ける。
   DataPlane backingとJS/WASMの一時領域・GC後の保持量を調べ、予算内のlogical bytesとprocess全体を区別する。
   まず上限を引き上げる修正はしない。
3. **終端でPlayボタンの表示が古い。** `PlaybackClock::commit_cursor`は終端で停止するが、
   Webのボタン更新はonclick内だけなので「Pause」と表示され続ける。
   最初の計測試行ではこれを頼りにクリックして再生を再開してしまった。
   本計測では終端でtoggleせず、再生中burstの最後だけ明示的にPauseした。製品修正は今回は行っていない。

## 再実行

通常のWeb release buildとChromeを用意する。ここで新たなMCAP生成やRust変更は不要。
サンプルと結果ディレクトリは新規pathを使う。

```sh
target/release/performance-probe extract-jpeg \
  /tmp/mcap-stress2-20261014-data/3840x2160-8cam-10hz-none-8MiB-jpeg.mcap \
  /tmp/web-camera-stress-samples-new /camera/0 32
python3 tools/performance-probe/prepare_web_jpeg_benchmark.py --samples /tmp/web-camera-stress-samples-new
python3 -m http.server 8094 --bind 127.0.0.1 --directory apps/viewer-web/dist
# 別terminal。CDP 9238は未使用にする。
python3 tools/performance-probe/measure_web_camera_stress.py \
  --prepare-references --output /tmp/web-camera-stress-references-new.json
python3 tools/performance-probe/run_web_camera_stress.py \
  --data-dir /tmp/mcap-stress2-20261014-data \
  --references /tmp/web-camera-stress-references-new.json \
  --output /tmp/web-camera-stress-results-new
# 再集計だけ
python3 tools/performance-probe/run_web_camera_stress.py \
  --summarize-only --output /tmp/web-camera-stress-results-new
```

検証: Python構文チェック、32参照値の一意性とJPEG SHA256、実ブラウザ6 run、
成功runの12個別Seek・3 burst復帰を確認した。global JS例外と未処理Promise rejectionは全runで0件。
3回の外部guard停止は失敗として保存した。製品Rustコードとresource limitsは変更していない。
