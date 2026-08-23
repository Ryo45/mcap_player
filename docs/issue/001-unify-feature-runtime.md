# Continuous feature stateをtransactional FeatureRuntimeへ集約する

- Priority: P0
- 規模: L
- 状態: 完了

## 背景

NativeWorkspaceとWebPlaybackはCamera/Path/Odometry/TF/optional Scene controllerの生成、forward route、
restore reset/replay、counter集約を別々に実装しています。Camera scheduling priorityもWorkspace/WebAppと
CameraControllerに複数copyがあります。

より重大なのは、physical restore readだけがtransactionalで、controller applicationが直接visible stateを
reset・更新する点です。

## 現在の問題

`McapPlayback::seek_with`はrestore readとsource reposition後に戻り値のないcallbackを呼び、
`NativeWorkspace::restore_messages`は全controllerをresetしてmessageをreplayします。Webも同じ順です。
各controllerはrouteに一致すればdecode failureをcounterへ加算するだけで成功扱いにします。

そのため一featureまたはmulti-feature restoreの途中でmalformed payloadが来ると、cursorをcommitしながら
empty/partial stateを表示できます。sequential playbackで残るprevious valid stateとも意味がずれます。

## 解決方針

viewer-coreへconcreteな`FeatureRuntime`を置きます。

    FeatureRuntime {
        CameraController,
        PathController,
        OdometryController,
        TransformController,
        Option<SceneController>,
        counters / processing timing
    }

責務はSessionPlanからの構築、forward reduction、Camera scheduler advance、strict candidate restore、
successful candidate commit、counter集約、global scheduling priorityです。Controller trait、registry、
`Box<dyn Feature>`、Any、generic FeatureUpdateは作りません。

restoreはruntimeをcloneしたcandidateへreset/decode/applyし、全messageがroute・decodeできた場合だけ
authoritative runtimeと置換します。source positionも事前にcandidate化し、runtime commit後に失敗しない
順序にします。

## malformed predecessor semantic

- bounded Message Index lookupが返したlatest predecessorをstrict decodeする。
- malformedならseek全体を失敗させ、old cursor/stateを維持する。
- unbounded prefix scanでprevious-validを探さない。
- 将来bounded previous-candidate探索を追加する場合も同じtransaction境界内で行う。

## 契約テスト

- physical read failureでcallback/runtime/cursorが不変。
- malformed latest predecessorでCamera/Path/Odometry/TF/Sceneが全て不変。
- multi-feature replay途中のfailureで先に適用したfeatureもcommitされない。
- Native/Web相当の同じmessage scenarioが同じsnapshot/counterになる。
- panel-local Camera selectionとglobal scheduling priorityが別stateである。

## 完了条件

- controller dispatch/reset/counter集約がFeatureRuntimeの一実装になる。
- NativeWorkspace/WebPlaybackにcontroller fieldやrestore loopが残らない。
- restore failureはold cursorと全visible feature stateを維持する。
- Camera scheduling priorityのauthoritative copyはCameraController内だけになる。

## 実装結果

- `viewer-core::FeatureRuntime`を追加し、Camera/Path/Odometry/TF/optional Scene、counter、processing
  timing、Camera scheduling priorityを所有させた。
- NativeWorkspaceとWebPlaybackのcontroller field、forward dispatch、restore reset/replay、counter集約を削除した。
- restoreはcloneしたruntime candidateへstrict decode/applyし、全件成功時だけ置換する。
- Nativeのsource positionは`prepare_seek`で候補化し、application成功後にruntime/source/cursorをcommitする。
  Webもrestore archiveとruntimeをcandidate化し、data-plane seekが成立してから同時にcommitする。
- malformed predecessor、各feature failure、multi-feature途中failure、Native/Web scenario parityをtestで固定した。
