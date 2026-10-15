# Web: 60秒・4K/8カメラでwindow解放の変更前後を比較

2026-10-16。[4秒ログの比較](../2026-10-16-web-window-retirement/README.md)を60秒へ延長した。
**非圧縮8 MiBでは現行版が完走したが、zstd64 MiBは約15秒でPSS guardに達した。**
短い試験の改善を、そのまま長時間再生の実用性として扱えない結果となった。

## 条件

- 3840×2160 JPEG、8 cameras×10 Hz、Scan 360点×10 Hz、mixed。
  生成duration 60秒、最終log timeまでのspanは59.9秒。画像4800件＋Scan600件＝5400件。
- 固定seed、quality85、32種類のgradient/noise JPEGを循環する合成入力。
  実カメラの圧縮率・運動・長期的変化を代表しない。
- none / 8 MiB targetを生成し、同じmessage集合をzstd level3 / 64 MiB targetへrechunkした。
  ファイルはそれぞれ14,944,878,140 bytes / 14,812,121,345 bytes。
  実最大uncompressed Chunkは約9.05 / 65.54 MiB、Chunk数は1600 / 219。
- 変更前は63676f0相当、現行はb90288a。製品差分は消費済みwindowの配送時退避のみ。
  **先読み方針・window長・予算・JPEG処理を変更していない。**
- 各版×各入力1回、計4回。専用Chrome/profileを毎回新しくし、順次実行。
  単発比較なので、停止時刻や細かな差の再現性は未確認。
- Browser Local、headless Chrome＋SwiftShader、viewport 1920×1080/DPR=1。
  focus 960×540、thumbnail 112×63×8、SceneはCSS非表示。focus FHD・実GPUの試験ではない。
- 既存のlight probeでWorker/Canvasのjobと時刻を記録。pixel readbackなし。
  画像内容の正しさを新たに検証する試験ではない。
- 約200 ms間隔のChrome process tree PSS guard 2 GiB、再生wall deadline 180秒。
  guard停止はOOMや製品の拒否ではなく、OSの厳密な強制上限でもない。
- 計測中はgenerator/build/testを併走させない。ファイルSHA256を起動前に読むcache-allowed条件。
  15 GB級の入力は利用可能なfile cacheより大きく、cold/warmは制御していない。
  **harnessの全ファイルSHA256読みはChromeと再生時間の外**であり、製品openのI/Oではない。

[dataset生成条件・SHA256](datasets.json)、[計測環境](environment.json)、
[変更前のbuild](baseline-environment.json)、[現行build](current-environment.json)を保存した。

## 結果

PSSはChrome tree合計、WASMは線形メモリの最大容量。異なる指標なので加算しない。
再生wall時間にはSHA256・Chrome起動・初期openを含めない。

| 入力 | 版 | 結果 / 到達log時刻 | 再生wall | 観測peak PSS | 最大WASM容量 |
|---|---|---|---:|---:|---:|
| none / 8 MiB | 変更前 | 42.93秒でguard停止 | 42.92 s | 2089.0 MiB | 1035.1 MiB |
| none / 8 MiB | 現行 | 59.9秒の終端まで完走 | 60.03 s | 1828.8 MiB | 793.1 MiB |
| zstd / 64 MiB | 変更前 | 2.93秒でguard停止 | 2.93 s | 2181.7 MiB | 1441.4 MiB |
| zstd / 64 MiB | 現行 | 15.14秒でguard停止 | 15.12 s | 2053.1 MiB | 1241.9 MiB |

停止したrunのpeakは完走時のpeakではない。異なる長さのrunから速度倍率や一様な削減率を出さない。
全4 runで観測されたglobal JS exception / unhandled rejectionは0件。

現行noneはbuffer underrun 0、終端は0 windows / 0 retained backing。
60 load requests、1642 reads、source累計14607.5 MiB（約15.32 GB）、per-message copied bytes 0。
これはload診断の値で、pixel queueのcompact JPEG copyは別。
再生中の描画jobは1511件、8台平均約3.15回/秒であり、10 Hz入力の全画像表示ではない。
JPEG await中央値も約63.9 ms/jobで、JPEG throughputの制約は変わっていない。

## 現行noneの時間別メモリ

各10秒log-time帯でPSSが最大のsampleを選んだ。WASM列だけはその帯の最大容量。
GPU processのPSSはVRAM使用量ではない。

| log-time帯 | peak PSS | 最大WASM | peak sampleのGPU process PSS |
|---|---:|---:|---:|
| 0–10 s | 1642 MiB | 784 MiB | 213 MiB |
| 10–20 s | 1608 MiB | 784 MiB | 253 MiB |
| 20–30 s | 1709 MiB | 784 MiB | 354 MiB |
| 30–40 s | 1784 MiB | 793 MiB | 415 MiB |
| 40–50 s | 1762 MiB | 793 MiB | 415 MiB |
| 50–60 s | 1829 MiB | 793 MiB | 415 MiB |

DataPlane backingは各帯の最大でも約731 MiB。記録全体約14.9 GBに比例して保持してはいない。
WASM容量も後半はほぼ一定だったが、process全体のPSSは一定ではない。
GPU process側が増加しており、Chunk/storeだけではprocess全体の推移を説明できない。
これが画像cache、compositor、SwiftShader等のどのallocationかは未分離で、リークと断定しない。
また、1分を超えても増えないという保証にはならない。

現行noneの全体peakはlog-time約53.50秒。
renderer群1151 MiB、GPU process415 MiB、browser等226 MiB、その他約36 MiBだった。
終端後のmain target GC後もPSS約1607 MiB、WASM容量約793 MiBで、store退避と物理メモリ解放は同じではない。

現行zstdのguard時には、storeは2 windows / backing約587.6 MiBだった一方、
WASM容量は約1242 MiB、main JSのWASM以外のbacking差分は約129.5 MiB。
renderer群約1560 MiB、GPU process約248 MiB、browser等約212 MiBだった。
圧縮入力・展開出力の一時確保、allocator高水位、feature-owned backing、browser資源の寄与を
分け切れていない。**4秒版での完走だけでは、大Chunk条件のメモリ問題を解決したと言えない。**

## メモリ単位の訂正

今回コードを確認し、UIが`bytes / (1024 * 1024)`をMBと表示していたことが分かった。
過去の調査でこれを十進MBと解釈した再換算は誤りだったため、2本の調査READMEを訂正した。
例えば前回の「backing 844 MiB」は正しくは885 MiB。
PSS・WASMの比較値には影響しない。過去のraw JSON内の表示文字列は変更しない。
今回UIのラベルもMiBへ訂正した。**上の計測はラベル訂正前のb90288a build**で、
単位ラベル訂正による再生挙動・メモリ計算の変更はない。

## 証跡と再実行

4つの`*.json.gz`が全sample・job・process PSS・Chrome情報を含むraw。
[summary.json](summary.json)と[timeline.csv](timeline.csv)は既存snapshotから再集計できる。
WASM容量はlive allocation量ではなく、DOM・`/proc`・CDPは同じsampling cycleで順次読む非atomic観測。

生成commandはdatasets.json、再生は次の形式で行う。

```sh
python3 -m http.server 8094 --bind 127.0.0.1 --directory apps/viewer-web/dist
# 別terminal。版を切り替える場合は、その版をrelease buildしてから専用Chromeで実行する。
python3 tools/performance-probe/diagnose_web_camera.py \
  --data /tmp/mcap-web-60s-data/3840x2160-8cam-10hz-none-8MiB-60s.mcap \
  --mode light --play-timeout 180 --output /tmp/minute-new/current-none-8MiB-r1.json
python3 tools/performance-probe/summarize_web_minute.py /tmp/minute-new
# このdirectoryのgzip rawから再集計する場合
python3 tools/performance-probe/summarize_web_minute.py docs/performance/2026-10-16-web-minute-playback
```

検証: 実Chrome4試行、版間のdataset SHA256一致、raw JSONとgzipの一致、集計、Python構文、
fmt、ラベル訂正後のviewer-webテスト（57 passed / 2 ignored）とTrunk release build。
先読みやメモリ制御の追加変更は今回行っていない。
