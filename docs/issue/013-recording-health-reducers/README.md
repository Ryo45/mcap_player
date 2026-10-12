# Health／Diagnostics の純粋な集計を MCAP の読み取りから分離する

- 種別: リファクタリング＋Web 機能追加の準備
- Priority: P2。Web Health／Diagnostics に着手する時点で実施
- 規模: 純粋な集計の分離は S-M、Local/Remote の機能追加は別の M-L 作業
- 状態: 第一段階対応済み（Web UI/I/O は別課題）
- 調査日: 2026-10-12
- 調査対象: HEAD `5fbfac2` ＋未コミットの `viewer-mcap` 分離後の作業ツリー

## 課題

Web には Topic Health と Diagnostics がまだない。MCAP crate の分離は設計変更を避けて
source-bound query module をそのまま移したため、純粋な集計と同期 I/O が一緒に残っている。
このまま Web に対応すると、計算だけ再実装するか、Web から同期 `McapSource` を無理に扱う必要が出る。

これは現在の結果が間違っているという指摘ではなく、実際に予定している Web 対応に向けた
責務分離の課題。ファイル形式に依存しない計算を core へ置く、という開発方針に合う。

## 調査したこと

| 処理 | 現在の場所 | MCAP 固有か |
| --- | --- | --- |
| Message Index の読み取り・chunk frontier・exact predecessor | `viewer-mcap/query/timestamp_window.rs` | 固有。MCAP 側に維持 |
| topic ごとの rate/age/recent gap/max gap | `topic_health.rs::summarize` | 時刻列と cursor の計算だけ |
| Diagnostics の CDR decode | `viewer-core` | 既に source-neutral |
| `(level, name, message)` の集計と最新詳細の選択 | `diagnostics.rs::DiagnosticAggregator` | `RawMessage` の内容と時刻だけ |
| candidate Chunk の visits/圧縮・展開 bytes 制限 | `query_diagnostics` | MCAP I/O 側の責任 |
| selected payload/messages、result strings/groups の制限 | `DiagnosticAggregator` | 集計への入力・保持量として共通化可能 |

- [summarize](../../../crates/viewer-mcap/src/query/topic_health.rs#L135) は I/O をしないが、
  MCAP query error を含む `TopicHealthSnapshot` を直接更新している。
- [DiagnosticAggregator](../../../crates/viewer-mcap/src/query/diagnostics.rs#L122) も I/O をしない。
  ただし、そのエラー型 `WindowQueryError` は `Source(McapOpenError)` を含み、型ごと core へ
  移すと禁止している `viewer-core -> viewer-mcap` 依存が生じる。
- [query_diagnostics](../../../crates/viewer-mcap/src/query/diagnostics.rs#L46) は `&McapSource` を受け、
  catalog validation、candidate Chunk 予算、range visit、集計を同時に実行している。
- Native の [WindowQueryLoader](../../../apps/viewer-native/src/window_query_loader.rs) は
  この同期関数を専用 worker で呼ぶ。Browser/Remote は別の非同期 I/O を使う必要がある。
- `viewer-core` の [SignalOverviewReducer](../../../crates/viewer-core/src/derived/plot.rs#L66) は
  入力 sample と予算だけを受け、MCAP reader から呼ばれる既存の具体例になっている。

## 解決方針

### 第一段階: Native の結果を維持したまま純粋な計算を分ける

1. core に具体的な Diagnostics reducer と、その集計結果・集計エラーを置く。
   入力は `RawMessage`、または既存 decoder が作る typed array＋log time にする。
   malformed CDR の skip count・最初の decode error を保持する現行仕様を維持する。
2. reducer error には入力・保持量の limit などを置き、MCAP/HTTP/ファイル error を持ち込まない。
   `viewer-mcap` の wrapper が source error と reducer error を区別して返す。
3. Topic Health は `summarize` の計算結果を source error を含まない timing facts として切り出す。
   正確な latest-before は明示的な入力にする。window 内 sample が空でも、その前の predecessor
   に基づく age が必要なので、window の末尾だけから latest を推測しない。
4. Native query wrapper は既存の indexed traversal と I/O 予算を維持し、共有 reducer を呼ぶ。
   第一段階では GUI・worker・HTTP endpoint を変更しない。

### 第二段階: 一機能ずつ Web へ接続する

Diagnostics は選択 stream の bounded range を Local/Remote で取得し、同じ reducer に投入する。
Remote の continuation は最後まで揃ってから結果を公開する。
Topic Health は payload を読む方法で代用せず、timestamp-only の indexed access を用意する。
Local の非同期 Message Index reader と、Remote の metadata query endpoint の契約は別途設計する。

Remote で server が集計する案も比較対象になるが、core の reducer を再利用できる形にする。
この issue の第一段階で新しい wire protocol や endpoint を決め切る必要はない。

## 維持する結果と検証

- window は `[cursor - lookback, cursor + 1 ns)`。時刻は MCAP log time で、measurement time と分ける。
- 同時刻の multiplicity、zero span、window 外の predecessor、nanosecond 精度を保つ。
- Diagnostics の group key、最古／最新時刻、最新詳細、同時刻の stream/source 順、結果のソートを保つ。
  reducer は入力順への要求を明示する。異なる backend の arrival ordering を暗黙に同じとしない。
- producer の level をそのまま示し、Viewer が timing から OK/WARN 等の分類を作らない。
- 入力10,000 messages／16 MiB、4,096 groups、4 MiB retained strings の既存上限を維持する。
  candidate Chunk 上限や index reads/bytes は I/O 層に残す。
- core reducer テストは MCAP Writer を使わず、型付き入力・raw CDR・時刻列で成立させる。
  mcap 側の既存 fixture テストで Native の最終 snapshot の同等性を確認する。
- core に MCAP/codec/HTTP dependency が戻らず、UI の snapshot も履歴全体を保持しない。

## 代案と着手判断

Web の集計を別実装する方が最初の変更は小さいが、rate/gap や group/tie/budget の修正が二重になる。
全クエリを一つの manager にする案は、結果・I/O・更新規則の違いを吸収する費用が大きく、今回の
共有対象には不要。具体的な reducer 二つと、各 source の I/O wrapper で十分。

Web 対応が当面予定されないなら緊急性は低い。対応するなら、その実装の最初に第一段階を行う。
共通の再生状態管理や GUI の統合は含めない。

## 実装結果

`viewer-core::DiagnosticAggregator` と `DiagnosticEvent`／`DiagnosticsSnapshot`／
`DiagnosticsReduceError` をcoreへ移した。入力はraw CDRとlog time、同時刻の詳細は
最後に投入した入力が勝つ。MCAP wrapperはstream順＋従来のsource traversal順を維持する。
入力・group・保持文字列の上限は変更しない。admit失敗後はreducerを失敗状態に固定し、
`finish` でもエラーを返すため、部分集計を成功として公開できない。
MCAP wrapperでreducer limitを既存query limitへ変換し、source errorと区別する。
Nativeのsnapshot参照もcore所有型に変更し、旧MCAP側のDTO再exportは残さない。

`TopicTimingFacts`／`summarize_topic_timing` は時刻列と明示的なexact predecessorから
count/rate/age/gapを計算する。空windowの古い直前値、同時刻のmultiplicity、zero span、
nanosecond/full signed rangeをテストした。DiagnosticsはMCAP WriterなしでCDRを投入し、
malformed skip、最初のdecode error、入力順tie、並び順、全reducer予算を検証した。
既存MCAP fixtureテストでもNative query結果の同等性を確認した。

候補chunk・Message Index読み取りとその予算、window request/error、worker、GUIは
MCAP／Native側に残す。第二段階のWeb Health／Diagnostics UIとLocal/Remote I/Oは
この変更には含まない。coreからMCAP/codecsへの逆依存も追加していない。
