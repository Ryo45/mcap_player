# ダウンロード用サンプルMCAP

GitHub Releasesの[sample-data-v1](https://github.com/Ryo45/mcap_player/releases/tag/sample-data-v1)でデータを配布する。
アプリのビルド済みバイナリは含まない。MCAP本体をGitやPages artifactへ追加しない。

## 試す

`turtlebot3-7cam-fhd-10s.mcap`をダウンロードし、[ブラウザ版](https://ryo45.github.io/mcap_player/)の**Open MCAP**で開く。
ファイルは約18.8MiB。`sample-data-notices.zip`にはライセンス・出所・検証結果を収録している。
ローカルファイルはアップロードされない。画像トピックの切り替え、再生・停止、前後Seekを試せる。

Nativeはソースからビルドして次のように開く。

```sh
cargo run --release -- --mcap /path/to/turtlebot3-7cam-fhd-10s.mcap \
  --layout monitor --no-platform-geometry
```

この記録の機体は改造したTurtleBot3 Burgerで、同梱の車体geometryはWaffle Pi用なので非表示にする。
Webの車体表示とCamera overlayは同梱デモ設定であり、この機体の実形状・校正の検証用ではない。
Sceneの点群・Pathとカメラ映像を確認するサンプルとして使う。

## 内容と出所

- 7方向のCamera：1920×1080、JPEG、要求10Hz。元映像の色違いではなく、それぞれGazeboが描画する視点。
- `/scan`、`/odom`、`/tf`、`/tf_static`、`/planning/path`と速度・加速度などの派生信号。
- 索引・Zstd圧縮・約1MiBのChunk。MCAP log timeは収録時刻、メッセージのheaderはシミュレーション時刻で、別の時間軸。
- Pathは移動指令から生成した予測軌跡。実機収録・自律プランナーの評価データではない。

以前の同梱fixtureとローカル収録環境との対応は確定できなかったため、公開用に新規収録した。
収録時点のforkコミット、未コミット変更、Docker image ID、収録用コピーのファイルhashは同梱の`provenance.json`で記録する。
実際の時間幅・画像数・更新率・Seek検証結果は`verification.json`、ファイル整合性は`SHA256SUMS`を参照する。

## ライセンスの確認

ローカルの`Ryo45/turtlebot3`の基点は、fork元にも存在する
[`1f67e8d477df3e91729a03e43a9cd71cc67addfa`](https://github.com/ROBOTIS-GIT/turtlebot3/commit/1f67e8d477df3e91729a03e43a9cd71cc67addfa)。
現在の`7cd9de3e09e4e37d0fbc6ac138eef163ecda4fe7`と作業ツリーの`LICENSE`は、基点のファイルとバイト単位で一致した。
SHA-256は`f9946796a8a5bd5981565142fa42e3b0cc36aef6f80e86beaf5905b3c0fef1b5`。
追加された収録スクリプト・モデルに別ライセンスの指定は見つからなかった。

実際に使った`ros-jazzy-turtlebot3-gazebo`は`2.3.7-1noble.20260615.175208`。
そのpackage.xml・Debian copyrightと[上流2.3.7](https://github.com/ROBOTIS-GIT/turtlebot3_simulations/tree/2.3.7)はApache-2.0を示す。
使用worldの参照を確認し、ROBOTIS arenaの`hexagon.dae` / `wall.dae`と機体meshを使用している。
元worldが参照するFuelのGround Plane / Sunは、公開用収録では基本形状・光源に置き換え、外部取得なしで実行した。

録画・追加部分はApache-2.0として配布する。埋め込みROSメッセージ定義にはApache-2.0およびBSD-3-Clauseの素材が含まれる。
`NOTICE.txt`と両ライセンス原文を同梱し、**単独で取得されるMCAPにもattachmentとして埋め込む**。
ソフトウェアのMITライセンスを録画全体へ一律に適用しない。

## 確認したこと

公開用の記録を新規収録し、message payload・log/publish time・sequenceを保持して再圧縮した。
索引とライセンスattachmentを検証し、FHDの7カメラは各96フレーム、記録上の更新率は約9.97〜10.01Hzだった。
SessionPlan / FeatureRuntimeを使う前後5回のSeekと、画像・点群からodomへのTF解決が成功した。
公開Web版でも7カメラ表示、前後3回のSeek、3秒間の再生を確認し、エラーはなかった。
これは機能確認であり、性能比較ではない。今回の記録でNative GUIの目視確認は実施していない。

収録・加工に使ったローカル専用ツールはアプリ本体に追加しない。
収録時のコピーでは元リポジトリを変更せず、画像サイズ・worldの地面と光源・静的TFを調整した。
GazeboのノイズやROSの配送時刻により、再収録時のバイト単位の一致は保証しない。
