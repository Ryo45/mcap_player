# Native JPEG presentation

NativeのJPEG pixel decodeはlibjpeg-turbo 3.1.0（`turbojpeg` 1.5.1 / `turbojpeg-sys` 1.2.0のvendored build）を使う。
`viewer-core`のCompressedImage CDR decode、camera admission、focused/background Hz、MCAP I/Oはこの変更で変えない。
Webの後続改善は[Browser JPEG presentation](web_jpeg.md)を参照。

## 表示サイズと座標

Native panelは実際の画像領域をdevice pixelで要求する。複数panelが同じcameraを表示するときは必要な最大サイズを集約する。
元のaspect ratioでcontainするサイズを満たす最小のIDCT倍率（1/8、1/4、1/2、1）を使い、RGBAへ直接decodeする。
表示後の縮小を目的として原寸RGBを経由しない。小さいサムネイルも4K入力なら最小480×270になる。

フォーカスを変えたりwindowを広げたりすると、同じarrivalのJPEGでも必要な解像度が変われば再decodeする。
scale classが同じなら再decodeしない。元画像のwidth/heightはpixel bufferのwidth/heightと独立して保持する。
Camera overlayは元画像のpixel座標で計算し、aspect ratioとUVの端を合わせて描画する。
641×479のような奇数寸法ではIDCT出力の丸めによる端のpaddingをUVでcropする。
非同期decodeのため、overlayに使うframeはcontrollerの最新frameではなく実際にGPUへuploadしたframeになる。
そのmeasurement timeとframe IDを使ってTF projectionする。

## ワーカーとメモリ

`CameraDecodeQueue`はNativeで2本のthreadを使い、各threadが独立したdecoderを再利用する。GPU uploadはUI threadで行う。
カメラごとに未開始の最新1枚を持ち、追加arrivalで待機中の古い画像を置き換える。同じcameraのdecodeは同時に1枚まで。
round robinで別cameraも処理する。完成結果のchannelは2枚までで、UIが受け取れないとworkerを止める。
したがって完成2枚＋実行中2枚までのRGBA bufferが一時的に残り得る。原寸decodeではthread化による一時メモリ増加が大きい。

worker入力はJPEG部分だけをowned bufferへコピーする。MCAP Chunk全体を`Bytes`のbackingとして引き留めないためである。
待機・実行中のJPEGコピーに64 MiBの共有上限を設ける。
デコード完了時にJPEGコピーを解放し、完成結果と表示済みstateはpixelsとCameraFrameMetadataだけを保持する。
metadataにはcamera ID、measurement time、arrival time、frame IDを保持し、投影APIはJPEGを要求しない。
入力1枚は32 MiB、元画像は32×1024×1024 pixels、camera identitiesは32まで。
progressive JPEGのTurboJPEG decodeには100 scansの上限を設定する。
巨大な画像を縮小表示する場合にも元寸法の上限を検査する。
CMYK/YCCKは既存Rust decoderで原寸RGBAへ変換するfallbackを持ち、lossless JPEGも原寸を使う。
これらは縮小の高速化を期待する経路ではない。GPUの最大texture寸法もupload前に検査する。

byte上限でadmissionできないときはUIを待たせず、次のtickで再試行する。
表示済みmetadataはencoded budgetを占有しない。32 MiBのJPEGを2枚処理して上限に達した場合も、完了後は次のframeを受け入れられる。
RGBAはupload後に解放する。GPU textureとcanonical CameraStateのbacking memoryはこの64 MiBの外側であり、総RSSとは別である。
成功したseek/source changeだけがgenerationを進め、待機と完成済みを破棄する。
実行中のdecodeは途中cancelせず、古いgenerationの結果をuploadから除外する。
終了時はreceiverを切断してからjoinし、完成channelが満杯でもshutdownをdeadlockさせない。

## ビルドと検証

NativeはSIMD有効のlibjpeg-turboを静的リンクする。C/C++ compiler、CMake、NASM、pkg-configが必要。
Docker開発環境には追加済みで、実行時の共有libjpeg-turboは必要ない。
関連packageにだけ`viewer-renderer/native-jpeg` featureを有効化し、WASMのcodec依存には入れない。

テストは縮小倍率の切替、奇数寸法/source geometry/alpha、破損JPEG、最新待機画像の置換、
byte-budget backpressure、compact JPEG backing、古いgenerationの除外、camera間のエラー独立、満杯channelのshutdownを検証する。
Native・WASM・Web release・ROS 2を含む`docker compose run --rm dev mcap-dev-check`を通した。
既存7-camera fixtureを1920×1080のNative windowで開き、camera一覧とplanned-path overlayを確認した。
これは機能確認であり、GUIのframe rateを測った結果ではない。

## 計測の条件と限界

[同じ4K stress recordingsでの比較](performance/2026-10-14-native-jpeg/summary.md)とraw JSONを保存した。
最低FHDを基準として、focusに**1920×1080の物理pixels**、他cameraに96×72の表示boxを要求する。
全camera原寸と全camera1080pも別条件として比較する。HiDPIはlogical sizeではなくdevice pixelsで決まる。
FHDより大きいfocus領域や高いDPRでは原寸decodeが必要になり、ここでの縮小条件と同じ性能にはならない。

workers2条件は各50 ms tickで完了を待つbarrierを置き、single-thread条件と同じ179枚（8cam）/100枚（4cam）をdecodeする。
本番UIにはbarrierがなく、I/Oとworker decodeは重なり、負荷超過時は待機画像を置き換える。
このheadless結果は「少ない枚数をdecodeしたので速かった」という比較を避けるためのものであり、
本番GUIの全camera全入力80 frames/s表示、GPU upload、frame timeやend-to-end latencyを証明しない。
既存のfocused10Hz/background5Hz admissionも維持している。

stress JPEGは品質85、4:4:4のgradient/noiseで大きなpayloadを持つ。IDCT縮小でpixel処理と保持量は減るが、
JPEG entropy decode自体を不要にできない。記録側の4:2:0、品質、画像内容、解像度の違いでも結果は変わる。
Chunk展開とI/Oも残るため、TurboJPEGだけであらゆる記録が実時間再生になるとは判断しない。

実装を確認した一次資料:
[libjpeg-turbo](https://github.com/libjpeg-turbo/libjpeg-turbo)、
[rust-turbojpeg](https://github.com/honzasp/rust-turbojpeg)、
[Decompressor API](https://docs.rs/turbojpeg/1.5.1/turbojpeg/struct.Decompressor.html)。
