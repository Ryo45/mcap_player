# Camera focus / Preview follow-up — 2026-10-27

Baseline: `fa39cb9`。前回の[検証](../2026-10-27-camera-topics/README.md)で残った
Native focus再復元、画像preview無効化、Web遅延増加を点検した。

## Changes and behavior

- Nativeで選択stream集合が同じなら既存plan / CameraId / runtimeを維持する。
  sourceを読み直さず、選んだCameraの最新admit済みmessageだけをmaterializeする。
  再生中・停止中とも点群、Path、TF、cursor、再生速度を保ち、GPU全体の無効化も行わない。
  malformedなpending CDRは旧表示・選択・pending・カウンタを更新せずエラーにする。
- stream集合が変わる場合は従来のtransactional replan / restoreを使用する。
- Preview schema 2でCameraId→元トピックを保存し、Nativeの現在のIDへ解決する。
  固定パネルのsubset、切り替え、元のトピックへの復帰でも正しい画像を選べる。
  previewが未生成のトピックはその画像だけ省き、数値・軌跡は保持する。
- preview cacheは従来arrival time / dimensionsだけのkeyだったため、同時刻・同寸法の
  別画像がID再利用で衝突し得た。対応のrevision変更時にGPU preview keyを消す。
- Webは毎frameのselect要素検索・value取得・文字列生成をやめ、選択/候補変更時だけ更新。
  拒否されたDOM選択も元のfocusへ戻す。非同期seekとpixel decodeの意味は変えない。

**Operational change:** schema 1のPreview sidecarは現在のpreview-builderで再生成が必要。
旧形式の自動移行や推測によるtopic対応は追加していない。元MCAPは変更しない。

## Native focus measurement

実際のApp action処理をrelease modeで計測するignore testを追加。
512MiB Chunk・4K JPEG×8の既存fixture、同じ中央時刻をseekしてwarm cacheにした後、
2 Cameraを40回交互に選択。変更前/後それぞれfresh process 3回。
各processの40操作の上側中央値の中央値は **0.023260→0.001758ms**（約23→1.8µs）。

これはsource/feature/action処理だけで、JPEG pixel decode、GPU upload、display待ちを含まない。
全CameraのJPEG再decodeを発生させるcache clearがなくなる効果は、この数値に含めていない。
大幅なend-to-end改善を主張する計測ではない。

```sh
MCAP_CAMERA_FOCUS_RECORDING=target/performance-regression/data/jpeg-512m.mcap \
  cargo test --release -p viewer-native measure_camera_focus_actions -- --ignored --nocapture
```

## Regression and Web latency

- `mcap-dev-check`: fmt、workspace Clippy -D warnings、全workspace tests、WASM check、
  Trunk release、ROS Live testsを通過。
- Nativeの既存5ケース×3 fresh process: 前回の最終Native suiteをbaselineとして全基準通過。
- Webは今回の編集前に保存したfa39cb9のrelease assetsを各3回、今回のreleaseを各3回測定。
  Core Ultra 7 265KF / RTX 5070 Ti / Chrome 143 / FHD / focus 960×540。
  fixture SHA照合・warm-read policy・回帰検出閾値は変更していない。ビルドと計測は直列。

| Case | Metric | Before | After |
| --- | --- | ---: | ---: |
| web/raw-fhd-30hz | seek_focus_ms | 107.25 | 110.70 |
| web/raw-fhd-30hz | focus_content_hz | 29.22 | 29.05 |
| web/jpeg-4k-8cam | focus_content_hz | 9.69 | 9.62 |
| web/jpeg-4k-8cam | heartbeat_max_ms | 62.90 | 66.30 |
| web/jpeg-512m | heartbeat_max_ms | 33.90 | 35.60 |

Webも既存の全回帰基準を通過。前回の512MiB heartbeat最大遅延増（36.05→52.60ms）と
同程度の差は今回再現しなかった。一方、今回の値もすべて改善したわけではなく、
DOM処理削減がSeek/heartbeatを有意に高速化したとは結論しない。
今回の追加計測に初回不合格や再測定による採用切り替えはない。

## Functional verification

- Native: focus時刻とID保持、選択Cameraのexact latest、点群Vec backing pointerとPath/TF
  revision保持、pending画像のCDR failure rollbackをテスト。
- Preview: artifact front=0/rear=1から、Native subsetでrear=0へ変わってもrearの画素を返す。
  previewなしのtopicを挟んでfrontへ戻った場合も再度利用できる。
  対応revisionが毎回変わることをテストし、画像cache invalidationへ接続する。
- Reader/writer: source-topic binding欠落画像と旧schemaを拒否。新map付きMCAP roundtrip。
- preview-builder releaseを実fixtureへ実行し、7 Camera / 42 images / 50 signal buckets /
  10 trajectory pointsのschema 2 sidecarを190,830 bytesで生成。既存sidecarは上書きしない。
- Web UI: production releaseをCDP操作し、Raw/PNG/JPEGの期待画素、paused cursor、
  source交換、非画像除外、実録`/cam/camera/image_raw`の実画素を確認。

ログ、Native focus全6processの数値、Native/Web suiteとcomparisonを同梱。
