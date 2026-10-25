# 継続する性能計測

通常の回帰確認は `regression.py` を入口にする。日付付きレポートは実験の記録として残し、
現在の実行スクリプトから参照しない。製品ソースへのpatch適用・文字列置換・書き戻しは不要。
Pythonは標準ライブラリだけを使う。Linux、同じマシン、cache-allowed条件で比較する。

## ビルドと入力準備

```sh
docker compose run --rm dev bash -lc \
  'cargo build --release -p performance-probe --locked && cp /home/dev/target/release/performance-probe target/release/performance-probe && cd apps/viewer-web && trunk build --release'
python3 tools/performance-probe/regression.py prepare
```

コード変更後は計測前にビルドを再実行する。`run` は指定された成果物を測り、ビルドは行わない。

初回に `target/performance-regression/data` へ固定したmessage集合の入力を生成し、以後はSHA256を検証して再利用する。
同じbaselineとの比較には生成済みMCAPをキャッシュして使う。再生成時のmetadata配置やcodec出力まで
同一と仮定せず、SHA256が変わった入力は比較を拒否する。
fixtureの定義は [regression_cases.json](regression_cases.json)。変更時には新しい入力ディレクトリとbaselineを使う。
大きな実験を並行して走らせず、入力生成とビルドを終えてから計測する。

| ケース | 主に見るもの |
|---|---|
| FHD rgb8・1カメラ・30Hz | Raw変換と入力追従 |
| 4K JPEG・8カメラ・10Hz・64MiB設定 | 選択読み込み、JPEG、focusと背景の配分 |
| 同じJPEG・512MiB設定 | 大Chunk、再展開、UI停止、メモリ |
| 同じ記録のScanだけ | 選択・Chunk pruning |
| PointCloud2・6万点・10Hz | CDRと点群feature更新 |
| 512MiBで同じ場所を往復 | 初回と再訪の展開・feature復元 |

Native経路は `performance-probe` が本番APIを呼ぶ。Forwardは20msの論理tick、
CameraはNative decoderの表示サイズを考慮する経路。SeekはGPUを含まない完全なfeature復元。
これはNative GUIの可視FPS／FIFO待ちを測る試験ではない。CLIはdelivery count、feature error、
前後関係を検査し、PointCloud2ではfixtureのtopicを実際にbindする。

Webは通常のrelease成果物を開き、既存のfile input／Play／timelineを操作する。
viewportは1920×1080、DPR=1、focusは960×540、Sceneは非表示。16×9のcanvas内容hashを観測し、
同じ内容の再描画を新画像と数えない。観測のreadback負荷も含む。
JPEGはfocusと背景の最低更新頻度を別に測る。Rawは再生後の4地点のSeekも測る。
WebのSeek指標はcursor確定と新しい画像jobのcanvas反映までであり、厳密なpredecessorの画素照合試験とは区別する。

## baseline作成と回帰確認

```sh
python3 tools/performance-probe/regression.py run --engine native \
  --output target/performance-regression/native-baseline
python3 tools/performance-probe/regression.py run --engine native \
  --baseline target/performance-regression/native-baseline/suite.json \
  --output target/performance-regression/native-current
```

Webは実GPUを確認できるLinux/X11のホスト上で実行する。例えばこの開発環境では:

```sh
python3 tools/performance-probe/regression.py run --engine web --wake-display \
  --vulkan-icd /usr/share/vulkan/icd.d/nvidia_icd.json \
  --output target/performance-regression/web-baseline
python3 tools/performance-probe/regression.py run --engine web --wake-display \
  --vulkan-icd /usr/share/vulkan/icd.d/nvidia_icd.json \
  --baseline target/performance-regression/web-baseline/suite.json \
  --output target/performance-regression/web-current
```

`--wake-display` は消灯中なら起こし、元がOffなら終了時に戻す。X11のscreen saverもresetするが、電源ポリシーや画面ロックは変更しない。
ハードウェア条件ではVulkanを有効にし、実際のWebGPU adapterも検査する。
Canvas/WebGLがGPUでもWebGPUはソフトウェアの場合があるため、GPU名だけで通過させない。
消灯・非表示・ソフトウェアへのfallbackは失敗。`--gpu software` は別baseline用の補助条件。

各ケースで最低3回、独立processを起動する。反復ごとにケース順を反転する。
`suite.json` とケースごとの出力・Chrome log・sampleを保存する。
baseline指定時は許容幅を超えた劣化、試行失敗、ケース欠落、入力／環境／計測器の不一致でexit 1。
既存結果を上書きせず、自動的にbaselineを更新しない。

オフライン比較もできる:

```sh
python3 tools/performance-probe/regression.py check \
  --baseline path/to/baseline/suite.json --current path/to/current/suite.json
python3 -m unittest discover -s tools/performance-probe/tests -p 'test_*.py'
```

閾値は `regression.py` の `LIMITS`。各processの値の中央値を比べ、相対・絶対両方の許容幅を
超えた変化を検出する。例えば処理時間は20%かつ10ms、Webのfocusは15%かつ1Hz、PSSは20%かつ64MiB。
これらは検出用の初期値で、製品のSLOではない。短い試行なので長期傾向は生データも確認する。
Nativeのhigh-water RSSと、Chrome treeのsample PSSは別の指標として保存する。

通常のテストと異なり性能比較には固定した実行環境が要る。常設GPU runnerはこのリポジトリでは
設定していない。専用runner／定期実行では上のbaseline付きコマンドの終了コードを利用する。
Native GUIの画面反映や長時間再生は、個別のDesktop計測を併用する。

## 巨大Chunkの追加試験

```sh
python3 tools/performance-probe/measure_web_recording.py \
  --data /path/to/recording.mcap --duration-seconds 59.9 --cameras 8 \
  --seconds 10 --wake-display --vulkan-icd /usr/share/vulkan/icd.d/nvidia_icd.json \
  --output target/performance-regression/large-chunk
target/release/performance-probe generate-sparse-chunk /tmp/one-20gib.mcap 20
```

単独Web計測も専用Chromeと専用HTTP serverを使い、CDP応答とは独立してPSS／実行時間を監視する。
PSS既定4500MiB（約4.39GiB）は**実験用の停止条件**。製品上限ではなく、sample間の超過はあり得る。

疎ファイルは有効なFHD rgb8・4カメラ10Hzの記録で、20GiBを超える単一の非圧縮Chunkを持つ。
ゼロ領域をholeとして作るので、見かけのfile bytesと実ディスク使用量を両方出力する。
大きなoffset・読み込み範囲・保持量の確認用であり、実データの圧縮率やcold diskの転送速度の根拠にしない。

過去の比較対象との寸法合わせや段階別trace用builderは、その実験専用のまま残す。
通常の回帰確認はそれらのsource書き換えに依存しない。

## Seek候補の破棄される旧状態を測る

`FeatureRuntime::stage_restore`を変更せず、旧点群を持つruntimeと、同じ経路で事前に空にした
runtimeを比較する計測。空の対照は新しい実装ではなく、不要な旧状態cloneのコストを分離するためのもの。

```sh
cargo build --release -p performance-probe --example seek_clone --locked
python3 tools/performance-probe/measure_seek_clone.py --output target/seek-clone-new
```

6万／100万点、XYZ／RGBA／RGBA＋intensityを各3process、10回warmup後50回測る。
state消去だけ、同サイズの復元decode、失敗を分け、時間・Rust allocator要求総量・追加live要求量を記録する。
I/O・MCAP展開・GPU・WASMは含めず、RSS/VRAM測定ではない。
結果と実装判断は[Seek候補の調査](../../docs/performance/2026-10-25-seek-candidate-copy/README.md)。
