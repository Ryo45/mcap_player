# PointCloud2 recording display

Native / Browser Local / Remoteの記録入力で、sensor_msgs/msg/PointCloud2を既存Sceneへ表示する。
MCAP adapterやrendererにROSメッセージのデコードを追加せず、coreの経路を拡張する。

~~~text
source catalog + workspace point-cloud topic
  → SessionPlan（CDR LaserScan / PointCloud2を選択）
  → PointCloudController（形式選択はsession作成時）
  → XYZ + source RGBA / intensity
  → Scene presentation（measurement-time TF、着色、bounded accumulation）
  → Native / Web共通のscene-renderer
~~~

## データと座標

- CDR1 little/big endianと、独立したis_bigendianの点データを扱う。
- organized/unorganized両方に対応し、point_step、row_step、field offset、行paddingを尊重する。
- x/y/zはJazzy PointFieldの8種類の数値型を受け付け、renderer用f32座標へ変換する。座標はmessage frameのmetres。geometryの倍率補正はしない。
- NaN/Inf、f32で表現できない座標は点ごと除外し、RGBAとintensityも同じindexで除外する。is_dense=trueでも検査する。
- frame ID、measurement time、MCAP log timeを区別し、既存のacquisition-time TFとlatest-before restoreを使う。
- canonical stateには直近1メッセージのXYZと元の属性を保持する。CDR data blobは借用して読み、点ごとのrawコピーやMCAP Chunkの永続保持をしない。

## 着色

- Auto: RGBA/RGB → intensity → solidの順で選ぶ。
- Solid: 従来のcyan。
- RGB / RGBA: packed FLOAT32またはUINT32のrgb / rgbaに対応する。floatは数値castせずbit patternを読む。両方あればrgbaを優先する。RGBは不透明、RGBAは元のalphaを尊重する。
- Intensity: scalarのintensityを、固定範囲でblue → cyan → green → yellow → redへ写像する。既定は0..255、ユーザーがproducerの値域に合わせて変更する。範囲外はclamp、非有限値はsolidへfallbackする。geometryの単位変換ではない。
- 指定した属性がない場合はsolidと理由を表示する。無効な強度範囲は0..255へfallbackして理由を表示する。
- 色の選択・colormapはcoreのpresentationにあり、rendererにはlinear RGBAを渡す。Nativeのeguiへは自動sRGB decodeをしないtexture viewを渡し、二重変換で元のRGBが暗くなることを避ける。Webもcanvas formatがsRGB encodeしない場合はpresenterで明示的にencodeする。RGB byteからのsRGB変換は小さい共有lookup tableを使う。
- 色または強度範囲の変更時は蓄積済み表示を破棄し、最新frameで再構築する。古い色の履歴と混ぜず、履歴全体の属性を別途保持しない。

NativeのScene設定にはcoloring: {"mode":"rgb","intensityRange":[0,255]}も指定できる。
操作はScene toolbar、Webも同じモードと範囲を使う。WebのPoint cloud topicは記録を開く前に設定する。
Nativeの--point-cloud-topicはworkspace bindingを上書きし、--layout sceneはカメラなしの記録にも使える。
既定bindingの/scanとLaserScan表示は維持する。ROS Liveの既存点群入力はLaserScanのままで、PointCloud2のLive受信は今回の範囲外。

## 上限とエラー

1メッセージ64 MiB、入力1,000,000点、256 fieldsまで。allocation前に点数と全field extent、row/dataサイズを検証する。
未知のdatatype、重複・非scalarのXYZ/色情報/強度、欠けたXYZ、壊れたstride/長さは明示的なdecode errorにする。
空のPointCloud2はXYZなしでも受け付け、空の最新frameを表現できる。
未使用fieldはextentを検証してから捨てる。法線や個別r/g/b fieldの解釈、任意field着色は追加していない。

Sceneは従来どおり最大65,536点を表示する。超過分は先頭から落とし、末尾を保持する。
これはdownsamplingではないため、大きな点群の形状を均等に保つ保証はない。
XYZ/属性の入力上限と表示点数の上限は別であり、比較時はinput/visibleの両方を確認する。
serialized windows、canonical state、TF後の一時配列、表示用配列、GPU bufferはそれぞれメモリを持つ。
65,536という論理点数だけを総メモリの上限とは扱わない。

forwardの不正メッセージはerror counterを増やして直前の有効stateを維持する。
seek restoreでのdecode失敗はcandidateを破棄し、既存のXYZ/属性/カーソルを部分更新しない。

## 検証とfixture

tests/fixtures/point-cloud2はROS 2 Jazzyの実rclpy.serialization.serialize_messageで生成する。
organized 2×2、32 byte/point、72 byte/row、並び替えたfields、LE/BE data、半透明RGBA、
FLOAT64 intensity、is_dense=true中のNaN点を含む。tools/generate_point_cloud2_fixture.pyで再生成できる。
単体テストはpacked float RGB、8数値型、壊れたfield/stride/count、全byte境界のtruncationも確認する。
integration testはMCAP indexed latest-before、restore rollback、TF後の色対応、色切替を確認する。

[ROS 2 Jazzy PointCloud2定義](https://github.com/ros2/common_interfaces/blob/jazzy/sensor_msgs/msg/PointCloud2.msg)と
[PointField定義](https://github.com/ros2/common_interfaces/blob/jazzy/sensor_msgs/msg/PointField.msg)に従う。

### Opaque depth

Fully opaque point clouds write depth, so nearer points occlude farther points independently of input order. Clouds containing transparency retain alpha blending without depth writes; their overlapping transparent points are not depth sorted.

The explicit GPU regression in scene-renderer, opaque_points_occlude_by_distance_in_both_orders, renders overlapping red/blue points in both input orders and reads back the centre pixel. It passed on NVIDIA GeForce RTX 5070 Ti / Vulkan / driver 610.57.04. Run it explicitly on a working GPU with cargo test -p scene-renderer -- --ignored --nocapture; ordinary workspace tests leave this hardware-dependent test ignored.
