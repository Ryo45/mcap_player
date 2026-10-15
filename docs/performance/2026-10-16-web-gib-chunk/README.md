# 1 GiB Chunkの試験とzstd展開の実行スレッド

2026-10-16。[60秒ログ](../2026-10-16-web-minute-playback/README.md)と同じmessage集合を
zstd level3・1 GiB targetへrechunkした。**Webは2回とも独立PSS guardで停止、Nativeの順次読み込みは既存上限で拒否した。**
製品の上限・先読み・スレッド構成は変更していない。計測ツールのrechunk上限だけを512 MiBから1 GiBへ広げた。

## zstdはどのスレッドで展開しているか

共通の`viewer_mcap::decode_mcap_chunk`は同期APIで、`zstd::bulk::decompress`を呼ぶ。
内部に専用thread poolを持たず、実行場所は呼び出し側が決める。

| 経路 | zstd展開の実行場所 | 根拠 |
|---|---|---|
| Web / Browser Local | ブラウザのmain thread | local loaderの`spawn_local` → File APIをawait → `collector.insert_chunk` → 同期decode |
| Native / 通常Forward再生 | UI/event-loop thread | `App::redraw` → `advance_session` → `McapPlayback::tick` → `read_through` |
| Web / Remote | Recording ServerのTokio blocking pool | HTTP handlerがpermitを取り`spawn_blocking`内で`read_batch` / `read_restore` |

`spawn_local`は別threadではない。I/O待ちの間はyieldするが、await後のChunk展開・parseは同期処理。
一つの大きなChunkを処理している間は、WebのUI処理やJPEG完了通知の受信も待たされる。
JPEGのWeb Worker 2個、およびNativeのJPEG workerは、このMCAP zstd処理とは別である。
NativeのInspector/Plot等のbackground queryまでUI threadで実行するという意味ではない。

参照: [Browser loader](../../../apps/viewer-web/src/local/loader.rs)、
[共通Chunk codec](../../../crates/viewer-mcap/src/recording/chunk.rs)、
[Native redraw](../../../apps/viewer-native/src/app.rs)、
[Native playback](../../../crates/viewer-mcap/src/playback.rs)、
[Recording Server API](../../../apps/recording-server/src/api.rs)。

## 入力

4K JPEG、8 cameras×10 Hz、Scan 10 Hz、mixed、名目60秒 / log-time span59.9秒。
画像4800件＋Scan600件＝5400件。32種類の合成JPEGを循環する条件は前回と同じ。

- 目標Chunk: 1,073,741,824 bytes = 1 GiB。
- 実最大uncompressed Chunk: 1,074,451,658 bytes = 約1024.7 MiB。
  writerがrecord境界で切るため、目標を少し超える。
- Chunk数14、ファイル14,812,776,927 bytes。
- 全Chunkのuncompressed bytesは14,944,459,940で、元入力・64 MiB版と同じ。
- `rechunk`は正常終了し、Summaryのmessage countと5400件の一致も確認した。
  作成に約61秒、generator最大RSSは約25 MiB。作成時に1 GiB分のmessageをRAMへまとめて保持する方式ではない。

[rechunk結果](rechunk.json)、[生成log](rechunk.log)、
[入力SHA256・binary/source/build hash](environment.json)を保存した。

## Webの実測

現行45c17faの通常release build、Browser Local、headless Chrome＋SwiftShader。
viewport1920×1080/DPR=1、focus960×540、thumbnail112×63、Scene非表示。
この極端な入力ではmain thread上のCDP応答自体が待たされる可能性があるため、
ページを読まずに`/proc`からChrome tree PSSを測る親processを別に置いた。

- Chrome-only PSS guard: 2 GiB、約100 ms間隔。超過を観測したら専用scopeへTERM。
- probe＋Chromeのcgroup: `MemoryMax=3G`、`MemorySwapMax=0`。監視親はscopeの外。
- cgroup memoryにはPythonやcharged file cacheも含まれ、Chrome PSSとは別の指標。
- SHA256を事前に読むcache-allowed条件。生成・buildを再生処理と併走させない。
- 64 MiBまでの旧計測はCDP応答後にPSSを読んでおり、監視方式が異なる。
  今回の停止時刻や観測peakから、64 MiB版との速度比を計算しない。

| 試行 | 結果 | 観測peak Chrome PSS | うちrenderer群 | OOM kill |
|---|---|---:|---:|---:|
| 1 | external PSS guard停止 | 2240.0 MiB | 1982.3 MiB | 0 |
| 2 | external PSS guard停止 | 2150.8 MiB | 1886.8 MiB | 0 |

どちらもChrome初観測から約1.9秒後にguard停止した。これはopen/展開時間そのものではない。
正常再生・画像Ready・cursor位置は確認できておらず、初期ロード付近で監視が止めたという結果。
製品が自分で1 GiBを拒否した、またはzstd展開が完了した、とは判定できない。
ページ内のprocessing時間やWASM内訳も取得できていない。

PSS guardはsamplingなので厳密な2 GiB上限ではない。3 GiB OS上限は保険で、両試行の
`memory.events`は`oom=0 / oom_kill=0`。OS OOMによる停止ではなかった。
[summary.json](summary.json)、`run-1/external.json`、`run-2/external.json`に独立監視の全sampleを保存した。

1回目はTERMでprobeも終了し、ページ側のJSONが保存されなかった。
2回目は入力identityを先にcheckpointするよう計測器を補強してから再実行した。
`run-2/browser.json`の`starting-browser`は**最初のcheckpointのまま**で、実際の停止phaseを示さない。
最終判定は外部監視の`external-pss-limit`とし、完走扱いしない。

小Chunk入力で同じ監視wrapperを先に試し、通常終了とChrome PSSの採取を確認した。
[preflight-summary.json](preflight-summary.json)に概要を保存した。

## NativeとWebの上限の違い

Native ForwardをCLIで同じfileへ通すと、exit 1、
`query exceeds forward overlapping chunk backing bytes limit (268435456)`となった。
[stderr](native-rejection.txt)に保存した。GUI/GPUを起動した比較ではない。
Nativeはactive Chunkをstageする前にuncompressed sizeを見て、256 MiBを超える要求を拒否する。

一方、Web local forwardの`chunk_request`はサイズの整数変換を確認するだけで、
Nativeと同じ256 MiBのcompressed/uncompressed Chunk上限はない。
256 MiBのwindow eviction budgetも、これから読む一つのChunkの確保を止めるものではない。
Seek/restore側には別のquery work budgetがあるため、通常再生と区別する必要がある。

現在のWeb経路は、JS ArrayBuffer → WASM入力copy → bulk展開出力の確保を行う。
1 GiB級ではこれらの一時的な共存が大きくなり得る。ただし、どの確保段階で今回guardへ達したかは未特定。
このメモリ予算のまま1 GiB Chunkを実用的に開ける、という結果にはならなかった。

WorkerへMCAP処理を移せばUIの長い同期処理を切り離せるが、bulk展開に必要なbacking量は残る。
今後はUI responsivenessの改善と、大Chunkのmemory/copy/streaming方針を分けて検討する必要がある。
今回その設計変更や上限引き上げはしていない。

## 再実行と検証

Linuxのuser systemdとcgroup v2、既存のWeb HTTP配信8094を使う。

```sh
cargo build --release -p performance-probe --locked
systemd-run --user --scope -p MemoryMax=3G -p MemorySwapMax=0 \
  target/release/performance-probe rechunk \
  /tmp/mcap-web-60s-data/3840x2160-8cam-10hz-none-8MiB-60s.mcap \
  /tmp/mcap-web-60s-data/new-zstd-1GiB.mcap zstd 1073741824
python3 tools/performance-probe/run_web_large_chunk.py \
  --data /tmp/mcap-web-60s-data/new-zstd-1GiB.mcap --output /tmp/web-gib-new
```

検証: fmt、performance-probe全targetのclippy、同crateの5 tests、release build、
Python構文、通常入力の監視wrapper、1 GiBの実入力2試行、Native resource rejection。
製品Rust/JSは変更していないため、workspace全体やWASMの再テストは今回は行っていない。
