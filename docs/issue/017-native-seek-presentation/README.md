# Native: 再訪Seekの画面反映に残るlatency

状態: 調査候補。2026-10-22の[往復Seek計測](../../performance/2026-10-22-seek-revisit/README.md)で確認。

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
