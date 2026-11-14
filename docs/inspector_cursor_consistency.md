# Inspectorと確定再生時刻の整合性

Inspectorは引き続き連続再生から独立した、MCAP log time基準のLatestBefore queryである。再生中の非同期coalescing、Nativeのworker、WebのLocal/Remote loaderと世代管理を維持する。

## 表示契約

- 再生中はbest effortで追従する。queryの対象時刻が現在のcursorと違う結果は、共通UIで **Stale snapshot** と対象時刻を明示する。messageのlog timeがcursor以前というだけで、現在時刻に対する正確な結果とは扱わない。
- 再生→停止（自然なEOFを含む）と成功したSeekで、それ以前の要求を世代で失効させ、確定cursorに対する新しい取得を要求する。Webの自動追従OFFでも行う。
- この要求は完了またはエラーになるまで対象時刻を固定する。途中で再生を再開しても、通常の追従要求で上書きしない。さらに新しい停止・Seek、Topic/Source変更は古い要求を失効させる。
- 停止・Seek時には古い値・metadata・エラーをクリアし、取得待ち（admission待ちを含む）を **Loading LatestBefore** として表示する。成功時は最新値または正確な「該当メッセージなし」、失敗時はエラーを表示する。
- 失敗したSeekは確定cursorを変えず、既存Inspectorの有効な状態も失効させない。
- Inspectorの完了を再生・Seekのcommit条件にしない。I/O完了をUI／再生threadで待たない。

「正確な取得」は、確定時刻で既存のLatestBefore処理を実行するという意味である。Sourceの既存index／message cacheの利用を禁止したり、同じpayloadを必ず再decodeしたりする意味ではない。

## Native

`ViewerSession`が明示的な停止とSeek成功後に`InspectorLoader::settle`を通知する。tickによるEOFはloaderがplaying→pausedを検出する。loaderは既存の結果をLoadingへ置き換え、workerのgenerationを進める。`exact_target`がある間、単一pending mailboxの対象をそこへ固定する。

workerの古い成功・失敗はgenerationで拒否し、確定要求では対象時刻も検証する。結果／エラーを受理したら固定を解除する。source forkのpending handoffは捨てず、新しい要求へ引き継ぐ。これにより、初回queryの進行中に停止・SeekしてもworkerがSourceを失わない。

## Web

`InspectorQueryState::settle`が世代を更新し、`required` targetを設定する。automatic followの設定は通常再生中の要求にだけ作用し、このtargetを消さない。各Inspector controllerは停止を検出し、Web hostは`PlaybackEffect::Seeked`の成功時にinteractive／pinned Inspectorへ通知する。

各loaderのcancel/generationとquery stateのgenerationを併用する。確定取得の通知はinboxの結果を採用するより先に処理する。Webの同時実行数は引き続き2件で、`in_flight`と「待機中を含むLoading」を区別し、admission待ちが実行枠を塞がないようにする。

比較用の旧DOM entryにも同じ停止・Seek契約を適用する。ROS Liveでは従来どおりcursor LatestBeforeを提供しない。

## 回帰確認

- Native：実際の小さなindexed MCAPでSessionの停止・Seek通知と最新messageを検証。停止したfake workerで遅延成功／失敗、同時刻Seek、対象時刻の固定、初回Source handoffを検証する。
- Web：query stateに加え、実際のegui Inspector controllerをhost側fake Remote loaderで実行し、automatic OFF、queued Loading、古い結果、正確な値、取得失敗と再要求を確認する。
- 共通UI：Stale／Loading／Errorを表示し、未完了や失敗を「該当メッセージなし」と表示しないことを確認する。
- 実GPUブラウザ：Recording ServerのInspector応答を保持しても再生が進むこと、停止前の成功を遅れて返しても確定しないこと、automatic OFFのSeek、HTTP取得失敗と復帰、Browser LocalへのSource交換を確認する。HTTP bodyをabort前に取得し、切り離した遅延応答を返すことで、実際に世代の拒否を通す。

```sh
cargo test -p viewer-native -p viewer-ui
cargo test -p viewer-web --features egui-ui
python3 tools/performance-probe/check_inspector_settled.py \
  --dist target/inspector-settled/web \
  --server target/web-parity/recording-server \
  --data target/sample-data/release/turtlebot3-7cam-fhd-10s.mcap \
  --output target/inspector-settled/new-browser-check
```

生ログとスクリーンショットはignoredの`target/inspector-settled/`へ置く。新たな履歴store、subscription broker、同期I/O、無制限retryは導入していない。

検証結果：Workspace Clippyと全551テスト（16 ignored）、egui controllerを含むWeb構成82 passed/2 ignored、Native拡張構成94 passed/8 ignored、ROS 2 Live 90 passed/5 ignored。egui/DOMのstrict wasm32 Clippy、共有crate wasm32 check、両Web entryのrelease build、上記実ブラウザテストが成功した。
