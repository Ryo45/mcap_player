# リリース前アーキテクチャ課題

追加対応: [015 分割MCAPの時間軸とWeb LZ4](015-split-mcap-and-web-lz4/README.md)。Nativeのディレクトリ入力とBrowser LocalのLZ4を実装。Raw Image/PNGは[016 Camera入力形式](016-camera-image-formats/README.md)で対応。

性能の改善候補: [017 NativeのSeek後の画面反映latency](017-native-seek-presentation/README.md)。段階別計測後、decodeをsurface待ちと重ねる変更で再訪Seekを約20%短縮した。Lichtblickとの差と他GPU・連続scrubの検証は残る。

調査対象HEAD: `f16cd90da2e62c8ef0481d126ef87beca9e26161`

## 次の開発候補（2026-10-12 調査）

以下は HEAD `5fbfac2` ＋未コミットの `viewer-mcap` 分離後の作業ツリーを調べた新しい課題です。
既存001〜010の調査・完了記録は下にそのまま残しています。
各 issue は課題・コード調査・解決方針・回帰確認・着手判断を専用ディレクトリにまとめています。

| 推奨順 | 種別 | Issue | 状態 |
| --- | --- | --- | --- |
| 1 | P2 不具合修正＋リソース制御 | [011 Native 直前値検索のキャンセル・要求予算・索引保持上限](011-native-latest-query-bounds/README.md) | 対応済み |
| 2 | P2 機能追加 | [012 Web Inspector 自動更新](012-web-inspector-auto-refresh/README.md) | 対応済み |
| 3 | P2 リファクタリング | [013 Health／Diagnostics の純粋な集計を分離](013-recording-health-reducers/README.md) | 純粋な集計の分離は対応済み。Web UI/I/Oは別課題 |

011〜013の第一段階は実装済み。各issue末尾に実装結果と検証を記録しました。
013のWeb Health／Diagnostics UIと非同期I/Oは次の独立した機能追加です。
共通の再生状態管理やGUI全面統合は含めていません。

## 判断基準

優先度はコードの見た目ではなく、次の順で決めます。

1. seek後のsemantic correctness
2. recording sizeに比例しないworking set
3. high-bandwidth payloadの不要なI/O/decode/copy
4. Native/Web/Remoteで同じ意味になること
5. physical indexed planning knowledgeを複製しないこと

message-centric architectureと、Exact Range Query・Plot・Previewの独立経路は維持します。
generic registry、event bus、Source trait、QueryManager、compatibility facadeは導入しません。

## Architecture priority

| Priority | Issue | Release判定 | 状態 |
| --- | --- | --- | --- |
| P0 | [001 Transactional FeatureRuntime](001-unify-feature-runtime.md) | seek correctness | 完了 |
| P0 | [004 Source capabilityとshared indexed planning](004-unify-source-capability-policy.md) | hidden seek failure / semantic差 | 完了 |
| P0 | [008 Native payload backing共有](008-native-payload-sharing.md) | high-bandwidth copy | 完了 |
| P1 | [005 bounded Plot query](005-separate-query-io-and-reduction.md) | unbounded RAM | 完了 |
| P1 | [003 Scene presentation ownership](003-own-scene-presentation-state.md) | state/reset correctness | 完了 |
| P2 | [002 concrete Panel input](002-narrow-panel-inputs.md) | UI boundary | 完了 |
| P2 | [009 Inspector background query](009-inspector-background-query.md) | open latency | 完了 |
| P2 | [006 Remote catalog contract](006-minimize-pre-release-contracts.md) | release前wire整理 | 完了 |

## Release Gate / policy

| 種別 | Issue | 状態 |
| --- | --- | --- |
| Release Gate: MUST / architecture low | [007 診断surfaceをdefault Web bundleから外す](007-remove-diagnostic-spike-from-release.md) | Gate/P3監査完了 |
| Optional policy decision | [010 Layoutをinternal presetとして明示する](010-layout-internal-contract.md) | 方針決定・完了 |

## 実装順

1. 001でcontinuous feature stateのcandidate restoreとcommitを分離する。
2. 003でSceneのCPU visible stateをpresentationへ移し、rendererをGPU同期だけにする。
3. 004と008を同じsource work packageで進め、index capabilityをopen時に確定し、Chunk backingを共有する。
4. 005でPlot overviewをfixed-budget streaming reductionへ変える。
5. 007の診断UI/moduleをdefault productから削除する。
6. P2でPanel input、Inspector background query、Remote recording-fact contractを整理する。
7. release hygieneで旧owner、unused option/API、one-off targetを同じwork package内から削除する。

## Restore transaction boundary

目標は次です。

    physical restore gather
      -> candidate source position
      -> candidate FeatureRuntimeへstrict route/decode/apply
      -> source + FeatureRuntime + cursor commit

decode/unrouted/application failureではold cursorと全visible feature stateを維持します。latest predecessorが
malformedでprevious-validをboundedに発見できない場合は、empty stateをcommitせずseek failureとします。
recording prefix scanは行いません。

実装済みの境界は次です。

    indexed restore gather
      -> candidate source/data-plane position
      -> candidate FeatureRuntimeへstrict route/decode/apply
      -> infallible source + FeatureRuntime + cursor commit

SceneのTF変換・accumulationは別の`ScenePresentationState`が所有し、seek成功後の一回のpresentation
transitionでresetします。restore failure時はtransitionしないためCPU visible historyも維持されます。

## 実装後のresource contract

- Native RawMessage: positional readまたは一回だけ展開したChunk backingの`Bytes::slice`。message単位payload copyは0。
- Plot overview: 各signal `O(max_display_points)`、exact full history常駐なし。currentはOdometry controller由来。
- Scene visible CPU history: 65,536点上限。rendererはGPU resource/revision同期のみ。
- recording source: Summary + Chunk Index必須。selected non-empty streamはMessage Index必須。
- platform planning: `IndexedChunkFact`からlatest/history/persistent candidate Chunkを共通決定し、I/Oだけを
  positional read/File.slice/Recording Server IndexedReaderへ分ける。

## 維持する境界

- `RawMessage(Bytes)`をcanonical serialized inputとする。
- Camera decode前coalescing、route後のPointCloud decodeを維持する。
- exact playbackとExact Queryを統合しない。
- Plot/Previewをcontinuous FeatureRuntimeへ入れない。
- Web RecordingDataPlaneのbounded windowとgeneration cancellationを維持する。
- WorkspaceBindings、fixed stream selection、Message Index based latest-beforeを維持する。
- dynamic TF historyとstatic TF persistent bootstrapを維持する。

## 検証

- `cargo test --workspace`: 成功（251 passed、3 ignored、0 failed）
- `cargo check --workspace`: 成功
- `cargo clippy --workspace --all-targets -- -D warnings`: 成功
- `cargo fmt --all -- --check`: 成功
- viewer-web / viewer-remote-protocol wasm check: 成功
- `trunk build`: 成功
- `viewer-native --features ros2-live`: 成功

## 011〜013 実装後の検証（2026-10-12）

- `cargo fmt --all -- --check`、workspace Clippy（all-targets、warnings禁止）、workspace tests 成功。
- Dockerでcore/mcap/renderersのWASM check、Web WASM Clippy、Trunk release 成功。
- ROS 2 Jazzy環境で `viewer-native --features ros2-live` テスト成功（78 passed、既存2 ignored）。
- ChromeでLocal/Remote InspectorとScene/BEVの通し確認成功。
- `cargo tree -p viewer-core --edges all` でMCAP/codecs/HTTPの逆依存なし。

## 014: camera presentation metadata

[表示済みcameraのmetadata分離](014-camera-presentation-metadata/README.md)に対応済み。Native/WebともJPEGコピーをデコード完了時に解放し、大きなJPEGでencoded budget到達後も次frameを受け入れられるようにした。
