# Camera topic selection

Cameraパネルのプルダウンで、開いた記録の画像トピックを切り替えられる。
NativeとWeb（Browser Local / Recording Server）の双方で利用できる。
トピック名を変更するためにコードやlayout JSONを編集する必要はない。

候補はcatalogのmessage encodingが`cdr`で、schema名が次のいずれかのstream。
名前のprefixや`/camera`、`compressed`などのsuffixからは推測しない。

- `sensor_msgs/msg/Image`: 現在のpixel decoderはrgb8 / bgr8 / rgba8 / bgra8 / mono8に対応。
- `sensor_msgs/msg/CompressedImage`: JPEG / PNGに対応。

schemaは画像の入力種別を示す。未対応のpixel encodingやcompressedDepthなど、
メッセージ内部の形式までcatalogから保証するものではない。
Camera calibration、TF、Pathのbindingは画像トピックの選択と独立しており、
別カメラ用のcalibrationを自動生成・流用する処理は追加していない。

## Native

`--layout monitor`はCamera 1枚と、存在する場合のScene入力を選択する。
layoutの`cameraTopic`は初期選択の希望値として使用する。MCAP内に存在しなければ、
対応画像の中から`--camera-topic`の希望値、次にトピック名の辞書順先頭を選ぶ。
画像がなければCameraは空表示となる。ほかのパネルが必須とするPathなどの入力条件は変えない。

複数Cameraパネルではそれぞれ独立して選択できる。
固定トピックを指定したパネル群では、その時点の選択の和集合をcontinuous playbackに渡す。
同じ画像を複数パネルで選んでも、読み込み・materializationは共有する。
サムネイル一覧型のパネルは従来どおり全画像を要求する。
候補一覧の取得そのものはメタデータの参照だけで、画像を展開しない。

切り替え時は新しいSessionPlanと空のFeatureRuntimeを用意し、
現在のMCAP log timeでlatest-before / TF history / static TFを復元する。
読み込み・CDR復元が成功してからsource traversal、plan、runtime、パネル選択をまとめて差し替える。
再生/停止と速度は維持し、失敗時は旧source位置・cursor・feature state・選択を保つ。
CameraIdは再割当可能なので、成功後は画像decode jobとGPU/presentation cacheを無効化する。

読み込み対象が変わらない場合はCameraIdとfeature stateを維持し、フォーカスだけ変更する。
停止中も既にadmitした最新画像をmaterializeできるため、source再読み込みは不要。

Preview sidecar（schema 2）はartifactのCameraIdから元トピックへの対応を保持する。
現在のSessionPlanへ対応づけてプレビュー画像のIDを変換するため、カメラ切り替え後も利用できる。
元トピックのpreviewがない場合はその画像だけを省き、exact画像を表示する。
数値・軌跡プレビューも維持する。対応変更時はGPUのpreview cacheを無効化する。
旧schema 1のsidecarは再生成が必要（自動移行は行わない）。

ROS Liveでは接続時のcatalog（購読済み画像）の範囲で表示を切り替える。
ROS graphからの追加発見・購読の動的変更は行わない。

## Web

既存の全対応画像＋サムネイル構成を維持し、プルダウンとサムネイルは同じfocus選択を使う。
切り替えはscheduler priorityを変更する。表示中は余分なseekを発行しない。
停止中は現在時刻の正確なpredecessorを非同期復元するため、背景用に間引かれた古い画像で止まらない。
すでにseek中なら、その要求先時刻を維持する。
ファイル/Remote recording交換時は選択・候補・非同期画像jobをリセットする。

## Verification

Rustの回帰テストはschema/encodingによる候補選別、初期fallback、画像なし、
NativeのRaw/Compressed切り替え、停止時刻と新規forward traversal、失敗時rollbackを扱う。
Webは停止中の復元、seek中のfocus変更、表示中の不要なrestore防止を検証する。

ブラウザUIと既知色の画素は実際のrelease版をCDPから操作して確認できる。

```sh
python3 tools/performance-probe/check_camera_topics.py \
  --output target/camera-topic-ui \
  --wake-display --vulkan-icd /usr/share/vulkan/icd.d/nvidia_icd.json
```

テストはFHD viewportでプルダウンを操作し、Raw / PNG / JPEGの停止中画素、
時刻維持、source交換時の候補リセット、非画像トピックの除外を確認する。

実行結果・性能比較・スクリーンショットは[検証記録](verification/2026-10-27-camera-topics/README.md)にまとめている。

フォーカス・プレビュー改善の[追加検証](verification/2026-10-27-camera-focus-followup/README.md)も参照。
