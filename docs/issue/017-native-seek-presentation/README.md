# Native: 再訪Seekの画面反映に残るlatency

状態: surface待ちとdecodeの重ね合わせを実装。残る画面反映差は継続調査。2026-10-22の[往復Seek計測](../../performance/2026-10-22-seek-revisit/README.md)で確認。

## 課題

Zstd 8 MiB設定・FHD Raw 1カメラの短距離往復では、Chunk再利用後の物理読取＋feature復元は再訪中央値約0.003msになった。一方、正しい画像が見えるまでの中央値はNative約83ms、Lichtblick Desktop 1.29.1約60msで差が残る。各3プロセス、操作列後半の再訪を使った値。

大Chunkでは約0.4〜1.3秒の待ちを解消できたが、小Chunkで残る差はChunk再展開だけでは説明できない。現在の証拠ではRaw変換・worker・upload・presentのどこが支配的かまでは分からない。

## 調査済み

- 操作完了確認の50ms pollingを避け、Native／Lichtblickとも5msへ揃えて再計測した。
- Nativeの操作完了ログまでは数ms〜数十ms。その後に画素一致までの時間がある。各resultの`control_completed_ms`と`latency_ms`を参照。
- Nativeは`graphics/camera.rs::upload_latest`で完了結果を回収してGPUへ渡し、未反映frameを`CameraDecodeQueue`へsubmitする。
- Rawは表示サイズへ直接RGBA変換する。JPEG専用のIDCT最適化の議論とは分ける。
- 制御ファイルとCDPで操作を注入するため、アプリ間の差に操作方式・観測の違いも含まれる。

## 次にどう調べるか

同一操作ID・frame時刻について、操作受理、復元完了、decode enqueue/start/end、upload完了、最初のpresentを測定専用ログで結びつける。画面の画素確認も維持する。追加ログの負荷を対照で確認する。

支配的な段階が判明してから、その段階だけを改善する。例えば余分なframe待ちなら結果通知・反映順を検討し、変換なら同じ表示品質・領域で処理を比較する。worker増設やdecoded画像cacheの追加を先に決めない。pause中のSeekにも正しく応答し、古いgenerationの画像を表示しないことを回帰検証する。

連続ドラッグではなく、各Seek完了を待つ実験である。重なるSeek／cancelの評価は別の操作列を用意する。

## 2026-10-23の段階別計測

[実収録確認とSeek調査](../../performance/2026-10-23-recorded-seek/README.md)を実施した。
合成FHD Raw・小Chunkの再訪で、各3processの中央値はNativeトレースoff約85.1ms、on約83.8ms、Lichtblick約60.8ms。
同じtargetが連続する1操作は画面が既に一致するため除外した。

トレース有効時の再訪復元は約0.13ms、Raw変換約6.28ms、worker完了後の回収待ち約9.12ms、新画像frameのsurface取得待ち約16.02ms。
操作受理からpresent呼び出し終了は約33.12ms。外部画素確認までの残差約49〜52msは、受理前待ち・FIFO/compositor・観測をまだ分離できていない。
ログ負荷を含む短いCPU区間、present API完了と実画面表示の違いに注意する。

実収録の180×320 rgb8でも、変換約0.15〜0.52msに対し結果回収待ち約15〜16msがあった。
次はsurface取得を待った後に最新結果を回収できる処理順と、FIFO frame latency=1の測定対照を検討する。
表示周期以上の裏描画や先回りしたdecoded cache・worker増設は行わない。

## 2026-10-23の改善

[処理順変更の比較](../../performance/2026-10-23-seek-latency/README.md)で、pixel workをsurface取得前にsubmitし、取得待ち後・UI構築前に完了結果を回収するようにした。
最終版の各3processでは、再訪中央値が83.4msから66.6msへ約20%短縮。同時比較のLichtblickは60.4msで、差は約23msから約6msになった。
操作受理から新画像present呼び出し終了も、前回の約33msから約17msになった。

FIFO latency hintを1にするだけの対照には改善がなく、製品は2を維持した。
worker・queue上限・cacheは増やしていない。取得後の描画から不要なResultを除き、取得失敗の回復経路は維持する。
残りの差は、外部操作の受理待ちやcompositorのpresentation feedbackを含めて調べる余地がある。連続scrub/cancelや他GPUの検証も残る。
