# 診断・dead surface を製品 bundle から外す

- Release Gate: MUST
- Architecture priority: low
- 規模: S-M
- 分類: 不要機能、未使用 option、release hygiene
- 状態: Release Gate / P3監査完了

## 背景

技術検証用に作られた Range Read Spike と Remote batch smoke は、現在の production local/remote
playback 実装ができた後も default Web bundle と画面に残っています。ほかにも dead-code suppression、
単一 variant option、利用者が一つだけの shared crate が残っています。

これらを個別 issue に分けず、release surface を最小にする一回の監査として扱います。

## Release Gate: Web の診断 UI

### 実装前の根拠

- `range_spike` / `range_spike_browser`が通常WASM buildとstart pathに入っていた。
- product HTMLに`Fetch Sample Batch`と`Range Read Spike`が表示されていた。
- `remote/smoke.rs`がproduction playback起動とdiagnostic batch fetchを同じstateで扱っていた。

### 問題

診断コードが bundle size、DOM contract、web-sys feature、保守対象、ユーザー操作面を増やしています。
production BrowserMcapWindowLoader と重複する range parser/controller probe があり、どちらが正しい
実装か追いにくくなっています。診断失敗が製品品質の失敗に見えることもあります。

### 解決案

- `range_spike.rs`、`range_spike_browser.rs`、HTML の Range Read Spike を default product から削除する。
- production Remote UI は server 接続、recording 選択、open playback に絞る。
- Fetch Sample Batch と format dump は削除、または明示的な non-default `diagnostics` feature/
  example page へ移す。
- spike でしか検証していない range/index invariant は production local loader の unit/integration test へ移す。
- historical spike 文書は historical として残してよい。

## P3: 同じ監査で整理する小項目

次は architecture issue ではありませんが、不要な拡張点を見分けにくくするため同時に確認します。

| 対象 | 判断と結果 |
| --- | --- |
| Camera `ImageFit` | Contain一variantのfield/enumとpreset値を削除 |
| Bookmark save | editing UI/commandがないため未到達save APIを削除。read/displayは維持 |
| ROS live module | 広域`allow(dead_code)`を削除し、module自体を`cfg(any(test, feature = "ros2-live"))`化 |
| viewer-ui crate | Native app内へ戻し、workspace member/dependencyを削除 |
| LoadedWindow diagnostics | source/decompression/copy metricsへ実際に集約されるため維持 |
| mcap codec feature | viewer-coreでzstd/lz4を明示し、NativeのChunk reader testで両codecを検証 |
| one-off uncompress bin | 計測履歴だけ文書へ残し、workspace sourceから削除 |
| Web host build suppression | module全体の`allow(dead_code, unused_imports)`を削除し、wasm/test itemを`cfg`で分離 |

## 維持する挙動

- Browser Local/Remote の production playback、seek、buffering、diagnostics。
- Remote server 接続、recording 一覧、catalog 取得、playback open。
- production loader の Summary/Chunk range validation と generation cancellation。
- bookmarks.json の read/display。save を削除しても読み取り契約は残す。
- `ros2-live` feature 有効時の mailbox/QoS/diagnostics。

## Characterization test 境界

- Browser fixture open -> first complete window -> Camera/Path/Odometry/TF state。
- Remote catalog -> WebPlayback open -> paged batch -> same feature state。
- ROS mailbox push/take/coalesced count。
- Bookmark sidecar load -> displayed marker list。

診断 UI の DOM element や report text は preserved behavior に含めません。

## 完了条件

- default Web HTML/WASM に `range-spike`、batch smoke button、diagnostic installer がない。
- production local/remote loader test が spike 由来の必要 invariant をカバーする。
- product codeの `allow(dead_code)` は feature/test 境界に置換される。
- 一 variant option と未到達 API は「今回実装する」か「削除する」のどちらかになる。
- default workspace member と release binary に one-off spike tool が紛れない。

## 実装結果

- default Web module/installer/HTMLから`range_spike.rs`、`range_spike_browser.rs`、Range Read Spike UIを削除した。
- Remote UIからFetch Sample Batch smokeと関連decode/report codeを削除し、接続・catalog・playback openだけを残した。
- production remote UI moduleを`source_control`へ改名し、diagnostic由来の`smoke` owner名を削除した。
- shared chunk backing、range validation、generation cancellation、local/remote feature parityはproduction
  loader/playback testで継続検証する。
- 表のP3項目も全て「実利用を明示して維持」または「旧surfaceごと削除」まで完了した。
