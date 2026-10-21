# 開発継続後のセルフレビュー — 2026-10-21

レビュー開始HEAD: `016228f`。
主な対象は分割MCAP/Web LZ4 (`88688be`)、画像入力追加 (`75ec947`)、Raw live (`016228f`)。
加えてworkspace全体のcrate依存、core/adapter/presentation/rendererの境界、app/lib/module配置を確認した。
全ファイルの全経路を網羅する形式検証ではない。

## 確認した不具合と修正

### [P2] Webの透明画像が前frame・旧overlayへ重なる

`apps/viewer-web/src/app/camera.rs`のbase Canvasとfocus/thumbnail Canvasは、寸法が同じなら内容を消さずにsource-overで描いていた。
不透明JPEGでは覆い隠されていたが、RGBA/BGRA/PNGでは透明領域に旧frameが残り、半透明のalphaも蓄積する。
baseと最終合成の両方でCanvasをclearしてから新しい画像を描くようにした。

[透明fixture](../../../tests/fixtures/camera-formats/README.md)で再生中の実Canvasを検査した。
[修正後](alpha.json)は102回のdrawについてsourceと出力の中心画素が一致し、alpha 0/128/255すべてを確認した。
[負の対照](alpha-negative-control.json)は、同じ修正後buildで`clearRect`を無効にするfault injection。
透明なsource [0,0,0,0]に対して出力[20,80,159,128]、半透明sourceに対して出力alpha192などを検出した。
これは修正前buildの実行記録ではなく、チェッカーが残像とalpha蓄積を検出できることを確認する実験である。

### [P2] Nativeのalpha表現がeguiの描画契約と不一致

`apps/viewer-native/src/graphics/camera.rs`はstraight RGBAをそのままGPUへ渡していた。
使用中のegui-wgpu shaderは色をそのまま出力し、blendのsource factorはOneで、premultiplied sRGBAを要求する。
半透明[200,100,50,128]をそのまま送ると、期待する[100,50,25,128]より明るくなる。alpha0のRGBも加算され得る。

CDR/Camera stateとdecoder出力はstraight RGBAのまま維持し、Nativeの所有upload bufferだけをeguiのColor32規則でin-place変換する。
NativeImageの`has_alpha`でJPEG/RGB/monoの不要な変換走査を避け、PNG/RGBA/BGRAだけを準備する。
alpha変換のCPU時間もupload準備時間へ含める。新しい元画像サイズのbufferやcanonical stateの変換は導入しない。
unit testで半透明・完全透明・不透明の出力と、decoderがstraight pixelsを保つ契約を検査する。
ハードウェアGPUによる画像readback試験は今回は行っていない。

### [P2] 空のMCAP partが統合timelineを時刻0へ広げる

`crates/viewer-mcap/src/recording/segments.rs`は全partのStatistics開始/終了時刻を無条件にmin/maxしていた。
schemaだけのChunkを持つ有効なpartはmessage_count=0、time=[0,0]であり、実データが[100,200]でも統合開始が0になった。
regression testで修正前の`ArrivalTime(0) != ArrivalTime(100)`を再現した。
メッセージがないpartは時間のmin/maxから除外し、全partが空なら[0,0]へ正規化した。
partの順序を逆にした場合と全空partも検査する。payload全走査は追加していない。

## アーキテクチャの評価

**責務と依存の秩序は維持できている。今回の不具合は境界の値の契約が不足していたもので、層が相互侵入した結果ではない。**
今すぐcrateの再分割やgeneric managerを導入する根拠は見つからなかった。

| 境界 | 現状と確認した点 |
| --- | --- |
| viewer-core | workspace内のproduction crate依存なし。MCAP/zstd/lz4/crc、rclrs、egui/wgpu/web_sys依存なし。source-neutralなcontracts、feature state、純粋なpresentation計算を保持 |
| viewer-mcap | coreへ依存。物理Chunk/Index/分割partの情報を所有し、統合後のRawMessageを出す。core→mcap逆依存はない |
| Native Live adapter | 型の宣言、ROS subscription/QoS、DynamicMessage→CDR、latest/ordered admissionを所有。ROS固有の型/設定はcoreやrendererへ渡していない |
| Camera state | JPEG/PNG/Rawで一つ。controllerはserialized admission/coalescingとCDR materializationを担当し、pixel decodeはpresentation側へ残す |
| renderer / frontend | Native/Web固有のdecode/worker/GPU/Canvasを所有。straight RGBAからeguiのblend形式への変換もNative frontend境界へ限定 |
| recording-server | indexed readとwire responseを担当。feature controllersやpixel decodeを所有しない |

`lib.rs`はpublic surfaceとmodule indexとして機能し、application実装の集積は見つからなかった。
Native/Webの`app.rs`は入力・session・presentationの接続とイベント進行を担当し、codec、Chunk読み込み、query worker、具体描画は別moduleへ委譲する。
`message::cdr::image`と`viewer-renderer::image`は同じstemでもparentが異なり、それぞれCDR metadata/payloadと画素処理を表すため意味が衝突していない。

PlatformGeometry、CameraCalibration、WorkspaceBindings、Layoutは独立した概念として維持され、今回も巨大なprofileやformatごとの状態管理は追加されていない。
Source facts → semantic feature state → presentation → rendererの向きは維持する。

### [A2] 次の画像形式追加で確認すべき点

入力形式の受理はSessionPlan（schema）、CDR codec（encoding）、renderer（pixel）、Live adapter（subscription/reconstruction）に現れる。
これは現在それぞれ異なる責務であり、分岐の数だけを理由にregistry/traitへ統合しない。
ただし今後の16bit/depth対応で**同じencoding validationや変換policy**を各層へ複製し始めたら、今あるRawImageEncoding/RawImageLayoutと共有codecへ集約する。
今回のRaw live encoderは既に共通layout検証を利用している。

32 MiB入力・source pixels・64 MiBコピー予算のような上限は、CDR安全性、decodeコスト、queue保持で意味が異なる。
数値が同じだからと巨大profileへまとめず、次に変更する際はNative/Web/JS側の整合を一緒に確認する。
現時点でこの点による不整合は確認していない。

## 検証と限界

```sh
docker compose run --rm dev mcap-dev-check
python3 tools/performance-probe/check_web_camera.py \
  --data tests/fixtures/camera-formats/camera_alpha.mcap \
  --camera-alpha --output /tmp/camera-alpha-check.json
```

workspace fmt/clippy/tests、WASM check、Web release build、ROS有効Native testsを実行。
既存の7形式のopaque画像と前後Seekも[再確認](opaque.json)した。
ブラウザはFHD、headless Chrome + SwiftShader + software Canvas2D。性能試験ではない。
以前からあるheadless accelerated Canvasの透明化問題を解決したとは主張しない。
今回のWeb回帰はclearRectがない場合に起きる画像の合成不良を切り分けて確認したもの。
