# Browser JPEG presentation

Webのpixel decodeはWASM内の原寸RGBA展開から、2本のDedicated Web Workerと`createImageBitmap`へ移した。
`viewer-core`のCamera admission、MCAP/CDR decode、DataPlane buffering/seekは変更しない。
NativeのTurboJPEG経路とは別のplatform実装である。

## データの流れ

1. JPEGのSOFから元画像のwidth/heightを得る。entropy dataを読まず、JPEG全体をコピーしない。
2. camera panel / thumbnailのCSS領域とdevicePixelRatioから、aspect ratioを保つ出力寸法を求める。
3. compact JPEGをWASM memoryからJSへ1回コピーし、ArrayBufferをWorkerへtransferする。
4. WorkerがJPEG Blobから指定サイズのImageBitmapを作り、main threadへtransferする。
5. Canvasへ描き、ImageBitmapを`close()`する。原寸RGBAをWASMからJSへ渡さない。

focus変更やresizeはarrivalが同じでも必要寸法が変われば再decodeする。
base canvasに加え、表示用thumbnail canvasの内部寸法も縮小する。CSSだけ小さくした原寸canvasをcamera数だけ持たない。
元画像の寸法と表示中frameを別に保持し、overlayは元のpixel座標・そのframeのmeasurement time/frame IDで投影する。
結果を縮小Canvasへ写す際にpixel座標を変換する。TF/calibrationへ表示倍率を混ぜない。

Chromeでは`imageOrientation: "none"`だけではEXIF回転を抑止できなかった。
WorkerはEXIF APP1 headerを除いたBlob partsを作り、encoded scan方向を維持する。scan dataを再encodeしない。
EXIF Orientation=6を付加したJPEGと元JPEGで、出力pixelが一致する回帰テストを実行した。
他のsegmentと圧縮scanはそのまま渡す。

## リソースと世代

- cameraごとに実行中1枚、未開始の最新1枚。待機中は新しいarrivalで置換する。round robinで順番を選ぶ。
- 実行中＋未uploadの完成結果は合計2枚まで。完成結果をdrainしてから次を開始する。
- 最大32 camera identities。JPEG 1枚32 MiB、元画像32×1024×1024 pixels、frame ID 4096 bytesまで。
  Workerのheader処理も4096 markersまでに制限する。
- RustのJPEGコピーは、待機・実行中を合わせて64 MiBまで。
  `Bytes`のMCAP Chunk backingをworker用に保持せず、JPEGだけをowned copyにする。
  byte上限に達したらそのtickのadmissionを見送り、次に再試行する。上限を自動拡大しない。
  完了時にJPEGコピーを解放する。完成結果と表示済みstateはBitmap/CanvasとCameraFrameMetadataだけを持ち、encoded budgetを占有しない。
  投影には表示中imageのcamera ID、measurement time、arrival time、frame IDを渡す。[metadata分離の対応](issue/014-camera-presentation-metadata/README.md)を参照。
- JS/Workerへ渡したArrayBuffer、Blob、browser内部のdecode作業領域、Bitmap、Canvas、canonical CameraStateとDataPlaneは64 MiBの外側。
  **総RSSの64 MiB上限ではない。** 同時job数と入力・元画像の寸法を制限するが、browser内部の一時allocation/GCは直接管理できない。
- source/seek reset時は待機と完成済みを破棄し、generationを進める。camera queue自体は置換しない。
  古いin-flight jobも完了まで2本の上限に含める。古いgenerationのbitmapはuploadせずcloseする。
  failed restoreは現在のpixel stateをresetしない。
- pixel decode/upload errorはcamera単位で保持し、他cameraを処理し続ける。通常のReady表示でerrorを消さない。
  そのcameraの次のupload成功またはresetで解消する。

`createImageBitmap`のresizeがIDCT段階で実装されるかはbrowser次第であり、縮小出力だから原寸の作業領域も省けたとは主張しない。
計測でも1 workerの縮小が原寸より速くならない場合があった。出力保持量とmain-thread停止時間は独立して評価する。
WebCodecsの`ImageDecoder.desiredWidth/desiredHeight`は次の検証候補だが、対応browser/secure contextとorientationの扱いを確認する必要がある。

## 配布・検証

`camera_decode_worker.js`をTrunkのcopy-file assetとして同じoriginへ配布する。
専用Workerとtransferableを使い、WASM threadsやSharedArrayBuffer、COOP/COEPを必須にしない。
通常buildには旧decoderの計測exportを含めない。計測用には`jpeg-benchmark` featureを使う。

[4K JPEG / FHD比較](performance/2026-10-15-web-jpeg/README.md)に条件と生データを保存した。
Native/WASM/Web release/ROS 2の全ゲートに加えて、WASM targetのclippyを実行した。
ブラウザでproduction queueのcoalescing、backward seek世代、2-job上限、encoded参照の解放、破損JPEG後の正常decode、EXIF方向を検証した。
1920×1080、7camera fixtureでカメラ一覧・focus変更・連続前後seek・overlayを確認し、JSの例外はなかった。
この環境のheadful ChromeではWebGPU adapterが取得できなかった。headlessの通常buildではWebGPU readyとなったが、3D領域が黒いため、3D実描画の再確認はできていない。

一次資料:
[createImageBitmap](https://developer.mozilla.org/en-US/docs/Web/API/Window/createImageBitmap)、
[WorkerでのcreateImageBitmap](https://developer.mozilla.org/en-US/docs/Web/API/WorkerGlobalScope/createImageBitmap)、
[ImageDecoder constructor](https://developer.mozilla.org/en-US/docs/Web/API/ImageDecoder/ImageDecoder)。
