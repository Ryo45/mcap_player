# リリース前アーキテクチャ課題

調査対象HEAD: `f16cd90da2e62c8ef0481d126ef87beca9e26161`

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

- Native RawMessage: mmapまたは一回だけ展開したChunk backingの`Bytes::slice`。message単位payload copyは0。
- Plot overview: 各signal `O(max_display_points)`、exact full history常駐なし。currentはOdometry controller由来。
- Scene visible CPU history: 65,536点上限。rendererはGPU resource/revision同期のみ。
- recording source: Summary + Chunk Index必須。selected non-empty streamはMessage Index必須。
- platform planning: `IndexedChunkFact`からlatest/history/persistent candidate Chunkを共通決定し、I/Oだけを
  mmap/File.slice/Recording Server IndexedReaderへ分ける。

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
