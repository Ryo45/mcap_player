# Web 4K・8カメラの更新率とメモリの原因調査

2026-10-15。前回の[実画面stress試験](../2026-10-15-web-camera-stress/README.md)で見えた
約3 Hzの画像更新と、zstd・64 MiB ChunkでのChrome PSS超過を切り分けた。
**製品コード・予算は変更していない。** 計測コードと調査結果のみ。

結論は、画像側ではJPEG展開の処理能力とfocus優先度、メモリ側では時間だけで先読みする設計と
Chunk backingの保持・一時確保が改善対象になる。計測用pixel readbackとJS GC待ちだけでは説明できなかった。
ただしブラウザ内部のdecode方式やallocatorの詳細までは特定していない。

## 条件と計測の限界

前回と同じ4K JPEG・8 cameras×10 Hz・名目4秒の合成MCAP、Browser Local、
headless Chrome＋SwiftShader。各条件2回、毎回新しいChrome/profileを使い、順次実行した。
ファイル全体のSHA256を起動前に読むcache-allowed条件。
viewportは1920×1080、DPR=1、focus 960×540、thumbnail 112×63×8、SceneはCSSで非表示。
**focusの表示領域をFHDにした試験でも、実GPUの性能試験でもない。**
この結果だけで一般の実機ブラウザの限界を決めない。

none/zstd × 8/64 MiB目標Chunkの4条件を比較した。message集合は同じ。
実Chunkサイズ・データ生成条件は[元fixtureのmanifest](../2026-10-14-4k-large-chunks/datasets.json)、
使用ファイルのSHA256とChrome情報は各raw JSON、ビルド資産hashは[environment.json](environment.json)に保存した。

観測方法は以下の3通り。

- `bare`: Worker・Canvasをhookせず、製品DOMの実base upload回数/秒、WASM確保量、CDP heap、process PSSのみ観測。
- `light`: Workerの通常リクエスト/返信に時刻を付け、`createImageBitmap`のawait時間と受信・Canvas描画を記録。
  Canvas/BitmapはWeakMapでjob IDへ結び、画像payloadやBitmapへの強参照は保存しない。
  readbackせず、実際に異なるjobを描いた回数を数える。これは異なる画像内容の証明ではない。
- `fingerprint`: `light`に前回と同じ16×9 pixel readbackを加える。既存の32参照値を使う。

`light`のworkerはBlob shimから製品workerをimportする。通常の返信に計測値を添付するだけで、追加返信は送らない。
小さなhook/Promiseの観測コストは残る。`bare`もCDP・DOM・メモリ収集を約200 ms間隔で行うため、完全無計装ではない。
条件順は各反復内で固定なので、僅かな差を方式の優劣として扱わない。

再生終端で自動停止後、in-flight処理のため約1秒待つ。そこでmain targetの`HeapProfiler.collectGarbage`を1回だけ行い、
約400 ms後のメモリと比較する。GCは性能計測中に実行しない。Seekは今回の切り分けには含めない。
PSSはChrome process tree全体。約200 ms間隔の2 GiB guardで打ち切り、OSの厳密な上限ではない。

## 1. pixel readbackが約3 Hzの主因か

none・8 MiBで比較した。

| 方法 | 成功 | 再生中の描画job数、8台合計 | worker await中央値/run | peak PSS |
|---|---:|---:|---:|---:|
| bare | 2/2 | hookなし | hookなし | 1768 / 1775 MiB |
| light | 2/2 | 102 / 100 | 62.4 / 62.8 ms | 1750 / 1683 MiB |
| fingerprint | 2/2 | 97 / 94 | 61.4 / 63.2 ms | 1721 / 1676 MiB |

`bare`でも、drain開始後に残っていた直近1秒bucketの製品表示は各カメラ約2〜4 Hzだった。
`light`の再生時間は約4秒で、全体約25描画job/秒、8台平均約3 Hz。
readbackは無負荷ではなく、追加時に数job減る差もあるが、**readbackを除いても10/5 Hz目標に届かない**。

none8/lightで、workerへのpostから`createImageBitmap`開始までは中央値約1.8〜2.0 ms、
そのawaitが約62〜63 ms、完了からmainの返信受信は約0〜0.3 ms。
Canvasの`drawImage`呼び出し時間は通常約0 msだった。
このCanvas時間は同期呼び出し時間で、GPU完了や画面scanout時間ではない。

同じ4K入力で、出力960×540は約64.8 / 66.0 ms、112×63も約62.1 / 62.2 msだった。
縮小出力を要求する現在のAPI経路では、処理時間が出力面積に比例して減っていない。
これは「ブラウザ内部が必ず4K RGBAへ全展開する」「IDCT縮小を全くしていない」という証明ではない。

製品の[CameraBitmapQueue](../../../apps/viewer-web/src/camera_bitmap.rs)は2 job上限で、pendingをcamera ID順に巡回する。
focus 10 Hz / others最大5 Hzは上流のadmission設定で、pixel workerのfocus優先予約ではない。
飽和するとfocusもほぼ均等な取り分になる。
2 workerと約63 ms/jobから得られる単純な上限は約32 job/秒で、tick・受け渡し・MCAP処理等を含む実測は約25 job/秒。
必要な約45 job/秒をこの処理能力では満たせない。

## 2. 圧縮とChunkサイズを分ける

全て`light`。WASMは線形メモリの最大byteLengthで、live allocation量ではない。

| 圧縮 / 目標Chunk | 結果 | peak PSS、各run | 最大WASM、各run |
|---|---|---:|---:|
| none / 8 MiB | 2/2完走 | 1750 / 1683 MiB | 1008 / 999 MiB |
| none / 64 MiB | 2/2完走 | 1923 / 1935 MiB | 1182 / 1185 MiB |
| zstd / 8 MiB | 2/2完走 | 1584 / 1526 MiB | 787 / 784 MiB |
| zstd / 64 MiB | 2/2 guard停止 | 2082 / 2051 MiB | 1308 / 1308 MiB |

zstd64は再生中に打ち切り。OOM・製品の拒否とは判定していない。打ち切りrunから完走時間や全体更新率を算出しない。
圧縮だけでもChunkサイズだけでも結果を説明できない。
zstd8のWASMがnone8より小さい理由を、この計測だけでallocatorの再利用等へ断定しない。

完走時の製品DataPlane表示は以下だった。ここは製品が出す十進MBであり、表のMiBとは異なる。

| 条件 | 保持window数 | logical payload | retained backing | 累計read bytes |
|---|---:|---:|---:|---:|
| none8 / zstd8 | 3 | 712.6 MB | 727.6 MB | 968.2 / 959.3 MB |
| none64 | 3 | 712.6 MB | 885.1 MB | 1146.4 MB |

zstd64のguard時も3 windows、logical 712.6 MB、backing 849.4 MBだった。
Chunk境界とwindow境界の重なりで、同じChunkを別window用に読み直し、backingを別に保持し得る。
現在のloaderはwindowをまたぐChunk再利用を行わない。

コードから確認できる増加要因:

1. [FetchPlanner](../../../crates/viewer-core/src/playback/data_window.rs)は1秒window・目標2秒先読みを使い、
   demandに常駐byte数がない。既に重いwindowがあっても時間条件で次を要求できる。
2. 同ファイルの`MemoryWindowStore::enforce_budget`はcursorのwindowが先頭なら停止する。
   その場合は後続windowも残る。256 MiBは厳密な上限になっていない。
3. [Web DataPlane](../../../apps/viewer-web/src/data_plane.rs)のbudget適用はloader完了時のみ。
   cursorが進むだけでは再適用しないため、最後のload後は消費済みwindowも残り得る。
   今回は終端・GC後でも3 windowsが残った。
4. [Browser loader](../../../apps/viewer-web/src/local/loader.rs)はFile sliceのArrayBufferをWASMへcopyする。
   [zstd展開](../../../crates/viewer-mcap/src/recording/chunk.rs)では、その圧縮入力と出力backingが一時的に共存する。
   大きなChunkではwindow境界の余剰と一時確保が大きくなる。

zstd64ではworker完了からmain受信までの中央値も約20〜28 msへ伸びた。
Chunk処理との競合は疑われるが、この計測はmain threadの個々の遅延をdecompress/parseへ完全に帰属させていない。

## 3. GC待ちとWASMを区別する

完走した10 runでは、終端後の明示的GCによるPSS減少は約7〜15 MiB。
WASMのbyteLengthはGC前後で変わらなかった。main JSのused heapは小さく、CDPのbackingStorageSizeはWASMを含む。
**WASM・CDP backing・Chrome PSSを足し合わせて内訳を作ってはいけない。**
線形メモリには解放済みallocator領域も含まれ、物理常駐量やlive allocation量ではない。
また今回のGCはmain targetのみで、全worker・native image資源を強制回収する試験ではない。
「リークはない」「zstd64もGCすれば戻る」とは結論できない。guard停止runにはGCを実行していない。
それでも、終端に残る大きなDataPlane backingとWASM高水位は、main JSのGC待ちだけでは説明できない。

## 改善の優先順位

1. **DataPlaneのbyte pressureを先読みに反映し、cursor前進時にも消費済みwindowを退避する。**
   required dataは確保し、任意の2秒aheadを優先しすぎない。まず既存store/plannerの責務内で扱う。
   1 windowや1 Chunk自体が予算超過する場合を別に定義する必要があり、「256 MiB厳守」にはwindow粒度の追加検討も要る。
   再生・Seekの完全性、未完windowでcursorを進めないこと、失敗時rollbackを保つ。
2. **workerのfocus配分を明示する。** 上流のadmissionだけに任せず、飽和時のfocus目標と背景の最低更新を決める。
   これはfocusの見やすさの改善であり、総decode能力の改善ではない。
3. **縮小JPEG展開の別経路を、小さな比較実験で評価する。** 出力540pだけで判断せず、focus FHDでも、同じ入力・画質条件・
   byte budgetで測る。Nativeで使うJPEG経路のWASM適用可能性等を調査し、実測前に置き換えない。
4. 大Chunkのwindow間重複がなお支配的なら、backing再利用と保持期間を検討する。
   無制限Chunk cacheや単純なworker増加・メモリ上限引き上げは最初の修正にしない。

今回は原因調査まで。上記の製品修正、CameraInfo対応、終端ボタン表示修正、Native再計測は含めない。

## 再実行と証跡

前回と同じWeb release build、HTTP配信8094、JPEG samples/manifest/参照値を用意する。
各outputは未使用pathを指定する。inputは`none|zstd`、`8MiB|64MiB`を切り替える。

```sh
python3 tools/performance-probe/diagnose_web_camera.py \
  --data /tmp/mcap-stress2-20261014-data/3840x2160-8cam-10hz-none-8MiB-jpeg.mcap \
  --mode light --output /tmp/web-diagnosis-new/none8-light-r1.json
# 同じdirectoryに-r1.json/-r2.jsonを保存してから集計
python3 tools/performance-probe/summarize_web_camera_diagnosis.py /tmp/web-diagnosis-new
```

[summary.json](summary.json)と12個の`*-r1.json` / `*-r2.json`が本集計。
`none8-light-1.json`はpreflightで、pre/post GCのprocess値を独立保存する前の形式なので集計対象外。
`zstd-64MiB-light-startup-error.json`は起動途中のDOM node取得失敗で、入力を開く前に終了した。
失敗を保存し、WASM起動完了を待つようにharnessを修正して同条件を再実行した。
各rawのsource hashはその試行時点のもので、harness修正前後で異なる。

検証: Python構文チェック、実Chromeの12本試行（10完走・2 guard停止）、全試行の集計を実施。
本集計runでglobal JS例外/未処理rejectionは0件。
Rustと製品JSは変更していないため、Rust workspaceの再ビルド・全テストは実施していない。
