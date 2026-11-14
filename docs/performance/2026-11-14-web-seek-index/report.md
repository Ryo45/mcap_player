# Browser Seek: Message Indexのまとめ読みと再利用

2026-11-14。Android Chromeで、配布サンプルを端末内から開き、Seek後の秒数変更からカメラ画像表示まで約3秒待つという報告への対策。実機Androidでの原因確定・改善確認は未実施。

## 分かったことと変更

タイムラインは要求先時刻を先に表示する。実際の復元がcommitされるまで古い画像を保持し、その後にテクスチャを破棄・新画像を展開する。このため「長く待つ → Waiting for camera frameが一瞬 → 新画像」という観察は、JPEGより前の読み込み／復元待ちと整合する。

Browser LocalのSeekは、各ChannelのMessage Indexをheader 9 bytesとbodyに分け、順番にFile.slice/arrayBufferで取得していた。同じChunkの隣接Indexを何度も読む構造があり、ファイル読み取りの往復が遅い環境で待ちが積み重なる。

- 1MiB以下のMessage Indexブロックを、Chunk単位で1回の読み取りにまとめた。選択メッセージのpayloadはまとめ読みしない。
- 生の索引bytesのみを、Sourceに属するLRUへ保持する。合計8MiB・最大32ブロック。Source交換で破棄し、同じSourceのInspector forkだけで共有する。キャッシュが満たされても取得範囲は増やさない。
- 1MiBを超える索引ブロックは、従来の必要Channelだけを読む経路を維持する。全索引の先読みや全ファイル走査はしない。
- キャッシュhitでも解析work budgetを消費し、外部offset・record長・Channel・配列長を検証する。キャンセル確認後にのみ取得結果をキャッシュへ入れる。
- RestorePlanner、latest-beforeの決定、TF history/static persistence、transactional commitは変更していない。JPEGデコーダ、GPU描画、連続再生の先読みも変更していない。

## 計測

同じ配布FHDサンプル、Monitor、1920×1080/DPR 1、NVIDIA実GPU Chromeで、.2/.7/.3/.8/.4/.65の6回Seek。初回のcold index cacheを含む。比較対象は旧main `2e6b73a` と今回の実装。コンパイルや他のGPU実験を同時に走らせずに測り直した値を採用した。

| 条件 | 変更前 | 変更後 |
| --- | ---: | ---: |
| 復元までのFile読み取り回数・中央値 | 25 | 6 |
| 通常のtap → focus image GPU import・中央値 | 45.8ms | 43.6ms |
| File読み取り1回へ100msを付加・同中央値 | 2569.8ms | 648.1ms |

遅延条件で約75%短縮。初回は26→7回、TF履歴が多い対象では40→8回だった。遅延は計測時だけBlob.arrayBufferの完了に付加する。製品の読み取りにタイマーは追加していない。旧版が実際のAndroidで100ms/readを要していると実証したものではない。

画像のGPU import時刻を使い、最終的な画面提示・Android実機の表示レイテンシーとは区別する。1プロセスずつ・各6回の短い比較で、OS cacheは許容。全ての巨大ファイル／圧縮方式の性能を保証するものではない。生trace・Chrome環境情報はignoredの`target/web-seek-review/verified-*`へ置き、小さな[集計](summary.json)のみGitへ保存する。

## 検証と再現

外部offset／record長／Channel／配列長の破損入力、キャッシュのbacking共有、byte/count上限、LRU evictionとSource分離を単体テストに追加。既存のseek rollback・境界・latest-before・圧縮形式のテストを維持した。

```sh
# 変更前ビルドを別ディレクトリに保存した上で、新版を標準Trunk targetでビルドする
python3 tools/performance-probe/measure_web_seek_io.py \
  --before target/web-release-check/site --before-base-path /mcap_player/ \
  --after target/web-seek-review/after \
  --data target/sample-data/release/turtlebot3-7cam-fhd-10s.mcap \
  --output target/web-seek-review/new-comparison --read-delay-ms 100
```

通常条件は別のoutputへ`--read-delay-ms 0`で実行する。計測ツールは隔離したChromeで、実際のeguiポインター入力とread-only statusを使用する。可視範囲とfocus/thumbnailを区別するため、このツールは既定のdesktop Monitor/DPR 1に限定している。

Workspace Clippyと全542テスト（16 ignored）、共通crateのwasm32 check、egui wasm32 Clippy、標準Trunk release buildが成功。実GPUブラウザではLocal/Remote、Source交換と古い結果の排除、30秒タイムアウト、不正layout、Seek後の画像、Inspector、物理設定、RGBA/PNG alphaの回帰確認が通過した。実機Androidは未確認。

## 残る制約

圧縮Chunk自体の読み取り・展開は残る。端末側の主因がそれらやGPU／メモリ圧迫なら、同じ短縮率にならない可能性がある。巨大な索引はChannel単位の読み取りを維持する。今回、Chunk cacheの拡張、並列復元、WorkerへのMCAP展開移管、部分的なfeature commitは導入しなかった。まず公開版で同じAndroid・同じファイルを再確認する。
