# Native UI/presentation の入力と state を機能別に狭める

- 優先度: P2
- 規模: M
- 分類: 依存方向、上位層の知識漏れ、UI state の配置
- 状態: 完了

## 背景

Panel は layout から生成される concrete enum で、Camera、BEV、Plot、Inspector、Scene、Status の
実装が分かれています。実装前は、描画入口がすべて同じ`PanelFrameContext`を受け取っていました。

このIssueはUI boundaryの改善であり、restore correctness、bounded memory、payload copy、indexed
planningより後に行います。UI cleanupのためにlower layerを動かしません。

## 根拠

実装前の`PanelFrameContext`は次を同時に公開していました。

- playback と interaction
- ViewerPresentation 全体と camera overlay
- signal query、preview、bookmark
- Camera/BEV/Scene の GPU resource
- Scene diagnostics
- inspection result

実際の利用は機能ごとに限定されています。Camera は camera presentation/texture/overlay、
BEV は BEV texture と path count、Plot は playback/signal/preview/bookmark、Scene は Scene resource、
Inspector は inspection だけを使います。

## 課題

新しい field を PanelFrameContext へ足すと全 panel がその型へ依存します。各 panel から本来不要な
source/query/GPU resource へアクセスでき、panel interface が依存ルールを強制していません。
既存設計文書が意図する「panel は narrow read-only input を受ける」という境界とも一致しません。

テストでも全 field を埋めた大きな fixture が必要になり、個別 panel の挙動より composition の
内部構造を固定しています。

同じ方向の漏れが Plot と viewer-ui にもありました。

- viewer-core の`PlotPanelState`が Overview/Follow、
  viewport、selected signal という Native panel interaction を所有していました。
- workspace はさらに egui_plot の PlotPoint cache を所有し、Plot の presentation state が
  core、workspace、panel に分かれていました。
- `viewer-ui` crate は「shared」とされていましたが、利用者は Native の
  `graphics/ui.rs` だけで、Web は DOM UI でした。

platform-neutral core に置くべきなのは SignalId、sample、時刻変換、downsample のような
deterministic model であり、panel mode や egui widget composition ではありません。

## 解決案

layout traversal だけが frame 全体を受け取り、NativePanel の enum dispatch で機能別 input を
組み立てます。各 concrete panel の `show` は次のような専用型だけを受け取ります。

- CameraPanelInput: camera presentation、exact/preview texture、overlay、preview active
- BevPanelInput: texture、path diagnostics
- PlotPanelInput: PlaybackView、SignalDataView、preview overview、bookmark、display time
- ScenePanelInput: texture、SceneDiagnostics、camera state、transform count
- InspectorPanelInput: 対象 TopicInspection
- StatusPanelInput: session status と必要な telemetry

共通 service locator や `dyn Any` は導入しません。root の frame bundle が大きいことは許容しても、
それを concrete panel module へ直接公開しないことが重要です。

## 段階的な進め方

1. 各 panel が現在読む field を characterization test で固定する。
2. Camera と BEV から専用 input へ切り替える。
3. Plot、Scene、Inspector、Status を順に切り替える。
4. PanelFrameContext を layout host 内部の composition 用型へ縮退、または削除する。
5. PlotPanelState、viewport/follow command、egui_plot cache を Native Plot panel runtime 配下へ集約する。
6. PanelResourceView、PreviewDataView、SceneDataView の不要な横断 bundle を整理する。
7. viewer-ui が release 時点でも Native 専用なら Native app へ戻す。共有予定が確定している場合だけ
   crate を残し、共有する widget 境界を文書化する。

## 維持する挙動

- layout の split、weight、panel ID による state の分離。
- panel action は App へ返し、panel から session/source を直接変更しない。
- BEV/Scene の logical size request は Graphics が GPU target へ反映する。
- preview 中の Camera/Plot と exact playback state は混ざらない。

## Characterization test 境界

    専用 PanelInput + panel local state + egui test UI
      -> ViewerAction + RenderRequest + panel local state

session、controller、Graphics 全体を fixture に含めないテストにします。

## 完了条件

- concrete panel の public-to-module な `show` 引数から PanelFrameContext が消える。
- Camera panel から signal/inspection/Scene resource へ参照できない。
- Plot panel から Camera overlay/GPU Scene resource へ参照できない。
- layout host の責務は rectangle 計算、dispatch、output aggregation に限定される。
- viewer-core は egui/Native panel interaction の state を所有しない。
- Plot の authoritative UI state と display cache が一つの panel runtime にまとまる。

## 実装結果

- layout traversal用の`PanelCompositionInput`はenum dispatchだけが参照し、各concrete `show`は
  `CameraPanelInput`、`BevPanelInput`、`PlotPanelInput`、`InspectorPanelInput`、
  `ScenePanelInput`、`StatusPanelInput`のいずれかだけを受け取る。
- Plot mode/follow/viewportと`egui_plot::PlotPoint` cacheをNative `PlotPanel` runtimeへ集約し、
  viewer-coreからNative interaction stateとunused `selected_signals`を削除した。
- Nativeだけが利用していた`viewer-ui` crateを廃止し、playback/source UIをNative app内へ戻した。
- layout hostのcharacterization test、Camera selection、Plot follow、Status current-value testで
  input narrowing前後のobservable behaviorを維持した。
