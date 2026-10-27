# 用途別レイアウト

Nativeの起動時に`--layout`で選ぶ。以下の画像は通常のreleaseビルドを1920×1080で撮影したもの。
[撮影用サンプルの準備](screenshots.md)後、`/tmp/mcap-readme-demo/recording.mcap`を指定すれば同じ内容を試せる。

## 複数のカメラを一覧する — cameras

![4つの画像トピックを2×2に配置](images/layout-cameras.png)

映像を4枚並べ、再生・停止・Seekを同じMCAP log timeで操作する。
フロントを入力周期へ追従するフォーカスにし、ほかの3台は既定の背景更新（最大5Hz）を使う。
全カメラを同じ頻度で表示する構成ではない。Path、Scan、TFは再生対象に含めない。

```sh
cargo run -p viewer-native -- --mcap /tmp/mcap-readme-demo/recording.mcap --layout cameras
```

同梱presetの対象は`/camera/front/image/compressed`、`/camera/front_left/image/compressed`、
`/camera/front_right/image/compressed`、`/camera/rear/image/compressed`。
これらは初期選択で、各Cameraパネルのプルダウンから記録内の別画像へ切り替えられる。
**このデモの複数映像は同じ元映像の色違いであり、異なる実視点ではない。**
同時表示の構成を示している。元画像は320×240なので、表示を拡大しても元の解像度は増えない。

## 映像・空間・動きを照合する — navigation

![3D Scene、Camera overlay、BEV、速度とヨーレート](images/layout-navigation.png)

左上の3D SceneでLaserScanと計画経路、右上の映像でCamera overlay、
下段でBEV・速度・ヨーレートを確認する。曲がる直前の映像と経路、速度変化を照合する入口に使える。
Plotのクリックでも再生位置を移動できる。

```sh
cargo run -p viewer-native -- --mcap /tmp/mcap-readme-demo/recording.mcap --layout navigation
```

フロント画像、`/planning/path`、`/odom`を使う。Scanは既定の`/scan`で、別の記録では
`--point-cloud-topic`でPointCloud2等へ変更できる。TF・固定frame・geometry・Camera calibrationは
通常の設定を使い、レイアウト内で物理倍率を変更しない。
撮影用Pathは生成データ、同梱Camera calibrationは実測値ではない（画面内にも表示）。

## Topicの値と時系列を調べる — inspection

![Topic一覧、LaserScanの値、速度とヨーレート](images/layout-inspection.png)

左のTopic一覧から現在位置以前の最新メッセージを調べ、右で速度とヨーレートの記録全体を見渡す。
数値の変化が気になる時刻へPlotで移動し、Inspectorでheaderや配列を展開する。
配列はページ単位、グラフは点数を制限した概要であり、全履歴のデコード済み常駐を前提にしない。
Camera・Sceneは表示せず、カメラのdecodeも要求しない。

```sh
cargo run -p viewer-native -- --mcap /tmp/mcap-readme-demo/recording.mcap \
  --layout inspection --inspect-topic /scan
```

速度・ヨーレートは`/odom`をqueryする。Inspectorは完全なros2msg依存定義が必要。
別のTopicを選んでも、カメラ等の再生対象を勝手に増やさない。

## 別の記録へ合わせる

これらは同梱デモに合う名前を設定した出発点。
[cameras](../config/layouts/native_cameras.json)、[navigation](../config/layouts/native_navigation.json)、
[inspection](../config/layouts/native_inspection.json)のJSONに、配置・比率・パネル設定がある。
トピック名が異なる記録では`cameraTopic`等を合わせてからNativeを再ビルドする。
現在はコンパイル時に埋め込むpresetであり、外部JSONの実行時読み込み・GUIでのlayout編集機能はない。
`--camera-topic`は固定された複数panelの`cameraTopic`を一括置換する引数ではない。

既存の`standard`、`monitor`、`showcase`、`explore`、`health`、`scene`も利用できる。
Browser版は現在のmonitor構成を維持し、ここで紹介した3種類の選択はNative用。
