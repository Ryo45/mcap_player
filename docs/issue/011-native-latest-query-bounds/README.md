# Native の直前値検索をキャンセル可能にし、要求全体の作業・索引保持量を制限する

- 種別: 不具合修正＋リソース制御
- Priority: P2。今回の候補では最優先
- 規模: キャンセル伝播は S、予算・復元時キャッシュの制限まで含めると M
- 状態: 対応済み
- 調査日: 2026-10-12
- 調査対象: HEAD `5fbfac2` ＋未コミットの `viewer-mcap` 分離後の作業ツリー

## 課題

`LatestMessageReader::read_at_cancellable` はキャンセル callback を受け取るが、
Message Index の探索には渡していない。索引を多数読む要求が始まると、topic/source の
切り替え後も不要な探索が続く。直前値がない場合は探索後の Chunk 読み取りもないため、
キャンセル callback を再確認せず `Ok(None)` を返す経路になる。

また、要求全体の索引読み取り回数・入力 bytes に上限がなく、復元用の
`latest_before` は探索した索引をすべて一時キャッシュへ保持する。直前値がない、または
チャンク時刻範囲が大きく重なる録画では、探索量と復元中の索引保持量が候補チャンク数に
比例する。これは `viewer-mcap` への移動による新しい回帰ではなく、移動前からある経路の課題。

## 調査したこと

1. [read_at_cancellable](../../../crates/viewer-mcap/src/recording/source/latest.rs)
   の入口では callback を確認するが、`latest_position` の引数に callback がない。
2. [latest_position](../../../crates/viewer-mcap/src/recording/source/index.rs)
   は候補チャンクを走査し、物理読み取りを
   `read_chunk_message_indexes(chunk, || false)` で呼び出している。
   下位の読み取り・チャンネル解析はキャンセルに対応済みで、伝播が抜けている。
3. [latest_before](../../../crates/viewer-mcap/src/recording/source.rs)
   は `retain_restore_indexes = true` を渡す。ローカル `HashMap` に入った索引には
   eviction や要求全体の保持上限がない。
4. [Inspector worker](../../../apps/viewer-native/src/inspector_loader.rs#L315)
   は generation 判定を callback に渡し、結果公開時にも generation を確認する。
   **古い結果の公開防止はある**。しかし実行中の索引探索を止める部分が欠けている。
   同ファイルの `Drop` はキャンセルして worker を join するため、探索が長ければ終了も待つ。
5. Inspector の8 topic・16 MiB payload 上限は、索引 I/O や復元中の索引キャッシュの
   合計量を制限しない。物理レコード単体の256 MiB上限とも別の問題。
6. Browser の [RestoreWork](../../../apps/viewer-web/src/local/loader.rs#L520)
   は索引 reads/bytes、Chunk bytes、選択メッセージ数を要求単位で制限する。
   Native にその仕組みを丸ごと移す必要はないが、予算を置く比較対象になる。

### 実行した再現確認

リポジトリのコードを変更せず、外部の小さな Rust harness から現行 API を呼んだ。
MCAP Writer で各チャンクに `/a` の未来時刻 `100 + chunk_id` と `/b` の log time `0`
を記録し、`/a` を target `0` で調べる。各チャンクの全体開始時刻は0だが、`/a` には
直前値がない、有効な非単調時刻の fixture である。

callback は「最初の確認は false、2回目以降は true」を返すようにした。
`reader.diagnostics().index_reads` と callback 呼び出し回数を確認した。

| チャンク数 | callback 確認回数 | 索引読み取り回数 | 戻り値 |
| --- | --- | --- | --- |
| 3 | 1 | 3 | `Ok(None)` |
| 64 | 1 | 64 | `Ok(None)` |
| 256 | 1 | 256 | `Ok(None)` |

callback が探索中に呼ばれず、直前値不在の探索量がチャンク数に比例することを確認した。
NAS の遅延やピーク heap は計測していない。復元用キャッシュの増加はコード経路からの確認であり、
Inspector の一索引 span キャッシュと混同しない。

## 解決方針

1. **まずキャンセル伝播を修正する。** `latest_position` に callback を通し、候補境界、
   物理 read 前後、チャンネル解析、結果返却前に確認する。直前値がない経路も確認対象にする。
   非キャンセル API は明示的な非キャンセル callback を渡す。
2. **要求単位の作業予算を追加する。** Inspector の複数 topic/channel を通して reads/bytes
   を消費し、失敗した read も I/O 前に予約する。reader の生涯累積 diagnostics を予算に流用しない。
   長時間使っただけで上限に達する設計にしない。
3. **復元時の索引保持を制限する。** 小さな物理キャッシュへ制限する案と、選択 stream を
   チャンク単位で更新して索引を直ちに解放する案を比較する。前者は変更量が小さいが再読込が
   増え得る。後者は shared chunk の再読込を減らせるが、走査の変更範囲が広い。
   `MessageIndexPredecessor` の exact/tie semantics は維持する。
4. 上限超過は型付きのエラーにする。`None` や途中までの探索結果を exact な成功として返さない。
   seek failure では元の cursor/source/feature state を保持する。

具体的な上限値は要決定。Browser の4,096 reads・32 MiBを参考にできるが、Native は複数 channel の
連続 span を読み、Browser は header/body を分けるため、単純に同じ reads 数を適用しない。
保持量は payload 長だけでなく索引 Vec の capacity や共有 backing も点検する。
実行中の OS ファイル read 自体の強制中断は対象外で、完了した read の後に次を発行しないことを保証する。

## 完了条件・回帰確認

- 最初の物理索引 read 後にキャンセルする fixture で、次の read が発行されず `QueryCancelled` になる。
- 直前値不在、キャッシュ命中、重なるチャンク、同時刻の chunk/offset tie を検証する。
- reads/bytes 上限を小さくした fixture で、上限を超える read が発行されない。
- 新しい demand は新しい予算を使い、正常な cursor 更新を繰り返しても生涯予算を消費しない。
- チャンク数を増やしても、復元中に保持する parsed index span/entry bytes が設定上限を超えない。
- 複数 stream が共有する必要な payload chunk の重複展開を防ぎ、既存の seek rollback を保つ。
- Source generation 変更・終了時に不要な索引探索が続かないことを worker テストで確認する。

## 次の一手

キャンセル伝播と最初の read 後キャンセルの回帰テストを独立した小さな変更として着手する。
続いて、予算の単位・既定値・復元用キャッシュ方針を決める。共通の再生状態管理や GUI の再編は不要。

## 実装結果

`LatestQueryBudget` を要求ごとに作り、Native Inspector の全 topic/channel で共有する。
既定値は64 stream、16,384候補、4,096索引 reads、32 MiB索引入力、256 MiB圧縮＋展開 chunk bytes。
予算は I/O 前に予約し、失敗した読み取りも消費する。上限超過は `McapSourceError::QueryLimit`。
`latest_before_with_limits` で復元側も同じ予算を指定できる。

キャンセルを索引探索と解析まで伝播し、不在・キャッシュ命中を含め結果返却前にも確認する。
復元と Inspector の parsed index は一 span に限定した。解析済み配列は Box slice にし、
malformed な offset の重複を含め、展開する entry の合計も32 MiBまでに制限する。
この上限は全 heap ではなく索引入力／entry量であり、マップ等の固定・比例 overhead は別途ある。
複数 stream の探索による索引再読み取りは増え得るが、payload chunk の重複展開は従来通り避ける。

回帰テストで、最初の read 後のキャンセル／世代変更、直前値不在、失敗 read の予約、
cache hit、新しい要求予算、chunk展開前の上限、3/128チャンクで同じ索引保持上限を確認した。
`cargo test -p viewer-mcap -p viewer-native --offline` と対象 Clippy が成功。
既存の exact/tie、重複展開防止、seek rollback テストも成功した。
