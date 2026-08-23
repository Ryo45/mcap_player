# Scene の presentation state を一か所に集約する

- 優先度: P1
- 規模: M
- 分類: state ownership、domain/presentation/render 分離
- 状態: 完了

## 背景

3D Scene は LaserScan、Path、Odometry、TF を組み合わせ、TF 変換済み点群と描画 frame を生成します。
現在はこの処理の state が controller、frame builder、GPU renderer に分散しています。

## 根拠

- [SceneController](../../crates/viewer-core/src/controllers.rs#L427) は PointCloudState に加えて
  [SceneFrameBuilder](../../crates/viewer-core/src/controllers.rs#L430) を所有します。
- SceneFrameBuilder は [TF revision、変換済み cloud、TF error](../../crates/viewer-core/src/frame_builder.rs#L54)
  を cache し、snapshot 作成時に mutation します。
- [SceneRenderer](../../crates/scene-renderer/src/lib.rs#L81) は GPU resource だけでなく
  `accumulated_points` という CPU-side 履歴を所有します。
- renderer の [update_accumulated_cloud](../../crates/scene-renderer/src/lib.rs#L514) が accumulation policy、
  65,536 点上限、座標変換、mode change 時の clear を決めています。

## 課題

semantic controller が presentation cache を所有し、GPU renderer が履歴 policy を所有しています。
そのため Scene の visible state を GPU なしで完全には観測できず、seek/source change 時には
SceneController の reset と Graphics の clear の両方を正しい順で呼ぶ必要があります。

Graphics は GPU/backend resource の所有者という設計文書上の境界とも一致しません。
将来 Web Scene や offscreen export を追加すると、renderer の内部履歴を別実装で再現する必要があります。

## 解決案

状態を次の三層に分けます。

1. SceneController は StreamId route、LaserScan decode、PointCloudState、counter だけを所有する。
2. ScenePresentationState は TF 変換 cache、missing-TF retry、accumulation mode、bounded point history、
   presentation revision、diagnostics を所有する。
3. SceneRenderer は渡されたcomplete visible point sliceとrevisionをGPU bufferへ同期する。

ScenePresentationState は PresentationState 配下、または GPU 非依存の viewer-core presentation module に
置けます。重要なのは controller と renderer のどちらにも跨がらせないことです。

## 段階的な進め方

1. 現行 SceneFrameBuilder と update_accumulated_cloud を同じ pure scenario から観測するテストを作る。
2. update_accumulated_cloud を GPU 型へ依存しない ScenePresentationState へ移す。
3. SceneFrameBuilder を SceneController から PresentationState 配下へ移す。
4. renderer input を完成済み point slice + revision に変更する。
5. seek/source change を PresentationTransition 一回で reset できるようにする。

## 維持する挙動

- 各 scan は measurement time の TF で一度だけ target frame へ変換される。
- TF が不足した scan は、TF revision が変わったときに再試行される。
- 後続の ego pose 更新で過去点群を再変換しない。
- accumulate off は最新 scan のみ、on は到着順の bounded history。
- 65,536 点を超えた古い点を捨てる。
- seek と source change で履歴と missing-TF diagnostics を消す。
- revision が不変なら GPU point buffer を upload しない。

## Characterization test 境界

    PointCloudState + Path/Telemetry/TransformState
      + accumulation command + presentation transition
      -> ScenePresentationFrame + diagnostics + visible point count

GPU を使わず上記を検証し、renderer test は revision と upload plan だけを確認します。

## 完了条件

- SceneController が SceneFrameBuilder を所有しない。
- SceneRenderer が accumulated_points と accumulation policy を所有しない。
- Scene の CPU visible state と reset を GPU なしでテストできる。
- renderer の state は GPU resource と同期済み revision に限定される。

## 実装結果

- `SceneController`はLaserScan route/decodeとPointCloud semantic stateだけを所有する。
- `ScenePresentationState`へTF変換cache、missing-TF retry、accumulation、65,536点上限、revision、
  diagnosticsを集約し、Native `PresentationState`が所有する。
- `SceneRenderer`から`accumulated_points`とaccumulation mode/policyを削除し、完成済みvisible cloudと
  revisionをGPU bufferへ同期するだけにした。
- GPUなしでlatest-only/accumulation/reset/bounded historyを検証するtestを追加した。
