# rosbag2 fastwriteを開く

ROS 2 MCAP storageの`fastwrite`はChunkを作らない設定であり、「ファイル全体が1個の巨大Chunk」ではない。
mcap_playerのNative/Webは索引付きの入力を使う。fastwriteを開くとChunk Index不足を表示し、
自動の全走査は行わない。今回追加したのは具体的な変換案内と検証であり、直接の順次再生ではない。
[ROS 2の設定](https://github.com/ros2/rosbag2/blob/jazzy/rosbag2_storage_mcap/README.md)を参照。

## 明示的に変換する

[公式MCAP CLI](https://mcap.dev/guides/cli)を使い、新しい出力ファイルを指定する。
以下はCLI **0.3.0**で検証した。古いCLIではオプションや保持対象が異なるため、version/helpを確認する。

```sh
mcap --version
mcap compress INPUT.mcap -o OUTPUT.mcap --order preserve --chunk-size 4194304
```

これは入力全体を読む独立した処理で、出力ファイル分の空き容量と変換時間が必要。
元ファイルは残す。終了コードと出力を確認してから、NativeまたはWebで`OUTPUT.mcap`を開く。
既定の圧縮はZstd。4 MiBはChunkの目標値で、単一メッセージが大きければ超える。
全メッセージの並べ替えが目的ではないため`--order preserve`を指定する。
分割記録では各partを変換し、出力だけを置いた別ディレクトリをNativeに指定する。
`ros2 bag reindex`のmetadata.yaml再生成と、MCAP内部の索引生成は別の処理。

壊れた記録の回復は別の作業。通常のfastwrite変換に`recover`を使って、欠損を容認した成功と混同しない。

## 保持の確認と制約

CLI 0.3.0の検証では、保存順、payload、log/publish time、sequence、使用中channelのschema/metadata、
Metadataレコード、Attachmentが保持された。一方、**メッセージが0件のchannelは省略された**。
ファイル全体の完全な複製とは扱わず、元ファイルを保存する。任意の未知レコードの保持も保証しない。

検証は[2026-10-24の結果](verification/2026-10-24-fastwrite/README.md)に記録した。
[Native / Lichtblick Desktopの同じRaw画像による比較](performance/2026-10-24-fastwrite-desktop/README.md)もある。
再実行には外部CLIを明示的に指定する（通常の`cargo test`では外部依存テストをskipする）。

```sh
MCAP_CLI=/absolute/path/to/mcap cargo test -p performance-probe --test fastwrite official_conversion -- --ignored --nocapture
```

このテストは逆順・同時刻メッセージ、空channel、Metadata、Attachmentを含む小さなfixtureを作り、
変換結果の内容と、Nativeが変換前を拒否・変換後を開けることを検証する。
ROS 2 writerを使う追加手順は検証結果のREADMEに記載。
