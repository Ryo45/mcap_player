# fastwriteの明示的な変換経路

合成FHD rgb8・1カメラ30Hz＋LaserScan 10Hz、3秒の120メッセージを、ROS 2 Jazzyの
`rosbag2_py.SequentialWriter`、`storage_preset_profile='fastwrite'`で書いた。
実センサの録画ではなく、実際のROS 2 fastwrite writerを通す互換性試験。
入力559,937,768 bytes、変換後538,567,885 bytes。ファイルSHA256は[fixtures.json](fixtures.json)。

公式CLI `mcap 0.3.0 (21755244b) mcap-rust/0.25.0`を使用。
実行バイナリのSHA256は`5d4100573fab880f1c6400952466275bb3b32c3d230a54f7c380ee0d08e59eef`。
下記のコマンドで変換し、ROS 2 readerとRust readerの両方で確認した。

```sh
# 先にtools/performance-probe/README.mdの手順で標準fixtureを準備する
mkdir -p target/fastwrite-check
docker compose run --rm dev python3 tools/ros-fixture/fastwrite_fixture.py create \
  target/performance-regression/data/raw-fhd-30hz.mcap target/fastwrite-check/fastwrite
mcap compress target/fastwrite-check/fastwrite/fastwrite_0.mcap \
  -o target/fastwrite-check/indexed.mcap --order preserve --chunk-size 4194304
docker compose run --rm dev python3 tools/ros-fixture/fastwrite_fixture.py verify \
  target/fastwrite-check/fastwrite target/fastwrite-check/indexed.mcap
# 環境変数のパスは絶対パスにする（cargo testの作業ディレクトリはcrate側）
MCAP_FASTWRITE_INPUT=/absolute/path/to/fastwrite_0.mcap \
MCAP_INDEXED_OUTPUT=/absolute/path/to/indexed.mcap \
  cargo test -p performance-probe --test fastwrite real_rosbag2 -- --ignored --nocapture
```

ROS 2 readerでtopic名・型・serialization、120件の保存順・時刻・serialized payloadが一致。
順序付きメッセージfingerprintは`b0f9650e4feb906e5431227ef56de2c16a78ea9bea43433f50887dc126039d40`。
Rust側ではschema/channel、log/publish time、sequence、payloadも全件一致した。
別の小fixtureでは逆順・同時刻・Metadata・Attachmentの保持と空channelの省略を確認した。

| 確認 | 結果 |
|---|---|
| 変換前をNativeでopen | ChunkIndexRequired |
| 変換前をWebでopen | 変換コマンドを表示。約54 ms、Blob.arrayBuffer読取5,565 bytes／約534 MiB |
| 変換後のNative本番API | 120/120件配送、90画像decode、feature error/dropなし |
| 変換後のWeb実GPU | FHD focus約29.8 Hz、再生速度約0.994、4地点Seek成功 |

Web拒否確認はsoftware Chromeでの機能試験。Blob.arrayBufferのbyte数を観測し、
全ファイル読取にならないことを確認した。性能値の比較には使わない。
変換後は1920×1080 viewport、focus 960×540、Scene非表示、NVIDIA WebGPUの標準計測器を使った。
Seek中央値約56 ms、Chrome tree PSS最大約1,101 MiB。
いずれも1回の機能確認・cache-allowed条件であり、fastwrite直接再生との性能比較ではない。
NativeはGUIを含まない本番API試験。生の結果は[native.json](native.json)、[web.json](web.json)、[rejection.json](rejection.json)。

標準Dockerチェックでfmt、workspace Clippy/tests、WASM check、Web release build、ROS Live testsを通過。
外部CLIを使う2テストも別途明示実行して通過した。

UIから変換を起動する機能、直接の制限付き順次再生、独自converterは追加していない。
プレイヤーの自動全走査禁止を維持し、公式CLIによる前処理を再現可能にした。
