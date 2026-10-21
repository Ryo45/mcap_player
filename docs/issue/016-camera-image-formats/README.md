# Camera入力形式: Raw Image / PNG

状態: 実装済み（2026-10-21）。JPEG専用だったCamera入力を、同じfeature state・パネル・画素ワーカーのまま拡張した。

## 課題と調査

SessionPlanはCompressedImageだけを選択し、CameraFrameはJPEGのBytesだけを持っていた。
Controllerはserialized messageをcoalesceしてからCDRを読み、Native/Webは表示直前にbounded workerへ画素処理を依頼していた。
この順序は巨大画像の無駄な変換を避けるため維持する。

ROSの[Image定義](https://github.com/ros2/common_interfaces/blob/jazzy/sensor_msgs/msg/Image.msg)では、`step`はパディングを含む行のbyte数、data長は`step * height`である。
[CompressedImage定義](https://github.com/ros2/common_interfaces/blob/jazzy/sensor_msgs/msg/CompressedImage.msg)はJPEG/PNGとcompressedDepthを区別する。
PNGというtokenがあってもcompressedDepthを通常PNGとして扱ってはいけない。

## 実装

- CameraFrameは`CameraImageFormat::{Jpeg, Png, Raw(RawImageLayout)}`と`data: Bytes`を持つ。Raw layoutはwidth/height/step/encodingを保持するsource fact。
- SessionPlanとWebのCamera選択でImageを受理する。Local/Remoteとも同じCDR/controllerを使い、serverには画像変換を追加しない。
- Raw CDRは`rgb8 / bgr8 / rgba8 / bgra8 / mono8`に対応。8bitチャンネルなので画像のis_bigendianは画素値に影響しない。CDR自身のendiannessは既存Readerが処理する。
- Rawは表示サイズのRGBAへ直接nearest samplingする。行パディングを読み飛ばし、BGRをRGBへ並べ替え、monoをRGBへ展開し、alphaを保持する。元画像サイズのRGBA中間バッファは作らない。
- Native JPEGはTurboJPEG/IDCT縮小を維持。PNGはimage decoderで再構築した後に縮小する。PNGは小さい表示先でも元画像のデコードが必要。
- Webは同じ2本のWorkerへformatとsource layoutを渡し、JPEG/PNGはImageBitmap、Rawは表示サイズのImageDataからImageBitmapを作る。PNGのUI側検査は固定長IHDRだけで、metadataの展開を行わない。
- source寸法はoverlayの投影座標として保持し、texture/bitmapの縮小寸法と区別する。表示メトリクスはJPEG固有名からcamera decode名へ変更した。
- 旧`.jpeg` payload fieldは`.data`へ置き換え、現行callerも更新した。互換aliasは追加しない。

## 上限とトレードオフ

画像payloadは1枚32 MiB、sourceは32×1024×1024 pixels、frame IDは4096 bytesまで。
Rawはさらに`step >= width * channels`と`data.len() == step * height`をoverflowなく検査する。
4K RGBA（3840×2160×4、パディングなし）は1枚の入力上限に収まる。
待機・実行中の所有入力コピーは共有64 MiB、32 camera identitiesまで。完了結果はmetadataとpixelsだけを持つ。
Web側にはWASM外の転送buffer、codec内部メモリ、Bitmap/Canvasもあるため、64 MiBはプロセス全体の上限ではない。
Native PNGのdecoder allocation budgetは256 MiB/ジョブ。縮小後のpixelと元画像のcodec出力が一時的に共存し得る。

小さい表示先でのRaw変換は出力画素数に比例するが、入力CDRのI/O・展開・ワーカー用コピーは元画像サイズのまま。
PNGの圧縮によるI/O削減とcodecコストは別のトレードオフであり、今回の機能テストから高速化率は主張しない。
Rawのnearest samplingには細線や模様のaliasingがある。より高品質な縮小は必要性を測って別途検討する。

## Raw ROS live入力の追加

Raw ROS live入力は後続変更で対応済み。`--live-raw-camera-topic TOPIC`を繰り返してImageのsource factを宣言する。グラフからの自動型判定は行わない。[実DDS検証](../../verification/2026-10-21-camera-formats/raw-live.md)を参照。

## 今回含めないもの

mono16、16UC1、32FC1、16bit PNG、compressedDepth、H.264/H.265、CameraInfo対応。
preview-builderのsidecar画像生成は引き続きJPEG入力のみ。Raw/PNGの再生用Camera表示と別に、preview workloadとして後で拡張する。
巨大なprofile、formatごとのパネル、manager/provider抽象は追加していない。

## 確認

[検証記録とブラウザのスクリーンショット](../../verification/2026-10-21-camera-formats/README.md)。
CDRのtruncation、不正layout/overflow、共有backing、admission前のcoalescing、失敗したrestoreで現在frameを保つことを単体テストした。
実MCAP fixtureで全形式の選択、正確なpredecessor、前後Seek、source/縮小寸法、色を確認した。
JPEGの既存worker budget・破損画像の分離・世代管理テストも維持する。
