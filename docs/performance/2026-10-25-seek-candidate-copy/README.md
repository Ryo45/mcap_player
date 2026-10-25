# Seek候補でclone後に捨てる旧点群のコスト

後続の[空の候補構築の実装と性能退行チェック](../2026-10-25-empty-seek-candidate/README.md)で改善した。
以下は実装前の調査時点の記録。

結論は**大点群のSeek向けに改善する価値がある**。100万点では旧点群のためだけに
12〜20 MBを確保し、破棄だけのrestoreに約0.51〜1.18 ms使っていた。
同サイズの復元decodeを含めた処理では、空状態の対照との差が約15〜19%。
一方、6万点では約0.01〜0.03 msで、普段の小さなScan/点群で大幅なSeek高速化を期待する変更ではない。

**今回は計測と実装判断まで。製品のFeatureRuntime/controller/stateは変更していない。**
比較先は提案実装ではなく、既存APIで事前に旧状態を消したruntime。
以下は実装後の改善率・アプリ全体のSeek改善率を示すA/Bではない。

## 確認した経路

`viewer-core/src/feature/runtime.rs::stage_restore`は`self.clone()`の後に各controllerをresetする。
PointCloudFrameのXYZ、RGBA、intensityはVecなのでdeep copyされ、resetの`cold_seek()`で解放される。
PathのVecとTFのmap/historyにも同じclone→消去があるが、この計測はPointCloudだけを対象にした。
Cameraの画像payloadはBytes共有であり、同じサイズの画像全体がdeep copyされるとは限らない。

Nativeのtransactional restoreとWebの候補stageはこの共通処理を使う。
本番の「候補が完成するまで正本を変更しない」保証は維持すべきであり、正本を先にclearする案は採らない。

## 結果

Linux x86_64、Intel Core Ultra 7 265KF、Rust 1.91.1、releaseビルド。
同一process内で旧点群あり／空対照の順を交替し、各条件3つのfresh process。
10回warmup後、各50回の中央値を取り、さらにprocess間の中央値を掲載。
共有ホスト、CPU pinなし、warm-memory条件。点群はすべてfiniteな合成ROS 2 PointCloud2。
属性・点数は実際にdecodeされたframeで確認する。MBは10進（1 MB = 1,000,000 bytes）。

旧状態を消すだけの`stage_restore(target, &[])`:

| 旧点群 | 旧Vecの量 | 旧点群あり ms | 空対照 ms | 旧点群ありの追加allocator要求量 |
|---|---:|---:|---:|---:|
| 6万点 XYZ | 0.72 MB | 0.0105 | 約0.0001 | 0.72 MB |
| 6万点 RGBA | 0.96 MB | 0.0145 | 約0.0001 | 0.96 MB |
| 6万点 RGBA＋intensity | 1.20 MB | 0.0195 | 約0.0001 | 1.20 MB |
| 100万点 XYZ | 12 MB | 0.5118 | 約0.0002 | 12 MB |
| 100万点 RGBA | 16 MB | 0.8593 | 約0.0003 | 16 MB |
| 100万点 RGBA＋intensity | 20 MB | 1.1781 | 約0.0004 | 20 MB |

同じサイズの点群1件を復元する`stage_restore(target, &[message])`:

| 点群 | 旧点群あり ms | 空対照 ms | 差分／旧点群あり | allocator要求総量（旧あり→空対照） |
|---|---:|---:|---:|---:|
| 6万点 XYZ | 0.3107 | 0.3003 | 3.4% | 1.44→0.72 MB |
| 6万点 RGBA | 0.3669 | 0.3526 | 3.9% | 1.92→0.96 MB |
| 6万点 RGBA＋intensity | 0.4123 | 0.3836 | 7.0% | 2.40→1.20 MB |
| 100万点 XYZ | 3.3631 | 2.8600 | 15.0% | 24→12 MB |
| 100万点 RGBA | 4.6130 | 3.8558 | 16.4% | 32→16 MB |
| 100万点 RGBA＋intensity | 5.6496 | 4.5931 | 18.7% | 40→20 MB |

要求量にはframe_id等の小さなallocationもあるため、丸め前の値は表のMBに数bytes加わる。
元の正本は両条件とも保持したまま。入力payload生成・初回decode・対照の準備・戻り値のdropは計測外。
stage内部で旧cloneをclearする際のdropは計測内。

失敗はCDR headerだけの不完全な点群を渡す。100万点の旧状態ありでは約0.51／0.92／1.23 ms、
追加要求量12／16／20 MB。空対照では1µs未満、要求0 bytesだった。
成功・失敗後も正本のpoints pointer、revision、countersが変わらないことを毎回検査した。
これはstageの検査で、ソース位置・cursorまで含む新実装のrollback検証の代用ではない。

## メモリの読み方

計測binaryだけにSystem allocatorへ委譲するカウンタを置き、Rustの要求bytes・回数・live要求量の
増分最大を測った。memcpy byte数そのもの、allocator内部の管理量、RSS/PSS、GPU VRAMではない。
時刻計測にもallocationカウンタのatomic演算が入る。両条件で同じ計測器を使った。

**成功時の追加live要求量のピークは減らなかった。** 100万点のRGBA＋intensityでは両条件とも約20 MB。
現状でも旧cloneを捨ててから新しい20 MBのframeをdecodeするため、余計な旧copyと新frameは
同時には保持されない。正本の旧frame＋候補の新frameというtransactionに必要な保持は残る。
削減が見込めるのは割り当て総量とコピー処理。空の位置へのSeekやdecode前に失敗するSeekでは
一時的な追加live要求量も削減できる。メモリ常駐量が常に半分になるとは主張しない。

## 実装する場合の最小案と条件

独立した空の候補を組み立て、現状と同じmessagesで復元し、成功時だけcommitする。
新しいmanager/traitやdecoded frameの共有化は、この変更に必要ない。

- runtimeのunknown_streams、累積processing_time、controllerのカウンタを引き継ぐ。
- streamのrouting、PointCloud2/Scanのdecode種別、CameraのRaw/JPEG種別・topics・focusを引き継ぐ。
- Path／Telemetry／PointCloud／TFは現行resetと同じrevision遷移にする。単にDefaultへ戻すと
  revisionの再利用によりpresentationの更新検出を壊す可能性がある。
- Cameraのpendingと旧画像はコピーしない。focus、累積input/presented/by-camera countersを維持し、
  scheduleを現行resetと同じ基準へ戻す。statusのキー・Waitingの扱いも確認する。
- TFも現状restoreでは全clearされ、静的TFはRestorePlannerのPersistent入力から復元される。
  旧static mapをそのまま引き継いで、過去へのSeekに未来のstatic TFを残す変更にしない。
- 全feature成功前に正本・source position・cursorへcommitしない。途中featureのdecode失敗、
  未route、Webの古いgenerationでも既存のtransactionを維持する。

この方針はroute・counter・revisionだけを扱うcontroller/state内部の小さな候補構築で実現できる。
この実験の空対照はrevisionが余分に1回進み、旧decoded stateも初めからない。
そのまま製品候補として使える実装ではない。実装後は新旧の成功・失敗・前後Seek・focus・
revision・static TF復元を比較し、この計測を再実行して実際の削減量を確認する必要がある。

優先度は大点群の反復Seek向けの局所改善として中。I/O・Chunk展開・GPU upload等を含む
GUI Seek全体の最適化や、Web/WASMで同じmsの短縮が得られることまでは今回測っていない。

## 再現と検証

```sh
cargo build --release -p performance-probe --example seek_clone --locked
python3 tools/performance-probe/measure_seek_clone.py --output target/seek-clone-new
```

実装ソースと共通stageのSHA256、CPU、binary hashは[manifest](manifest.json)、
各processの中央値・P95と全条件は[summary](summary.json)、全観測は`runs/*.json.gz`。
18process、計5,400回のtimed stage（別にwarmup）がすべて完了した。
`cargo fmt --all -- --check`、performance-probe全targetのClippy、PointCloud2の既存回帰テスト、
小fixtureでのallocator/結果smokeと全matrix内の状態保持検査を実行。
製品変更がないためworkspace全体やGUIの再計測は実施していない。
