# Source capability と MCAP indexed access 方針を統一する

- 優先度: P0
- 規模: M-L
- 分類: source contract、重複、fallback、エラー処理
- 状態: 完了

## 背景

Native mmap、Browser File.slice、Recording Server は物理 I/O が異なるため adapter を分ける必要が
あります。一方、選択 stream、時間範囲、Chunk Index、Message Index から「何を読めば forward
playback/seek/restore を満たせるか」を決める規則は同じです。

現在は source capability の表現と indexed planning が各経路に分散しています。

## 根拠

### required が実質 optional

[PlaybackRequirements](../../crates/viewer-core/src/session_plan.rs#L19) は `require_path`、
`require_odometry` などの API を持ちます。しかし
[SessionPlan::build](../../crates/viewer-core/src/session_plan.rs#L154) は topic/schema がなければ
error ではなく `None` を格納します。Camera も priority topic 以外の固定要求は欠落を個別に
報告しません。

したがって、layout が要求した stream の typo、schema mismatch、「なくても表示できる optional
overlay」が同じ silent absence になります。

### indexed planning の重複

- Native は [McapSource::latest_before/indexed_range](../../crates/viewer-core/src/mcap_source.rs#L328)
  で Message Index を選び、candidate Chunk を group します。
- Browser Local は `local/loader.rs` の restore loader で同じ latest/history/persistent の
  candidate 選択と Chunk group を再実装しています。
- Recording Server は [restore_service](../../apps/recording-server/src/restore_service.rs#L77) で
  同じ semantics を三つの request list として再実装しています。

各実装にテストはありますが、一つの conformance scenario を三経路で共有していません。

### capability/fallback の不一致

- Native McapSource は
  [64 MiB 以下なら Summary/Chunk Index なしを linear scan](../../crates/viewer-core/src/mcap_source.rs#L10)
  します。ただし indexed restore は利用できず、open 後の seek で失敗し得ます。
- Browser Local は [Summary がなければ open を拒否](../../apps/viewer-web/src/local/loader.rs#L837)します。
- Recording Server は [Chunk Index なしを拒否](../../apps/recording-server/src/recording.rs#L47)します。
- compression support は application の direct `mcap` dependency feature が viewer-core の
  `mcap` へ Cargo feature unification されることに依存しています。

## 課題

「開ける」「forward playback できる」「seek できる」「persistent restore できる」が一つの
暗黙 bool に潰れています。Native で開けたファイルが Web/Remote では開けない、Native でも
timeline 操作時だけ失敗する、という product 差が README や型に現れません。

indexed selection の重複は inclusive/exclusive 境界、同時刻 ordering、empty stream、
Message Index 欠落時の扱いを platform ごとにずらす危険があります。

## 解決案

### 1. capability matrix を先に決める

少なくとも次を source open 結果へ明示します。

| Capability | 必要な MCAP 構造 |
| --- | --- |
| Catalog | Summary/Statistics または明示的 fallback scan |
| Forward playback | 読み出せる message/chunk |
| Exact seek | 対象 stream の Message Index |
| History restore | 対象 stream と期間を覆う Message Index/Chunk |
| Persistent restore | 対象 stream 全体を列挙できる index |

このViewerではexact seekをrecording playbackの必須機能とします。Nativeのlinear fallbackを削除し、
selected streamのMessage Index不足はsession open中に失敗させます。`open succeeded -> seekで初めて失敗`
という状態を残しません。

### 2. required/optional を型で区別する

`PlaybackRequirements` を少なくとも次に分けます。

- Required: 固定 Camera panel の topic など、欠落時に layout/session を成立させない入力。
- Optional: Camera overlayのPath/TF、SceneのPath/Odometryなど、欠落してもdegraded表示できる入力。

SessionPlan は required 欠落を field 名、topic、期待 schema と共に error にし、optional 欠落だけを
明示的なdegraded `None`として扱います。現在の `require_*` という名前のまま silent optionalにはしません。

### 3. I/O ではなく planning を共有する

巨大な async Source trait は作りません。MCAP Summary から次を計算する pure な planning module を
共有します。

- forward window が必要とする Chunk list
- latest-before の Message Index candidate
- history/persistent が必要とする Chunk group
- stable ordering key と index 欠落理由

Native、Browser、Server は plan が示す byte range/Chunk を各 I/O API で実行し、共通 selector へ
parsed metadata/message を返します。

### 4. codec feature を明示する

viewer-core が `mcap-lz4` / `mcap-zstd` のような feature を公開し、Native が viewer-core dependency
上で選択します。direct dependency を単なる feature carrier として使わない構成にします。

## 段階的な進め方

1. 現在対応する fixture を Summary/ChunkIndex/MessageIndex/compression の matrix にする。
2. required/optional の product 判断を panel ごとに記録し、SessionPlan test を追加する。
3. latest/history/persistent の共通 conformance fixture と期待 message 列を作る。
4. pure candidate planning を Native から抽出し、Browser Local、Server の順に置き換える。
5. linear fallback を削除、または capability-aware UI にする。
6. Cargo codec feature を明示する。

## 維持する挙動

- log time の `[start, endExclusive)` semantics。
- latest-before は target と同時刻を含む。
- dynamic TF history は target を含む 1 秒間。
- static TF persistent archive は一 session 一回だけ bootstrap する。
- shared candidate Chunk は restore 一回につき一度だけ展開する。
- 同時刻は StreamId、同一 stream 内は source order を維持する。
- source read 失敗時は cursor と visible controller state を commit しない。

## Characterization test 境界

    Catalog/index facts + selected streams + target/range
      -> physical read plan

    physical read plan + parsed indexed messages
      -> ordered RawMessage list または capability error

同じ fixture/期待値を Native、Browser Local、Recording Server adapter test から利用します。

## 完了条件

- required 欠落と optional 欠落が異なる結果になる。
- open 成功後に初めて seek 非対応が判明する hidden fallback がない。
- latest/history/persistent の candidate planning が一実装になる。
- platform 差は byte-range/file/HTTP execution に限定される。
- compression feature の有効化元を Cargo.toml から直接追える。

## 実装結果

- `SourceCapabilities`でCatalog/ForwardPlayback/ExactSeek/HistoryRestore/PersistentRestoreを明示した。
  recordingはindexed capability、liveはCatalog/ForwardPlaybackだけを宣言する。
- NativeのSummary-less/unchunked linear fallbackを削除した。selected non-empty streamのMessage Indexは
  `select_streams`中、Browser Localはcatalog adaptation中、Recording Serverはrecording open中に検証する。
- `PlaybackRequirements`をRequired/Optional/Disabledとして扱い、`require_*`欠落をSessionPlan errorにした。
  Camera overlayはoptional、fixed Camera/BEV Path/Plot Odometry/Scene PointCloudとTransformはrequiredとした。
- `IndexedChunkFact`とlatest/history/persistent candidate関数をviewer-coreへ追加し、Native、Browser Local、
  Recording Serverのindex有無・Chunk候補policyを共有した。byte-range/File.slice/IndexedReader実行は各adapterに残す。
- viewer-coreの`mcap-zstd`/`mcap-lz4` featureを明示し、codec supportをCargo dependencyから追跡可能にした。
