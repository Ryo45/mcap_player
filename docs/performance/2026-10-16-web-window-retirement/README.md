# Webの消費済みwindow解放とメモリ内訳

2026-10-16。[前日の原因調査](../2026-10-15-web-camera-diagnosis/README.md)を受けた改善。
採用した製品変更は、Web DataPlaneが**消費済みserialized windowを配送時に解放する**ことだけ。
先読み方針、1秒window、2秒ahead、256 MiB予算、JPEG処理、Nativeの実装は変更していない。

## 採用した変更

従来のbudget適用はload完了時だけだった。cursorがwindowを通過しても、次のloadが終わるまで
古いwindowが残り、新しいbacking確保と重なり得た。最後のload後には終端まで進んでもwindowが残った。

`RecordingDataPlane::messages_through`で、配送完了範囲のwindowを退避する。
最終のinclusive timestampを配送した場合は、最後のexclusive windowも退避する。
返却した`RawMessage`の`Bytes`はそのまま共有し、featureへ渡すpayloadをcopyしない。
完全性のhorizonは後退させず、次のrequestは元のexclusive境界から続く。

既存のstore APIで解決できるため、新しいcache/manager、crate分割、loader変更は不要だった。
windowより細かいbacking解放やJPEG展開の改善は今回行っていない。

## 変更前後の再計測

同じhost・Chrome・fixture・通常release buildで、変更前を各条件1回、採用版を各2回計測。
専用Chrome/profileを毎回作り、計測中にbuild/testを併走させない。
cache-allowed、viewport 1920×1080/DPR=1、focus 960×540、thumbnail 112×63、Scene非表示。
headless Chrome＋SwiftShaderであり、focus FHDや実GPUの性能を主張する試験ではない。

`diagnose_web_camera.py`のlight計測。採用版は中央値、変更前は1試行。

| 条件 | 変更前の結果 | 採用版の結果 | peak PSS、前→後 | 最大WASM容量、前→後 | 再生時間、前→後 |
|---|---|---|---:|---:|---:|
| none / 8 MiB | 完走 | 2/2完走 | 1767→1518 MiB | 1004→772 MiB | 4068→4103 ms |
| zstd / 64 MiB | PSS guard停止 | 2/2完走 | 2090→1876 MiB | 1308→1047 MiB | 打ち切り→3980 ms |

約200 ms間隔のChrome tree PSS guardは2 GiBのまま。厳密なOS上限ではない。
変更前のzstdは途中停止のため、完走時peakとの公平な上限比較や速度比較はできない。
採用版の診断4 runは全て終端で0 windows / 0 retained backing、buffer underrun 0。
WASM容量は解放済み領域も含み、終端やGCで縮まない。

JPEG await時間は引き続き約63〜65 ms/jobで、約3 Hzの表示更新という制約は残る。
この変更はメモリ保持期間の改善であり、JPEG throughput改善ではない。

rawは[baseline/](baseline/)、[final/](final/)。各directoryのsummary.jsonは既存集計scriptで生成した。
`git_parent`は63676f0で、採用版はそのworktree変更を含む。ビルド資産とsourceのhashは
[environment.json](environment.json)に保存した。

## メモリは何に使われるか

追加の1試行でChrome各processのPSSを収集した。[memory/zstd64.json](memory/zstd64.json)、
[集計](memory/summary.json)に保存した。以下はそのpeakの**同じsampling cycle**の値。
DOM、WASM、`/proc`、CDPの収集は順次行うためatomic snapshotではない。

| Chrome内のprocess分類 | PSS |
|---|---:|
| renderer合計（予備renderer等も含む） | 1464.6 MiB |
| browser等、`--type`のないprocess | 205.7 MiB |
| gpu-process | 178.2 MiB |
| zygote | 25.9 MiB |
| utility | 6.1 MiB |
| 合計 | 1880.5 MiB |

これはprocessごとの加算可能な内訳。GPU processのPSSはGPU VRAM使用量ではない。
全sampleで個別PSSの合計がtree PSSと一致することを検証した。

以下は上とは別の、重なる指標である。

| アプリ内で観測できるもの | 量 | 解釈 |
|---|---:|---|
| DataPlane logical payload | 約679.6 MiB | 画面表示712.6 MB。3 windows内の選択streamのserialized message |
| DataPlane retained backing | 約844.1 MiB | 画面表示885.1 MB。Chunk単位の余剰やwindow間の読み直しを含む |
| WASM線形メモリ容量 | 1046.6 MiB | DataPlaneを含む。allocatorのlive/free領域は未分離 |
| main JS used heap | 約1.1 MiB | ArrayBuffer等の外部領域は別 |
| main JS backing − WASM容量 | 約100.2 MiB | 外部backingの差分。個々の用途への帰属は未確認 |

大きい保持元は、decode済みの表示履歴ではなく、先読みwindowの**serialized JPEGを含むChunk backing**。
3 windowsで約713 MBのmessageがあり、backing単位の保持でさらに約173 MB増えている。
表示対象が8カメラなら上流のtopic選択でも8台分を読む。focus/backgroundのpixel decode頻度制限は、
このserialized windowの量を直接減らす仕組みではない。

変更前後でpeak時のstore表示が同じ885 MBでも、古いwindowと次のloadの一時的な重なりは異なる。
早期退避でこの重なりを短くし、WASMの高水位を減らせたと解釈できる。
ただしallocatorのallocation traceを取ったわけではなく、各byteの生存期間を完全に追跡してはいない。

表示解像度から計算したbase canvas＋visible canvasのRGBA相当量は約4.36 MiB。
これは`width × height × 4`の**算術値**で、Canvas実測割り当てやGPU texture数ではない。
encoded pixel queueは64 MiB上限だが、今回その時々の実使用量は測っていない。
canonical Camera stateはChunk-backed JPEGを共有し得るため、別に加算すると二重計上になる。

未分離なのは、WASM内のlive/free/allocator overhead、featureに残るbacking、JPEG worker内部の
入力/Blob/展開作業領域、ImageBitmap・Canvasのnative資源、GPU内部cache等。
`createImageBitmap`が内部で4K RGBAを何枚確保したかは、この計測からは断定できない。
main target GC後でもPSSは約1579 MiB、WASM容量は1047 MiBであり、window数0をprocess memory 0とは扱わない。

従って、**大きな保持元とprocess別使用量は把握したが、全割り当ての用途別内訳はまだ把握していない**。
WASM・CDP backing・process PSSを足したり、差分を全て画像メモリへ帰属させたりしてはいけない。

## 画像内容とSeekの確認、残る制限

既存のfingerprint stressを変更せず実行した。[stress/summary.md](stress/summary.md)と各rawに保存。

- none8は3/3成功。12個別Seekと3連続Seekで8カメラの正しい画像を確認し、その後の差し戻しも観測しなかった。
  再生3909 ms、個別Seek396 ms、連続Seek537 ms、peak PSS1587 MiBが成功run中央値。
- zstd64の2本は通常再生と4個別Seekを通過したが、連続Seekの最終要求へ復帰せずtimeoutした。
  peak PSSは1920 / 1981 MiBで、今回はmemory guard停止ではない。
- 残るzstd64の1本はharness起動中のCDP node取得に失敗し、記録を開く前に終了した。
  入力処理の失敗とは区別して保存した。zstd64 stressを成功扱いしていない。
- global JS exception / unhandled rejectionは、入力を開けたrunでは0件。

[採用しなかった案の試行](rejected-prefetch/stress-zstd64-0.json)では
`browser indexed query exceeds chunk byte budget`も観測した。
restore/queryは今回変更していないが、最終版の連続Seek timeoutと同じ原因かは分離できていない。
このfixtureの連続Seekまで解決したとは主張しない。queryの上限引き上げも行っていない。
退避だけの改善で問題全体が解消したわけではなく、採用根拠は通常再生のメモリ改善と既存テストの通過である。

## 採用しなかった先読み変更

最初にbyte pressureで任意の先読みを止める案も試したが、利用者の指示に従って全て取り消した。
その試行ではzstd64のpeak PSSは1681 MiBに減った一方、3.9秒分の再生が5459 msとなりunderrunが4回発生した。
[試行raw](rejected-prefetch/zstd64-light.json)、[stress集計](rejected-prefetch/stress-summary.md)は採用版とは分けて保存。
先読み案のsource変更はコミットに含めない。

## 検証と再実行

追加のRust回帰テストは、load完了がなくても消費済みwindowを解放すること、境界messageを一度だけ渡すこと、
返却messageのpayloadが有効なこと、退避後もcompleteUntilと次のrequest境界を保つこと、終端の最後のwindowを解放すること。
既存のSeek失敗・古いgeneration・continuation・Local/Remote parityのテストも通過した。

`docker compose run --rm dev mcap-dev-check`がexit 0:
fmt、workspace/all-targets clippy、workspace tests、WASM check、Trunk release、ROS 2 JazzyのNative ros2-live tests。
[checks.txt](checks.txt)にtest結果の抜粋を保存。Python samplerの構文チェックと、実Chromeでのprocess別PSS照合も実施。

再実行は前日の同じfixture・HTTP 8094・JPEG参照値を使う。

```sh
python3 tools/performance-probe/diagnose_web_camera.py \
  --data /tmp/mcap-stress2-20261014-data/3840x2160-8cam-10hz-zstd-64MiB-jpeg.mcap \
  --mode light --output /tmp/web-retirement-new/zstd-64MiB-light-r1.json
python3 tools/performance-probe/run_web_camera_stress.py \
  --data-dir /tmp/mcap-stress2-20261014-data \
  --references docs/performance/2026-10-15-web-camera-stress/references.json \
  --output /tmp/web-retirement-stress-new
```
