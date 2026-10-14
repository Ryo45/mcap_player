# 表示済みcameraのmetadataが圧縮JPEGを引き留める

状態: 未対応。2026-10-15、Native/Web JPEG非同期化のレビューで確認。

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
