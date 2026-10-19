# Web camera: focusの鮮度とbackgroundの更新頻度

2026-10-16。JPEG decoderと先読みを変えず、pixel jobの配分を比較した。
待機中のfocusを最優先し、残りをbackground間のround robinにする方式を採用する。
focus専用workerを予約する方式も比較したが、focus Hzがほぼ同じままbackgroundと総処理数が下がった。

## 変更と守る条件

- 変更前は全cameraのround robin。変更後は再生中のpending focusを先に選ぶ。
  focusが実行中または未到着なら、空き枠をbackgroundへ回す。backgroundの巡回位置はfocusのdispatchで進めない。
- 1 cameraあたり実行中1枚・pending最新1枚、active＋ready合計2枚、encoded JPEG合計64 MiBを維持。
  実行中jobは中断しない。focus変更後は空きができてから新しいfocusを優先する。
- 停止中のopen/seek復元は両workerを使った全cameraのround robin。
  seek/source resetのgenerationチェックと未完了jobのカウントを維持する。
- controllerのfocus 10 Hz / background上限5 Hz、2 worker、createImageBitmap、出力寸法、
  MCAP/CDR・window・先読み・メモリ予算は変更しない。Nativeは対象外。
- 処理能力を増やす変更ではない。backgroundの最低Hzを製品として保証する変更でもない。
  decoderやmain threadが止まればfocusもbackgroundも止まる。

## 計測条件

- [60秒fixture](../2026-10-16-web-minute-playback/datasets.json)の非圧縮8 MiB target。
  3840×2160 JPEG、8 camera×10 Hz、Scan 360点×10 Hz、mixed。
  生成duration 60秒、log span 59.9秒、JPEG 4800枚＋Scan600件、14,944,878,140 bytes。
- 固定seed・quality85・32種類の合成JPEGを循環する。実カメラ映像や一般的なJPEG decode性能の代表ではない。
- Browser Local、専用の新規Chrome/profile、headless＋SwiftShader、viewport 1920×1080/DPR=1。
  focus出力960×540、thumbnail112×63、SceneはCSS非表示。focus FHDや実GPUの検証ではない。
- camera 0から開始し、log time約20秒でcamera 3、約40秒でcamera 0へfocusを切り替える。
  定常区間2–18 / 22–38 / 42–58秒を個別に集計する。各区間のHzは測定wall時間で割る。
- Worker/Canvasのjob対応を追跡し、実際に表示用Canvasへ届いた異なるjobを数える。
  focusとそのthumbnailの二重描画は1回。JPEGは保持せず、128点＋byte lengthの識別子を使う。
  fixtureの32 JPEGのSHA256と識別子の一意性を記録する。pixel readbackはこの60秒試験では行わない。
- 画像年齢は、実描画jobのJPEG variant・camera ID・dispatch時のcursorから元のsequenceを推定し、
  表示時cursorとの差を取る。32 frame / 3.2秒周期のfixture専用推定であり、任意の記録に使える計測ではない。
  最新frameへ置換するqueueと10/5 Hzのadmissionを前提とし、推定dispatch年齢−7〜500 ms、
  decode往復3秒未満を確認する。1周期以上古い入力を厳密に検出する方法ではない。
  timeline表示の丸めにより最大約6 msの誤差がある。
- `focus_paint_age_p95_ms`は描画された時点の年齢、`focus_sample_age_p95_ms`は約200 ms間隔で
  表示中frameの年齢を観測したp95。後者は次の更新まで古くなる時間も含むが、短い停止やsamplingの位相偏りを見落とし得る。
- backgroundは各cameraの定常区間Hzの最小値と、5秒log区間ごとの最小Hzを併記する。
  さらに区間をまたぐ隣接paintを含めた最大更新間隔を調べる。平均だけでstarvationを判断しない。
- SHA256による入力全体の読み取りはChrome起動前、計測時間外。cache-allowedでcold/warmは未制御。
  generator/build/testは再生と併走させない。約200 ms間隔のChrome tree PSS guard 2 GiB、wall deadline120秒。
  PSSはsample間のpeakを見落とし得る。WASM容量とPSSは別指標で、合算しない。

## 結果と採用理由

全camera均等、focus専用枠、空き枠を背景にも使うfocus優先の3方式を各2回、順次実行し、全6回が60秒を完走した。
値の範囲はrun×定常区間の最小〜最大。Hzの最小は区間内のcamera別集計で、毎秒の保証ではない。

| 指標 | 変更前：全camera均等 | 専用枠の実験 | 採用：focus優先・空き枠共有 |
|---|---:|---:|---:|
| focus表示 Hz | 3.12–3.19 | 8.93–8.94 | **8.94–9.00** |
| background最低 Hz | 3.06–3.13 | 1.75–1.81 | **2.25–2.31** |
| background 5秒区間の最低 Hz | 2.99 | 1.60 | **2.19** |
| background最大更新間隔 ms | 353 | 608 | **512** |
| focus最大更新間隔 ms | 352 | 133 | **177** |
| focus描画時の推定画像年齢 p95 ms | 241–259 | 173–176 | **210–222** |
| focus表示中の推定画像年齢 p95 ms | 449–514 | 246–259 | **266–303** |
| focus切替→正しい出力寸法の画像 ms | 160–312 | 103–133 | **100–132** |
| 全体のpixel job数 / wall秒 | 24.99–25.18 | 21.66–21.76 | **24.96–25.01** |
| Chrome tree peak PSS MiB | 1798–1838 | 1780–1836 | **1824–1855** |
| peak WASM容量 MiB | 800.2–801.2 | 801.3–805.2 | **795.3–799.6** |

- focusの更新頻度は約3倍になり、backgroundの更新頻度を約3.1 Hzから約2.3 Hzへ譲った。
  backgroundの更新間隔も約0.35秒から最大約0.51秒へ伸びた。この犠牲を含めた改善である。
- 専用枠はfocusの間隔・画像年齢をさらに良くしたが、focus Hzはほぼ増えず、総処理数を約13%落とした。
  backgroundも約1.8 Hzへ下がったため、空き枠を共有する方式を採用した。
- 採用版も10 Hzには達していない。JPEG decoder自体の高速化は別課題である。
  総処理数は約25枚/秒のままであり、デコード能力が向上したという結果ではない。
- メモリは変更前と同程度。PSSのrun間差はあり、削減・悪化の有意差はこの試行数では判断しない。
  2 GiB guardを下回ったことを、長時間や別の圧縮条件でも安全な上限として扱わない。
- 全6回の[raw](raw/)と[区間別・camera別集計](summary.json)を保存した。
  baseline-r1はtimeOrigin未記録、reserved-r1はnavigate前のtimeOriginを記録した初期harnessである。
  この2回は描画時年齢・Hz・gapだけを使用し、表示中年齢と切替latencyは集計から除外した。
  それらの列の比較は修正済みbaseline-r2 / reserved-r2とpriorityの2回に基づく。
  baseline-r1のfocus変更直後に限り、旧harnessのDOM選択表示の遅れによるcamera帰属の不一致を除外する。
  定常集計からは最初から変更前後2秒を除いている。診断値を遡って補正せず、rawをそのまま残した。

## 再現

baselineはa12b97cの通常release build。採用版の変更対象は
`apps/viewer-web/src/camera_bitmap.rs`と`apps/viewer-web/src/app/camera.rs`。
[buildのSHA256](builds.json)を保存した。`priority`は60秒測定版、`final_normal`は
試験用assert増強とfeature試験を終えて戻した最終通常版を区別して記録する。製品の配分処理は同じ。
専用枠の実験は採用版へ[reserved-scheduler.patch](reserved-scheduler.patch)を適用すると再現できる。
`git apply --unidiff-zero`を使う。
これは比較用の差分で、製品に切替設定を追加するものではない。

```sh
python3 -m http.server 8094 --bind 127.0.0.1 --directory apps/viewer-web/dist
python3 tools/performance-probe/diagnose_web_camera.py \
  --mode focus --data /tmp/mcap-web-60s-data/3840x2160-8cam-10hz-none-8MiB-60s.mcap \
  --jpeg-samples /tmp/mcap-web-jpeg-samples --span-seconds 59.9 \
  --play-timeout 120 --output /tmp/focus-new-run.json
python3 tools/performance-probe/summarize_web_focus.py \
  docs/performance/2026-10-16-web-focus-scheduling/raw/*.json.gz --output /tmp/focus-summary.json
```

`--web-dist`は配信するbuildのhash記録用、`--url`は配信先。
fixture生成・JPEG抽出方法は既存の[4K実験](../2026-10-14-4k-large-chunks/findings.md)と
[Web JPEG計測](../2026-10-15-web-jpeg/README.md)を参照する。

## 回帰確認

- 通常release buildの[実ブラウザ確認](seek-check.json)：全8cameraの画像fingerprint、
  個別の前後Seek、連続Seek後のcursorと全cameraの画像、落ち着いた後の旧世代による上書き防止が通過。
- `jpeg-benchmark` featureの[実queue確認](queue-check.json)：focusを開始してもbackgroundの巡回位置を進めず、
  reset前のactive jobを2-job上限から除外しないことを確認。
  pending coalescing、旧世代reject、bitmap寸法、64 MiB backpressure、metadata保持、encoded解放と再admissionも通過。
  計測専用featureを試験後に外し、通常release buildへ戻した。
  復帰後の[8camera画像・Seek確認](final-normal-check.json)も通過。
- `docker compose run --rm dev mcap-dev-check`が通過。
  workspace fmt / Clippy（warningsをerror扱い）/ tests、core・mcap・各renderer・WebのWASM check、
  Trunk release build、ROS 2 Jazzyの`viewer-native --features ros2-live`を含む。
- Python driverと集計scriptの`py_compile`、6 runの成功・worker errorなし・variant識別の検証、再集計が通過。

この比較でzstdのmain-thread展開、巨大Chunk時のmemory、backgroundの絶対的な最低Hz、
長時間のmemory上限、実GPUでの3Dとの併用は解決・保証していない。
