# Native Camera: fixed-period admission experiment

2026-10-20。Lichtblickとの比較で見えたNativeの画像遅延について、まず受信から描画までの待ち時間を分け、CameraControllerの周期のずれを修正する実験。

## 結果と採用判断

固定周期への変更を採用する。変更前・変更後を交互に各3回測った結果は次の通り。細かい段階別traceはこの6試行では無効にした。

| 指標（各runの値の中央値） | 変更前 | 固定周期 |
|---|---:|---:|
| 観測した画像更新 Hz（下限） | 9.78 | 9.86 |
| 画像時刻の前進 Hz | 9.86 | 9.86 |
| 画像の遅れ 中央値 / P95 ms | 165 / 217 | 157 / 170 |
| サンプル最大PSS MiB | 260.2 | 269.4 |
| CPU %（1 core = 100%） | 40.8 | 39.8 |

[全集計と段階別診断](summary.md)、[各反復・範囲・trace](summary.json)、[実行順](manifest.json)、[binary等の識別情報](environment.json)を保存した。P95欄は各runのP95の中央値であり、全サンプルをpoolしたP95ではない。

- P95は約48 ms小さくなった。変更前は3回とも約217 ms、変更後は167 / 170 / 203 ms。中央値の差は約9 msで観測周期50 msより小さく、平均的な鮮度の大幅な改善とは主張しない。
- 更新は約10 Hzを維持した。曖昧な画像識別で除外したサンプルは双方2/720。各runの4回のSeek / 先頭復帰、計24回で期待する画素とNative textureのpredecessor時刻を確認した。
- CPUはほぼ横ばい。PSSは変更前260.1〜264.1 MiB、変更後254.2〜269.5 MiB。中央値は約9 MiB増えており、「メモリも改善」とは扱わない。計測対象は新規履歴やcacheを加えない同じbounded pipelineだが、この短い3反復だけではPSS差の原因は切り分けられない。
- 別のtrace各1回では、受信→採用のP95が50.09 msから0.03 ms、受信→UIへのtexture配置が93.22 msから50.03 msとなった。一方JPEG展開には約30〜33 msを使っており、その処理は変更していない。trace間のJPEG時間の差まで周期修正の効果とは帰属しない。

この結果は「待ちの大きい場面を減らす」改善を支持する。Lichtblickとの全面的な同等性能や、8カメラ同時表示の改善を証明するものではない。次にCPU / 完了待ちを詰めるなら、worker完了通知と更新のない描画の削減を組み合わせて検討する。JPEG worker増設や先読み追加は今回行わない。

## 問題と変更

変更前は画像を採用するたびに `next = now + interval` としていた。100 msの締切を107 msで処理すると次回は207 msとなり、200 msで届いた画像はその時点では採用されない。遅れを次の締切へ持ち越すことで周期が徐々にずれる。

変更後は元の周期に沿って次の未来の締切を選ぶ。107 msで処理しても次は200 ms。長いstallの後は失った周期を飛ばし、coalesce済みの最新1枚だけを採用する。背景Cameraの初期位相を維持し、focus変更とrestoreは従来の明示的な再設定を使う。100 / 200 msは周期の予算であり、隣接する採用の厳密な最小間隔ではない。近接する別周期の採用は起こり得るが、過去の画像をcatch-up decodeしない。

採用前のserialized coalescing、2 JPEG workers、入力の64 MiB上限、completionの上限、IDCT scaling、GPU texture再利用、Seekの世代破棄は変更しない。新しい履歴や先読みは加えない。共有viewer-coreの変更なのでNative / Web双方に適用される。

## 計測方法

[前回の実GPU比較](../2026-10-20-native-lichtblick-gpu/README.md)の同じ環境・MCAP・pixel observerを使う。RTX 5070 Ti、NVIDIA 610.57.04、X11 / DPMS On / locked session、1920×1080ウィンドウ、960×540画像、DPR=1。約14.94 GB、60秒、4K・8カメラ×10Hzの記録から `/camera/0` だけ表示する。圧縮なし、mixed、目標8 MiB Chunk。各runの連続再生は12秒、25.31 / 48.71 / 16.11秒へのSeekと先頭復帰を確認する。8カメラ同時表示や60秒全期間再生の試験ではない。

比較対象はbaseline `4132f5d` と [cadence-change.patch](cadence-change.patch) を適用した版。アプリと子プロセス全体のPSS / CPU、実ウィンドウの画像識別、wall timeと画像時刻の差を測る。撮影から画面走査までの遅延ではない。cache-allowed、各runでMCAP全体のSHA256を読んでから開始する。共有ホストであり、他用途のGPUプロセスは停止しない。

最初に各版1回、段階別traceを有効にして診断する。その後、細かいtrace出力を無効にし、baseline / periodicを交互に各3回、新しいプロセスで計測する。両版に同じ計測用フックを入れ、binary / dataset / patch / harnessのSHA256をresult.jsonに残す。計測用フックは製品Rustには残さない。

### 段階別traceの意味

- `received`: CameraControllerがRawMessageを受信。MCAP I/Oより後の境界である。
- `admitted`: pendingから最新のserialized画像を選択。これからCDRをdecodeする。
- `submitted`: JPEGをcompactに所有したworker requestを登録。重複submitは記録しない。
- `started` / `decoded`: workerでJPEGを展開する前後。
- `uploaded`: UI側でtexture更新をenqueueした後。GPUの転送完了を待った時点ではない。
- `painted`: 新しいtextureをUIへ置いた時点。surface present / monitor scanoutの完了ではない。実画素の観測は別のpixel observerが行う。

SystemTimeのnanosecondsを同じプロセス内で対応付ける。clock補正が入る環境への一般的な計測APIではない。再生0.1〜10.9秒の109画像を対象に、同じCameraId / arrivalの最後のtraceを結合する。先行する遠いSeekは区間外、0秒の初期化・サイズ変更・先頭復帰は除外する。各段階の中央値を足してend-to-endの中央値とみなさない。

### 再現

計測用レイアウトと操作注入は前回と同じ。追加のtraceと周期修正を混同しないよう、baseline / periodicそれぞれのmeasurement patchを保存する。baselineのパッチは4132f5dへ、periodicのパッチは先にcadence-change.patchを適用したソースへ適用する。ビルド後はmeasurement patchを戻す。

```bash
# baseline source + baseline-measurement.patch
cargo build --release -p viewer-native --locked
# Save as target/gpu-native/viewer-native-cadence-baseline.

# baseline source + cadence-change.patch + periodic-measurement.patch
cargo build --release -p viewer-native --locked
# Save as target/gpu-native/viewer-native-cadence-periodic.

python3 tools/performance-probe/run_native_camera_cadence.py \
  --data /tmp/mcap-web-60s-data/3840x2160-8cam-10hz-none-8MiB-60s.mcap \
  --jpeg-dir /tmp/mcap-gpu-comparison-jpegs \
  --output /tmp/mcap-native-camera-cadence-new
```

個別のtrace診断は`compare_native_desktop.py --camera-trace --native ... --native-patch ...`を使用する。ログの解析スクリプトと集計を同梱する。前回と同様、画像32種類は反復するため絶対時刻を画素だけで一意に証明するfixtureではない。観測20Hz、識別が曖昧なサンプルの明示的除外、Seekの異なる注入境界等の制限も引き継ぐ。

## 検証

- 変更前にtick jitterの回帰テストが失敗することを確認（200 ms時点で100の画像が残り、200が採用されない）。変更後に成功。
- 遅延tick、stall中のlatest-only / catch-up抑制、背景の位相、focus / restoreの再設定、Duration::MAXでの算術を含む5つの回帰を追加。
- `docker compose run --rm dev mcap-dev-check` が終了コード0。fmt、workspace Clippy（warnings禁止）、workspace tests、WASM target check、Web release build、sourced Jazzy環境のNative `ros2-live` testsが通過。既存のignored testsはそのまま。出力は[checks.log](checks.log)。
- 計測専用コードはmeasurement patchとしてのみ保存。製品RustにMCAP_BENCH / CAMERA_TRACEを残していない。

計測用フックなしの製品release buildも、README fixtureでCamera / BEV / Scene、Pauseの保持、Resume、ResizeをNVIDIA Vulkan上で確認した。[実行記録](production-smoke/result.json)と代表画像を保存し、Pauseの2画像が同じ1.774秒を保持することとResumeの進行を目視確認した。
