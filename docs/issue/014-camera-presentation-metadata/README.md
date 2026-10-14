# 表示済みcameraのmetadataが圧縮JPEGを引き留める

状態: 対応済み（2026-10-15）。Native/Web JPEG非同期化のレビューで確認。
以下の「課題と調査」は対応前の状態を記録している。

## 課題と調査

Native `Graphics::presented_camera_frames`とWeb `CameraViewState::presented_frames`は、
overlayのcamera ID / measurement time / arrival / frame IDを保つために`CameraFrame`を保持する。
`CameraOverlayState::update`→`CameraCalibrationSet::project_plan`のAPIがCameraFrame全体を要求するが、投影はJPEG自体を使わない。

worker用のcompact JPEGも一緒に残り、64 MiBのencoded budgetを使い続ける。
本計測の8camera JPEGは表示済み合計約24 MiBなので次のworkを受け入れられた。
しかし表示済み合計＋新しい1枚が64 MiBを超える条件では、pending/in-flightを空にしても新しいworkを開始できない。
これは単なる一時backpressureではなく、source/seek resetまでpixel更新が止まり得る。
同じJPEGの原寸/縮小pixelサイズには依存しない。

## 最小の解決方向

投影とpixel cacheのkeyに本当に必要なmetadataだけを表示済みstateに保持する。
`CameraFrame`はfeature stateのJPEG所有者として維持し、GPU/Canvas upload完了後にworker側のcompressed copyを解放できるようにする。
shared projection APIが圧縮payloadを要求しない形へ改め、Native/Web両方のcallersを更新する。
偽の空JPEGを入れたCameraFrame、無計上の追加JPEGコピー、budget拡大で回避しない。
必要な小さいmetadata型は検討対象だが、generic frame managerやprofileは不要。

## 検証

- 表示後のencoded budgetが解放され、新しいframeをadmitできること。
- metadataが表示中imageのarrival/measurement time/frame IDと一致すること。
- source/seek generation、resize、古いcompletionの破棄を維持すること。
- large JPEG fixtureまたは所有権テストで、表示済みpayloadを保持しないことを確認すること。

## 実装

viewer-coreにCameraFrameMetadata（camera ID / measurement time / arrival time / frame ID）を追加した。
投影API、Native/Webの完成結果と表示済みstateはこの型を使い、JPEGを保持しない。
CameraBaseImageTrackerにはcache keyであるcamera IDとarrivalだけを渡す。
canonical CameraFrameは従来どおりJPEGを持ち、resize時にも利用できる。

JPEGコピーはGPU/Canvas uploadを待たず、デコード完了時点で解放する。
64 MiB上限は待機・実行中のcompact JPEGコピーに適用する。
完成pixelsや表示済みmetadataが次のframeのadmissionを永久に妨げる状態を解消した。
worker数、generation、source/seek、resize、元画像座標の扱いは維持する。

## 回帰検証

- core: metadataへ変換後、metadataを生存させたまま元JPEG backingのDropを確認。
  nanosecond精度の両timestamp、camera ID、frame IDも確認する。
- Native: 有効JPEGを末尾paddingで32 MiBにし、2枚で64 MiBを占有する。
  3枚目のbackpressure、完了後のencoded bytes = 0、metadata/pixels保持中の次frame admissionを確認。
- Web: production queueのブラウザ検証に同じ32 MiB×2のケースを追加。
  表示用metadata保持中の予算解放と3台目の正常decodeを確認。
  従来のpending coalescing、backward seek、stale generation、2-job上限も維持する。
- 再現用: jpeg-benchmark featureのTrunk buildとJPEG samplesを用意し、
  HTTP 8094で配信、Chrome CDPを起動して
  `python3 tools/performance-probe/measure_web_jpeg.py 9235 /tmp/camera-queue-check.json --queue-check`。
  build/sample準備手順は[Web JPEG計測記録](../../performance/2026-10-15-web-jpeg/README.md)を参照。

## 実行結果（2026-10-15）

標準のDocker `mcap-dev-check`（fmt、workspace Clippy/tests、WASM check、Trunk release、
ROS 2 JazzyのNative Live tests）は成功。Web WASM Clippyは通常・jpeg-benchmark featureとも成功。
Native decodeの6回帰テストも成功した。

Chrome headless、1920×1080、DPR=1でproduction queueの上記検証が成功。
7camera fixtureの全thumbnail描画、focus変更、連続した前後seekも成功し、JS例外はなかった。
3Dの実描画は今回の検証対象に含めていない。
