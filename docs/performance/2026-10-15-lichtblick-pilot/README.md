# LichtblickとのWeb GUI比較：最初のパイロット

2026-10-15。Foxglove StudioのforkであるLichtblick v1.29.1と、mcap_player Webを比較した。
目的は勝敗をつけることではなく、同じMCAPを使って再現可能な比較を始め、測定方法の不足も特定すること。
**初期読み込みと20秒再生のメモリ観測だけ**を行った。表示FPS、画像の鮮度、ランダムSeekは未測定。

## 対象

- mcap_player: `e67827e`の通常Web release build。計測用に製品Rustコードは変更していない。
- Lichtblick: [v1.29.1](https://github.com/lichtblick-suite/lichtblick/releases/tag/v1.29.1)。
  Docker配布物をdigestで固定した。
  `ghcr.io/lichtblick-suite/lichtblick@sha256:230ed53813f11ab5d23c47380f94c6d358438ecf15c3a7e7de9625b07c437611`。
  同じtagの参照ソースcommitは`95713edb309d77589b130286642ab149995a5f7b`。
- 現行商用Foxgloveや旧Studioの結果ではない。
  [Lichtblickの公式README](https://github.com/lichtblick-suite/lichtblick)にforkの由来とMPL-2.0の説明がある。

Lichtblickの参照ソースでは、`McapIndexedIterableSource`が選択トピックを使ってindexed readし、
`IterablePlayer`がread-aheadとpreloadを扱う。相手が無条件に全トピック・全履歴を読むとは仮定しない。
ソースのcache上限という定数だけから、実測の常駐量や画像decode数を推定しない。

## 共通条件

- 同一のROS 2/CDR MCAP。JPEG 3840×2160、1 camera×10 Hz、20秒、200 images。
  `/scan` 360点×10 Hzも含む。非圧縮Chunk、目標8 MiB、mixed。
- 固定seedのgradient/noise、JPEG quality85、32種類を循環する既存generatorを使う。
  実カメラのエントロピーや運動を代表するデータではない。
- FHD viewport 1920×1080、DPR=1。focus canvasは960×540、thumbnailは112×63。
  これはFHD画面での比較であり、focus画像自体が1920×1080という意味ではない。
- 両方ともBrowser Localのfile inputへ同じpathを指定する。HTTP経由でMCAPを配信していない。
- 各runで専用Chrome process/profileを新規作成。3反復で実行順を交互にする。
  同時には動かさない。ChromeのバージョンとGPU情報は各raw JSONに保存。
- SwiftShaderを明示指定したheadless実行。**ハードウェアGPU環境での実用性や8camera×10 Hzの評価ではない。**
- GUIの画像領域を計測harnessのCSSで固定する。Lichtblickは[固定レイアウト](../../../tools/performance-probe/lichtblick-pilot-layout.json)の2 Image panels。
  mcap_playerはcamera panelの位置・寸法を固定し、Sceneを`display:none`にする。
  非表示canvasはCSS寸法0で、WebGPU hostがsurface描画をskipする。初期GPU資源やScene stateは残る。
- mcap_playerはScanもmaterializeする一方、Lichtblickの購読対象は`/camera/0`だけ。
  画像以外の仕事まで完全に同じにしたcomponent benchmarkではない。
  同じカメラを2領域へ表示する際の共有・再decode方法は製品に任せる。
- datasetのSHA256を毎run直前に計算する。**cache-allowed**でありcold diskではない。
  大量データの生成やビルドを計測中に並走させていない。

[dataset.json](dataset.json)にfile bytes、実Chunk数・最大サイズ・時間幅を保存する。
SHA256、クライアント資源、時系列samples、状態表示、canvas寸法は各run JSONにある。

## 指標と判定

`/proc`で専用Chromeと子孫processの`smaps_rollup`を読み、PSSを合算する。
ブラウザ本体、renderer、GPU process、Worker、file brokerのうち、そのprocess treeにあるものを含む。
HTTP/Caddy serverや計測Pythonは含めない。PSSは共有pageを各processへの持ち分で数える。
RSSの単純合計もrawに残すが、それは共有pageの二重計上を含むのでメモリ比較の主指標にしない。
GPU VRAMや全機のpage cacheを含む総消費量ではない。

- `before_open`: app起動後、ファイル選択前。
- `before_play`: 初期画像と寸法を確認した後、再生開始前。
- `peak_sampled_pss`: 再生中の約200 ms間隔の観測最大。真の瞬間peak/HWMではない。
- `settled`: 終端で停止後3秒。GCを強制していないため、必要最小heapやfeature state量とは異なる。
- `cursor_run_ms`: Play操作からUIが終端になるまで。mcap_playerはnormalized cursor=1、LichtblickはPlayへ戻ることを使う。
  **画像が全200枚描画された証拠ではない。** mcap_playerのadmission/drop counterはrawの状態表示に残す。
  Lichtblickの同等counterと画像frame IDは今回収集していない。
- `ready_ui_ms`: 各app固有のReady判定まで。判定と初期Seekが異なるため、横断の「最初の描画までの時間」として比較しない。
- CPU/I/Oの累積値はその時点の生存processだけを合算した補助情報。
  processが終了すると値が落ち得るため、CPU時間やMCAP I/O節約率の比較には使わない。
  `rchar`はMCAPだけのread量ではなく、`read_bytes`もpage cacheに依存する。

globalのJS例外と未処理Promise rejectionを記録し、canvasの実寸をassertする。
Lichtblickのpanel内log/警告までは収集・判定していない。
両方で初期画像のスクリーンショットも確認した。Seek復元や表示タイムスタンプの一致までは検証していない。

## 結果と解釈

3反復の中央値。クライアントprocess tree全体のPSS（MiB）。

| 条件 | mcap_player Web | Lichtblick |
|---|---:|---:|
| ファイルを開く前 | 315.69 | 438.50 |
| 再生開始前 | 453.02 | 1081.05 |
| 再生中の観測peak | 1124.42 | 1328.71 |
| 終端停止後3秒 | 1022.35 | 1023.48 |
| カーソル終端まで | 20.10秒 | 19.95秒 |

初期静止時には差があるが、再生後はほぼ同じ。少なくともこの条件では、
「mcap_playerなら再生後も大幅にメモリが少ない」とは言えない。
再生中peakはmcap_playerの方が小さかったが、画像の更新数・鮮度が未比較なので、
同じ品質の再生をより少ないメモリで達成したという結論にはしない。
両者の起動時の固定コスト、先読み、allocator/GC、画像decode一時領域等が含まれる。
CameraStateの保持量だけを示す数字ではない。

[summary.md](summary.md)、各3反復のraw JSONと[environment.json](environment.json)を参照。
画面条件は[mcap_player](mcap-layout.png)と[Lichtblick](lichtblick-layout.png)に保存した。
この20秒・1cameraという条件から、長時間記録でのメモリ傾向や、疎なトピック選択の効果は判断しない。
メモリの数値だけで、画質・表示更新頻度・seek性能まで優れているとは結論しない。

## 次の比較

1. 同じ多数トピック記録で「Scanのみ」「画像1台」「画像8台」の選択をそろえる。
   mcap_player Webは現在bundled bindingsでcameraを選ぶため、単にpanelを隠して未購読とみなさない。
   計測用bindingsと製品の既定bindingsを区別し、ビルド・設定hashを残す。
2. 画像に時刻と連番の識別子を持たせ、出力frameの更新数と鮮度を検証する。
   カーソルが1xでも画像が古い・更新が少ない場合を区別する。
3. 事前固定したrandom Seekと近傍Seek列で、操作から必要な全panelが正しいframeになるまでを測る。
   メモリを抑える代わりに再読み込みで遅くなる条件を残す。
4. none/zstd/lz4、実Chunkサイズ8/64/512 MiB、mixed/groupedを広げる。
   まず既存4秒の8cameraデータを使い、長時間・数十GBは別段階で追加する。
5. 実GPUのFHD画面、実データと長い記録、圧縮・分割を改善した記録も試す。
   20秒合成データの結果を全用途へ外挿しない。

## 再実行

データと結果は新規ディレクトリへ作る。MCAPはGitへ追加しない。
必要なrelease probe、通常のWeb build、Chrome、Dockerを用意する。

```sh
target/release/performance-probe generate \
  /tmp/oss-pilot-data-new/3840x2160-1cam-10hz-none-8MiB-20s.mcap \
  none 8388608 mixed 20 jpeg 3840 2160 32 10 1
cp tools/performance-probe/lichtblick-pilot-layout.json /tmp/mcap-lichtblick-layout.json
docker run --rm -d --name mcap-lichtblick-compare -p 127.0.0.1:8095:8080 \
  -v /tmp/mcap-lichtblick-layout.json:/lichtblick/default-layout.json:ro \
  ghcr.io/lichtblick-suite/lichtblick@sha256:230ed53813f11ab5d23c47380f94c6d358438ecf15c3a7e7de9625b07c437611
python3 -m http.server 8094 --bind 127.0.0.1 --directory apps/viewer-web/dist
# 別terminal。CDP 9238を使うため、他の専用Chromeがそのportにないことを確認する。
python3 tools/performance-probe/run_viewer_pilot.py \
  --data /tmp/oss-pilot-data-new/3840x2160-1cam-10hz-none-8MiB-20s.mcap \
  --output /tmp/oss-pilot-results-new
# 終了後
docker stop mcap-lichtblick-compare
```

harness導入前の試行で、mcap_playerの終端でボタン表示が更新されないことと、Lichtblickのpanel mount完了前にはresizeできないことを確認した。
後続の[8camera検証](../2026-10-15-web-camera-stress/README.md)で、時計自体は終端で停止していると確認した。
このpilotの`playing`フィールドはボタン表示から取得しており、時計状態そのものではない。
これらを計測の待機条件で扱い、失敗したpreflightは集計に含めていない。

検証: Pythonの構文チェック、両app各3回の実ブラウザrun、canvas寸法assert、
初期画像の目視確認を実施。使用中CDP portの拒否も確認した。製品Rustコードは変更していない。
