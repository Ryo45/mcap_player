# Native JPEG / FHD表示の比較

2026-10-14。[直前の4K/large Chunk計測](../2026-10-14-4k-large-chunks/findings.md)と同じ4つのMCAPをSHA256で確認し、5条件×3回、合計60 processを順番に実行した。全run成功、どの条件でも8camは179枚、4camは100枚のJPEGをdecodeした。

[summary.md](summary.md)に中央値、[runs.jsonl](runs.jsonl)に個別結果、[environment.json](environment.json)に入力・binary・source hash、実行条件を保存している。
実装とresource boundsは[Native JPEG presentation](../../native_jpeg.md)を参照。

## 結果

8cam × 4K × 10Hz、Zstd 64 MiB target Chunk、4秒の生成記録では:

| 条件 | JPEG処理のwall time | 全処理 | 保持RGBA | peak RSS |
|---|---:|---:|---:|---:|
| 従来Rust RGBA | 7.015 s | 8.389 s | 253.125 MiB | 566.80 MiB |
| TurboJPEG 原寸 | 5.809 s | 7.173 s | 253.125 MiB | 516.75 MiB |
| TurboJPEG 全camera 1080p | 4.681 s | 6.067 s | 63.281 MiB | 279.24 MiB |
| FHD focus + thumbnails、1 thread | 3.942 s | 5.342 s | 11.371 MiB | 227.81 MiB |
| 同じ表示条件、2 workers | 2.705 s | 4.102 s | 11.371 MiB | 245.12 MiB |

TurboJPEG原寸だけではJPEG時間を約17%減らすに留まった。IDCT縮小と2 workersを組み合わせると全処理は約2.05倍速、保持RGBAは約95.5%減。
thread化には1-thread縮小版から約17 MiBのRSS増加があった。grouped条件では約32 MiB増えた。
この入力は4:4:4、品質85のgradient/noise JPEGであり、縮小してもentropy decodeの仕事は残る。

2 workersの全処理中央値は、8cam/無圧縮8 MiBが3.633 s、8cam/Zstd64 MiBが4.102 s、
4cam/20Hz/Zstd64 MiBが3.175 s、8cam/grouped512 MiB targetが4.051 s。
「4秒分の入力を実時間内に処理」できる条件と、まだ届かない条件が残る。
grouped512 MiBはtargetであり、実Chunkは約29.8 MiB。512 MiB実Chunkのresource-limit rejectionを解消した計測ではない。

## 比較の意味

- 最低FHDを基準に、focusへ1920×1080 **device pixels**を要求。ほかは96×72の表示boxで、4K JPEGの1/8 IDCT出力は480×270。
  通常のFHD windowでcamera panelが画面の一部なら、このfocus条件より小さいこともある。高DPRやもっと大きいfocusでは必要なdecode解像度が上がる。
- `rgba` / `native-full` / `native-half` / `native-layout`はsingle thread。`native-layout-workers2`は本番と同じbounded queueを使い、各50 ms tickで全完成を待つ。
  このbarrierで179/100枚という同じ仕事量を維持する。workers2のJPEG msはコピー・待機を含むstage wall timeであり、各threadのCPU時間の合計ではない。
- 本番UIはbarrierを持たず、I/Oとpixel decodeが重なる。遅ければ未開始の画像を最新に置き換える。
  今回の結果だけではGUIのFPS、end-to-end latency、8camera全てを10Hz/20Hzで表示できるとは主張できない。
- 入力は合計80camera messages/sでも、既存のfocused10Hz/background5Hz admissionを通した比較である。
  全camera全入力を原寸decodeする性能の比較ではない。4秒はfixtureの生成durationで、最終sampleは8camで3.9s、4camで3.95s。
- cold/warm cacheを制御せず、最初のSHA読みにもcache効果がある。OS storage read bytesは各runに記録した。
  特に無圧縮`native-full`のread中央値180 msは他条件の約850 msと異なる。全処理差をcodecだけの効果と解釈しない。
  JPEG stageの比較と、独立したI/O/展開の測定を併記する。
- peak RSSはCPU process全体。保持RGBAの数値はGPU VRAMではない。Canonical CameraState、MCAP backing、allocatorもRSSに含まれる。
  1.5 GiB RSS guard / 180s deadlineを使い、計測中に自分で開始したGUIやbuildを併走させなかった。

## 再実行

CMake/NASMを用意し、前回と同じfixtureを生成・保存しておく。SHAが違う場合はこのscriptは停止する。

```sh
cargo build --release -p performance-probe --locked
python3 tools/performance-probe/measure_native_jpeg.py \
  --baseline docs/performance/2026-10-14-4k-large-chunks \
  --data-dir /tmp/mcap-stress2-20261014-data \
  --output /tmp/native-jpeg-comparison-new
```

Source hashとbinary hashは計測時のworktreeのもの。`git_commit`は変更前のparent commitであり、この結果がそのparentの未変更codeで測られたという意味ではない。
次の確認対象は、本番UIのcamera完成遅延・coalescing枚数と、I/O/展開とJPEGの重なり。
threadを増やすかはその結果と、一時RGBA/encoded backingのメモリ増加を見て判断する。
