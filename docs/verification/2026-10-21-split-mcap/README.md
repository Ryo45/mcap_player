# 分割MCAP / Web LZ4の検証

`checks.log`は最終Rustコードに対するDockerの`mcap-dev-check`。以下すべて成功:

- workspace fmt、Clippy（all-targets / warnings禁止）、全テスト。
- core/mcap/renderers/viewer-webのWASM check。
- Trunk release build。
- ROS 2 JazzyのNative Liveテスト（80 passed、既存2 ignored）。

`portable-lz4.log`は`cargo test -p viewer-mcap --lib --no-default-features --features mcap-lz4-rust portable_lz4`。
workspaceのfeature統合でNativeのC decoderが選ばれる場合と分け、portable decoder単独でサイズ不一致・header破損・終端不足を確認した。

## 実ブラウザ

既存fixtureをNativeのLZ4 encoderで再圧縮した。元のrecord/payloadを保ち、圧縮形式とChunk構成を変えた相互運用確認である。

~~~sh
target/gpu-native/performance-probe rechunk \
  tests/fixtures/camera-jpeg/camera_7_5s.mcap /tmp/camera-lz4.mcap lz4 65536
python3 tools/performance-probe/check_web_lz4.py \
  --data /tmp/camera-lz4.mcap --output /tmp/web-lz4-new.json
~~~

最終確認は`web-lz4-final-0/1/2.json`と対応PNGの3回。7カメラが描画され、0.8→0.2→0.6の位置へSeekした後、全thumbnailの中央alpha=255、focusの画像hash変化、Ready状態、JSエラーなしを確認した。撮像時刻や各画素の期待frameとの完全一致、性能値を検証するものではない。

最終テストはSwiftShaderと`--disable-accelerated-2d-canvas`を使う。専用Chrome profile・HTTP serverは終了時に停止する。

## 観測上の制限

ヘッドレスのアクセラレートCanvasでは、JPEG workerが完了しcanonical Camera stateとpresentation metadataが揃っていても、canvasの画素が透明になるケースを観測した。`headless-canvas-diagnostic.json/png`に一時観測版の結果を残した。ソフトウェアCanvasでは最終3回とも成功し、元記録を使った対照も`web-original-control.json`に残した。ただし元記録と再圧縮記録はChunk構成も異なるため、codecだけのA/Bとは扱わない。

原因の確定と実GPUでの再現性は未検証。LZ4の全ブラウザ描画経路が検証済みとは主張しない。今回、製品へCanvas無効化やJPEGの推測に基づく回避策は追加していない。一時的なRust状態観測は復元し、通常buildで最終確認した。

`environment.json`にRust source・スクリプト・WASM・fixture・結果のSHA256を保存した。分割記録の境界、ID対応、TF復元、rollback、不要payloadの非読取はRustテストで確認している。
