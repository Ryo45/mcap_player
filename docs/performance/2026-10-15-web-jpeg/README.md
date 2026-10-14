# Web JPEG / FHD比較

2026-10-15。旧Web経路のWASM JPEG → 原寸RGBA → ImageData/Canvasと、Dedicated Web Workerの`createImageBitmap` → Canvasを比較した。
[Nativeの比較](../2026-10-14-native-jpeg/README.md)で使ったMCAPから、`/camera/0`の先頭32 JPEGを抽出して使う。
fixtureは3840×2160、quality85、4:4:4のgradient/noise。各JPEGのSHA256は[runs.json](runs.json)にある。

## 結果

Chrome headless、1920×1080、DPR=1。同じ固定179 jobsを各条件3回実行した中央値。
focusは1920×1080 pixels、他cameraは160×90の出力。元画像より拡大しない。

| 条件 | JPEG＋base Canvas wall time | 10 ms heartbeatの最大間隔 | base Canvas保持量 |
|---|---:|---:|---:|
| 旧WASM RGBA | 16.242 s | 226.0 ms | 253.125 MiB |
| Bitmap原寸、1 worker | 10.842 s | 27.1 ms | 253.125 MiB |
| FHD focus＋thumbnails、1 worker | 11.502 s | 11.9 ms | 8.295 MiB |
| FHD focus＋thumbnails、2 workers | 6.799 s | 13.7 ms | 8.295 MiB |

2 workersは旧経路比で約2.39倍速。Canvasの寸法×4で求めた保持量は約96.7%減。
UI timerの最大間隔は大きく短くなった。一方、1 workerの縮小は原寸より約6%遅く、
縮小出力を要求するだけでJPEG decodeの仕事が減るとはいえない。NativeのIDCT縮小と同じ効率を仮定しない。

## 測った範囲と限界

- 32 variantsを循環する179 jobs。camera0は40枚、camera1〜6は各20枚、camera7は19枚。
  4秒・focused10Hz/background5Hzを参考にした**固定worklist**であり、Nativeのadmission traceそのものではない。
  全条件が同じworklistを使う。間引いて速くした比較ではない。
- MCAP I/O、Chunk展開、CDR、FeatureRuntime、Rust queue admission、visible DOMのoverlay合成、3D Sceneを性能値へ含めない。
  worker APIとJPEG＋base Canvasのcomponent比較であり、8cam×10Hz全入力の実時間再生を証明しない。6.8秒というpixel処理量にもまだ限界がある。
- 2 workersも同じcameraのjobを同時実行しない。各batchの間にmacro-taskでyieldし、旧WASMのmicrotask連続実行でtimerを不当に飢餓させない。
  `workMs`はbatch内のwall timeの合計で、人工的yieldの待機は除く。yield込みの`elapsedMs`もraw結果に残した。
- 10 ms heartbeatの最大**間隔**であり、その値をそのままmain-thread CPU占有時間と解釈しない。job P50/P95も各runに保存した。
- fixtureはあらかじめJSへfetch/SHA検証した。旧経路の計測exportはJSからWASMへJPEGを渡すため、入力copyを含む。
  実製品では入力がWASMにあり、新経路にはRustのcompact copy/header/admissionもある。componentの境界差を含む比較である。
- 同じpageでmode順をrotateして3回測った。fresh processやcold cacheの比較ではない。WASM linear memoryはhigh water markが次modeにも残る。
- Canvas bytesは**総RSS、GPU VRAM、browser内部decode作業領域の実測ではない**。bitmapはcloseするが、browserのGC/cache挙動もある。
  buildや別の自分のGUIテストを計測中に走らせなかった。
- header-only寸法取得、EXIF除去、production queueのgeneration/coalescing/2-job上限/参照解放、破損後の正常decodeは実ブラウザで別途assertした。
  EXIF Orientation=6の画像と元画像が同じpixelsになることも確認した。`imageOrientation:none`だけではこのChromeの回転を防げなかった。

[summary.md](summary.md)、[runs.json](runs.json)、[environment.json](environment.json)を保存した。
`parent_commit`は未変更baselineの測定という意味ではなく、測定したworktreeのparent。source / benchmark WASM hashesを併記する。
[gui-check.json](gui-check.json)は通常buildの7camera fixture、FHD、focus変更と連続前後seekの結果。
カメラ・overlayにJS例外はなかった。headlessではWebGPU readyになったが3D領域が黒く、headfulではadapterを取得できなかったため、3D実描画は未検証。

## 再実行

```sh
cargo build --release -p performance-probe --locked
# OUTPUT_DIRは新規ディレクトリ。
target/release/performance-probe extract-jpeg \
  /tmp/mcap-stress2-20261014-data/3840x2160-8cam-10hz-none-8MiB-jpeg.mcap \
  /tmp/web-jpeg-samples-new /camera/0 32
python3 tools/performance-probe/prepare_web_jpeg_benchmark.py
docker compose run --rm dev bash -lc 'cd apps/viewer-web && trunk build .jpeg-benchmark.html --release'
python3 tools/performance-probe/prepare_web_jpeg_benchmark.py --samples /tmp/web-jpeg-samples-new
python3 -m http.server 8094 --bind 127.0.0.1 --directory apps/viewer-web/dist
# 別terminal: 専用のChromeと新規profileを使う。
google-chrome --headless --no-sandbox --enable-unsafe-webgpu \
  --remote-debugging-port=9235 --user-data-dir=/tmp/web-jpeg-profile-new about:blank
python3 tools/performance-probe/measure_web_jpeg.py 9235 /tmp/web-jpeg-result-new.json
```

計測用exportは`jpeg-benchmark` featureだけに存在する。`data-cargo-features`付きの一時HTML entryを生成する。
通常配布には`trunk build --release`を使い、一時`.jpeg-benchmark.html`は削除する。
通常buildのカメラ回帰確認はfixtureをdistへ`gui-fixture.mcap`としてコピーし、同じdriverへ`--camera-check`を付ける。

実装とbyte budgetの制限は[Browser JPEG presentation](../../web_jpeg.md)を参照。
