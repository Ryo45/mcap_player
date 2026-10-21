# Raw ROS live input — 2026-10-21

## 変更

`--live-raw-camera-topic TOPIC`でsensor_msgs/msg/Imageの入力を宣言できるようにした。
複数回指定可能で、all-cameraパネルでは宣言したRaw topicも選択候補になる。
既存のCameraパネルがtopicを固定している場合は、その要求だけを選択する。
指定していないCamera topicはCompressedImageであり、topic名のsuffixから型を推測しない。
`--camera-topic`で優先Cameraを明示しなければRaw topicの辞書順先頭を使う。

現行Live adapterはROSグラフを自動探索する方式ではなく、source factを宣言してから購読する方式。
publisherがまだいない場合も開始でき、Raw/CompressedImageの混在を扱える。
16bit/depth画像と、実行中に同一topicの型を変更する機能は今回含めない。

Rawも同じlatest-only mailbox → CameraController → pixel worker → rendererを使う。
受信callbackではROSのborrowed uint8列から直接CDRへコピーし、一時的な画像Vecを経由しない。
shared encoderは既存のRawImageLayout検証を使用し、32 MiB、width/height/step/data長、frame IDの上限を確認する。
8bitなので画像のis_bigendianは0/1どちらでも同じ画素として扱え、再構築CDRでは0へ正規化する。
画素変換はcontrollerによるcoalescingより後の表示段階で行う。

## 実DDS確認

```sh
docker compose run --rm dev cargo test -p viewer-native --features ros2-live \
  rosbag_raw_and_compressed_camera_smoke -- --ignored --nocapture
```

ROS 2 Jazzy / rclrs 0.7.0で、subscriberを先に作成してからrosbag2を起動。
[既存fixture](../../../tests/fixtures/camera-formats/README.md)のRaw 5形式とPNG/JPEGを10 Hzで同時配信した。

[実行ログ](raw-live-dds.log): **成功**。

- 全7 topicで受信とCamera state生成を確認。
- header stampからsequenceを求め、160×120へ縮小した画像の画素が既知色と一致。Raw/PNGは完全一致、JPEGは各成分±3。
- Rawの行paddingとis_bigendianの0/1混在をROS DynamicMessage経由で処理。
- adapter errors / ordered overflow: 0、queue high water: 7 messages。
- 今回の受信数は207/210。best effortの試験であり全メッセージ受信の保証ではない。
- GPU描画や4K多数カメラの性能測定はこのテストの対象外。

通常の単体テストでは100枚のRawが1枚の未変換messageへcoalesceされること、Raw/Compressedの選択・delivery class、
固定パネルによる未選択入力の除外、CLIのvalidation、共通Raw encoderの5形式round tripと不正データ拒否も確認する。
既存の`rosbag_multi_stream_dynamic_smoke_and_reopen`（Camera/Path/Odometry/LaserScan/TF）も明示実行し成功した。

## リソース上の境界

viewer自身のLive mailboxはCameraごとに最新1枚。表示用入力コピーの64 MiB予算とworker数は変更していない。
DDS自身の受信・deserialization・QoS履歴や、callbackの一時CDRはこれらと別のメモリを使う。
今回の小さいfixtureでキューの挙動を確認した結果から、巨大Raw入力のthroughputやRSS上限は主張しない。

## チェック

ROS有効時のNative clippy、workspace fmt/clippy/tests、WASM check、Web release buildがすべて成功。ROS有効時の通常Native testsは83 passed / 3 ignored（うちDDS smoke 2本を別途明示実行し成功）。
セルフレビューでは、Raw topicが既存のfeature bindingと衝突する場合の拒否、未選択入力を購読しないこと、
Camera専用状態を追加していないこと、エラーをadapter diagnosticsへ返してexecutorを維持する経路を確認した。
