# 空のSeek候補: 実装と退行チェック

[事前計測](../2026-10-25-seek-candidate-copy/README.md)を受け、`FeatureRuntime::stage_restore`の
runtime全体のclone→resetを、設定・統計を引き継ぐ空の候補構築に置き換えた。
正本は変更せずに復元し、全featureが成功した候補だけをcommitする。

**点群復元の交互比較・Native代表5ケース・Web代表3ケースは、全て設定した許容幅内で通過した。**
最初に見つかったNativeのI/O条件差は下に分け、失敗した試行も保存した。
製品コードの最終diffと計測済みbinary／Web assetsの一致を確認してからコミットする。

## 変更内容と保証

- PointCloud／Path／TFの古いVec・履歴をコピーしない。元のrevisionを基準に、現行clearと
  同じ繰り上がりを行う。u64 wrapもテストする。
- 各controllerのstream routing、Raw/Compressed Camera・PointCloud2/Scanの種別、
  Camera focus、累積counter／input／presented／by-camera統計を引き継ぐ。
- Cameraのpending・旧画像は引き継がず、statusをWaitingへ、background scheduleを基準位置へ戻す。
- TFは空からplannerのDynamic historyとPersistent static入力で復元する。
  過去へのSeekで未来のstatic TFが残る経路は増やさない。
- unknown stream counterとprocessing timeの累積を維持。PointCloud無効時は候補でも無効。
  不正入力や未routeの復元失敗、古いWeb generationで正本を先に更新しない。

変更はviewer-core内部の具体的なcontroller/state候補構築に閉じる。
新しいmanager・trait・共有decoded history・renderer補正は導入していない。

## 点群復元: 変更前後を交互に比較

保存した変更前binaryと変更後binaryを同じ入力で交互に実行。
6万／100万点 × XYZ／RGBA／RGBA＋intensity、各3process、各50回（warmup 10回）。
空の位置への復元・点群1件の復元・不正CDRによる失敗、さらに空state対照も同じ計測器で確認した。
中央値はprocess内50回の中央値をさらにprocess間で取る。

100万点の点群1件を復元するstage（MBは10進）:

| 属性 | 変更前 ms | 変更後 ms | allocator要求総量 前→後 |
|---|---:|---:|---:|
| XYZ | 3.363 | 2.816 | 24→12 MB |
| RGBA | 4.600 | 3.727 | 32→16 MB |
| RGBA＋intensity | 5.719 | 4.577 | 40→20 MB |

古い点群のための12〜20 MBの確保がなくなった。空の位置への復元・decode前の失敗は
point-cloud-onlyの全条件でallocator要求0 bytes。成功時のピーク追加live要求量は前後とも
12／16／20 MBで、正本と新しい候補の両方が必要な点は変わらない。RSSが半分になる意味ではない。

`compare_seek_candidate.py`の退行条件は中央値10%かつ30µs、P95は20%かつ100µsを超える増加。
allocator要求bytes・回数・追加live要求量は増加不可。36process・10,800 timed stageを検査して成功。
しきい値超過、allocation増加、欠落・非有限値のnegative controlも13件のPythonテストに含む。
[判定と全条件](clone-summary.json)、`raw/clone-comparison`に全観測を保存。
旧binaryと新binaryのexample差分は説明文のみで、計測区間は同じ。

## Native代表負荷

固定MCAPの同一SHA256を使い、各条件3process。時間は各run値の中央値。
このheadless試験は本番I/O・feature・Native image decode APIを使い、GUI/GPUの表示FPSではない。

| ケース | 変更前 ms | 変更後 ms | peak RSS 前→後 MiB |
|---|---:|---:|---:|
| FHD Raw 30Hz | 789.1 | 788.4 | 75.27→75.24 |
| 4K JPEG 8カメラ | 4952.4 | 5028.0 | 228.19→228.07 |
| 同記録のScan選択 | 1066.9 | 1060.3 | 200.30→200.26 |
| 6万点PointCloud2 | 25.77 | 25.48 | 14.55→14.45 |
| 512MiB Chunkの再訪Seek | 0.012 | 0.012 | 1027.33→1027.37 |

最後の行は再訪Seekの中央値で、上4行の記録全体の処理時間とは別。
JPEGの約1.5%増加を含め、既存しきい値（通常処理20%かつ10ms、再訪Seek25%かつ1ms等）内。
全15試行が正常完了し、配送件数・feature error等の検査も通過。
[Native判定](native-comparison.json)にproblems=[]を記録した。

## Web代表負荷

Chrome 143 / NVIDIA RTX 5070 Tiの実WebGPU。1920×1080、focus 960×540、Scene非表示。
通常のrelease assetsを開き、実canvasの内容変化、heartbeat、全Chrome子孫processのPSSを観測する。
各条件3回、各アプリprocessを独立起動。背景最低Hzはrun内の最も低い背景Cameraの値。

| ケース | focus Hz 前→後 | 背景最低Hz 前→後 | peak PSS 前→後 MiB |
|---|---:|---:|---:|
| FHD Raw 30Hz | 29.23→29.23 | — | 1133.3→1133.0 |
| 4K JPEG 8カメラ | 9.63→9.69 | 2.28→2.29 | 1647.2→1627.3 |
| 同JPEG、512MiB Chunk | 9.65→9.66 | 2.28→2.29 | 1578.5→1574.4 |

Rawの4地点Seekのrun内中央値をさらに集計すると112.35→99.30ms。
今回のWeb fixtureは百万点のPointCloudではなく、Camera/Seek/時間窓処理に悪影響がないかの確認。
open・再生速度・heartbeat・PSSも含む既存判定が全9試行で通過した。
[Web判定](web-comparison.json)にproblems=[]を記録。各反復の値は`raw/web-warm-*`に保存。
短いcache-allowed試験・共有ホストであり、あらゆる入力で速度が不変という証明ではない。

## 最初の失敗と計測条件の修正

最初のNative suiteでは6万点のworkloadが24.80→42.24msとなり、既存gateが失敗した。
feature decodeは約14.7msで変わらず、差はread_msだった。
旧suiteは冒頭に全入力をSHA256検証するだけで、途中の大きな入力によって小さな入力がcacheから
追い出される。変更前の実disk readは各86,016／35,663,872／0 bytes、変更後は
36,012,032／0／26,288,128 bytesとなり、比較した3反復のI/O条件が異なっていた。

`regression.py`を各caseの起動直前にその入力を検証・readする方式へ修正し、Native/Webとも
変更前から全suiteを取り直した。readは計測区間外、判定しきい値は変更していない。
cache policyもsuiteへ記録し、異なるpolicyの比較を拒否する。
修正後の6万点では前後全6試行のdisk readが0、25.77→25.48msで通過した。
変更前binary自体にも一度45msの旧試行があり、変更後だけの現象ではなかった。

最初のsuite／失敗した判定／個々のI/O値も`raw/native-before`、`raw/native-after`、
`native-after.log.gz`へ残した。最初のWeb baselineも`raw/web-before`に保存。
成功試行だけを抜き出した再集計や、しきい値の緩和ではない。

## 正しさ・ビルド・再現

標準Dockerチェック（fmt、workspace Clippy/tests、WASM check、Web release、ROS Live tests）通過。
加えた回帰検証は7 Cameraのroute/focus・統計・pending破棄、空Seekのrevision遷移、
Raw Cameraの候補route、無効PointCloud、未route・途中feature失敗のrollback、revision wrap。
既存のstatic TF過去復元・索引Seek・stale Web generationのテストも通過した。
ログは`checks.log.gz`。最終製品diffは[product.patch](product.patch)。

```sh
python3 tools/performance-probe/compare_seek_candidate.py \
  --before target/seek-candidate-ab/clone-before --after target/release/examples/seek_clone \
  --output target/seek-candidate-comparison-new
python3 tools/performance-probe/regression.py run --engine native \
  --baseline path/to/native-before/suite.json --output target/native-after-new
python3 tools/performance-probe/regression.py run --engine web --wake-display \
  --vulkan-icd /usr/share/vulkan/icd.d/nvidia_icd.json \
  --baseline path/to/web-before/suite.json --output target/web-after-new
```

beforeは製品9afa483から保存したbinary/assets。before suiteは新しい計測器とその保存物で作る。
[builds.json](builds.json)に実binary/assetのSHA256と製品ソースを対応付けた。
suiteのsource項目は計測起動時のworkspaceを表すため、保存済みbefore binaryの製品ソースは、
編集前に同じSHA256のbinaryで取得した元suiteから記録した。変更後は計測時のproduct diff hashと
コミット対象のdiff hashが一致することを確認する。
