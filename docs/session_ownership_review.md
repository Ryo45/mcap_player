# Native / Web session ownership review

調査基点: main `fa6b266`、2026-11-11。`feature/query-panel`は今回取り込んでいない。

## 所有関係とデータフロー

| 責務 | Native | Web |
| --- | --- | --- |
| source open / replacement | `App::load`が候補を構築し、成功後にSession・Workspace・Presentationを更新 | Localはasync summary読込とsource generation検査、Remoteはcatalogから構築。成功後に共通`commit_playback`へ渡す |
| Source / clock | `ViewerSession -> McapPlayback -> McapSource + PlaybackClock`。Liveは別の`RosLiveHandle` | `WebPlayback -> PlaybackClock + RecordingDataPlane + restore loader`。Local/RemoteのI/Oはloaderへ分離 |
| Plan | Sessionが所有し、UIの要求変更とrestoreに使用 | open時のPlanをFeatureRuntimeのrouteとrestore inputsへ展開。WebPlaybackはPlan全体を重複保持しない |
| decoded semantic state | `NativeWorkspace -> FeatureRuntime` | `WebPlayback -> FeatureRuntime` |
| forward | `McapPlayback::tick`のread成功後、callbackで`workspace.process_messages`、clock commit | data planeのcomplete horizonを検査し、`runtime.process_messages`後にclock commit。未完了ならBuffering |
| Seek | sourceでlatest/history/persistentとforward positionを準備し、FeatureRuntimeのtransactional restore成功後にsource/clockをcommit | async restoreをpollし、runtime候補とpersistent archiveを準備。data planeの境界・runtime・clockをcommit |
| Camera変更 | 同じstream集合ならpending frameの提示とpriority変更だけ。集合が変わる場合は新PlanでSourceとruntimeを復元 | 現在は全対応cameraを選択済み。playingではpriorityだけ、pausedでは既存非同期Seekを使ってexact predecessorを取得 |
| presentation | `PresentationState` / graphicsがWorkspaceのfeature stateとSessionのclock/queryを参照 | `WebViewState` / DOM / rendererがWebPlaybackのfeature stateとclockを参照 |
| 独立query | Inspector / numeric Plot / odometry / window / optional plugin loadersが独立readerを使用 | TopicBrowserがforkしたrestore loaderと独立したInspectorQueryの世代管理を使用。Nativeと同じPlot UIは存在しない |

両アプリに独立したclose操作はない。成功したsource replacementで旧Sourceを解放し、終了時は所有者のdropで解放する。
open失敗では現在のsessionを置き換えない。Webの古いopen completionはsource generationで破棄する。

## 評価と今回の選択

NativeのWorkspaceがruntimeを持つことは、Source/clockとの整合性をAppが保証する必要があり、隠れた結合でもある。
ただし現状のAppは同期・単一GUIスレッドで、fallibleな処理を候補構築へ寄せてからcommitしている。
一方Webは、複数frameにまたがるpending seek・bufferingを持つため、WebPlaybackがruntimeも所有する方がcommit境界を読みやすい。
構造体が違うことだけを理由に所有者を統一しない。

比較した選択肢:

1. **現状の所有者を維持し、再構成の値の流れを整理する（採用）**。
   読込・restore失敗の保証を保ちつつ、Planの二重構築とAppの出力slotを除去できる。
2. NativeのruntimeをViewerSessionへ移す。
   Sessionとruntimeの同時存在を型で強くできるが、Workspaceのfeature access・Live・extension・presentationまで横断して変える必要がある。
   今回の具体的な重複はその移動なしで解消できるため延期した。
3. 共通Session / generic transport / managerを作る。
   Nativeの同期seek、Live push、Webのcomplete-window・pending-generationを同じinterfaceに押し込む追加状態が必要になる。
   現在の要求では複雑さを上回る利点を確認できず不採用。

## 発見した問題と変更

### Native: 同じPlanを二度解決し、復元候補をAppのOptionへ書き出していた

変更前の`App::apply_actions`はcamera selectionからPlanを構築し、stream集合が変わる場合に
`ViewerSession::select_camera_topics`が同じcatalog / requirements / bindingsで再度構築していた。
さらにAppの`candidate: Option<FeatureRuntime>`をrestore callbackから書き換え、Sessionから戻った後にWorkspaceへ代入していた。

変更後は一度構築したPlanをSessionへ渡す。Sessionはcapability検査・restore plan生成・Source再選択・自身のPlan commitを担当する。
callbackは`FeatureRuntime`候補を返し、SessionはSourceのcommit成功後だけその候補をAppへ返す。
Appの残りの処理はWorkspaceへの代入、panel binding、preview / scheduler / presentationの更新。
Sourceとruntimeの間にasync yieldや追加のfallible commitは入らない。

下位の`McapPlayback::{seek_with,reselect_with}`は、既存callbackの成功値をcommit後に返すようにした。
MCAP層はFeatureRuntime型を知らない。NativeのSession APIは実際の用途に合わせて戻り値をFeatureRuntimeに限定し、汎用Session abstractionは追加していない。
通常tick・通常Seekのroutingやdecode、read-ahead、cache予算は変更していない。

### Web: LocalとRemoteでsource確定時の初期化がずれていた

変更前の`app/source.rs`ではtopic query・view・focusのresetが二重実装されていた。
Local側はRemote側と異なり、`previous_ms`、speed dropdown、Playボタンをresetしていなかった。
新しいWebPlaybackはpaused / 1xなのに、直前のsourceのPause / 2x表示が残り得た。

両経路を`commit_playback`へまとめ、topic loader・feature presentation・focus・frame timestampと
timeline / speed / play controlを同じ箇所で初期化する。
source generationの発行・検査、Local async read、Remote catalog adaptationは引き続き各入口が担当する。
失敗したopenはこの関数を呼ばず、古いsessionを維持する。

## 共通化する階層と、残す差異

共通の意味論はすでに`viewer-core`のSessionPlan、RestorePlanner、PlaybackClock、FeatureRuntime、presentationで共有している。
strict restore、TF history、static TF、serialized camera admissionをアプリへ再実装する必要はない。
同じ`Toggle / SetSpeed`の短いmatchがあるが、共通command frameworkを追加するほどの重複ではない。

物理Sourceとメッセージ供給は`viewer-mcap` / browser loader / RecordingDataPlaneへ分離し続ける。
同期readと、async generation・complete horizon・bufferingには異なる失敗境界がある。
persistent inputの保存も、Nativeの起動時archiveとWebの最初のasync restore bootstrapで寿命が違うため、今回は無理に共通化しない。
queryは再生のstream selectionを拡張せず、FeatureRuntimeにも履歴を保持させない。

残る改善候補は、Nativeで今後multi-workspaceや非同期source再構成が必要になった場合の、session/runtime同時存在の型表現。
またWebのpaused camera選択は他featureも復元するので、計測で問題になればcamera限定復元を検討できる。
今回の目的のためだけにどちらも追加しなかった。

## 検証

- Native: 新しいカメラのCDR復元失敗時にPlan・時計・既存frame・panel bindingを保ち、その後のforward配送も旧streamのままかを追加検証。
- MCAP: 再選択が返す復元候補と、その後のforward traversalを同じ成功testで検証。既存のread / restore失敗rollback testsを維持。
- Native/Web: 既存のSeek rollback、camera変更、TF、async古世代、complete window、query分離テストを実行。
- Browser: `tools/performance-probe/check_web_source_switch.py`で、2x再生中のLocal再open、Local→Remote、Remote→Localのcontrol resetと切替後Seekを確認する。

2026-11-11の結果:

- `docker/check.sh`全体が成功。workspace Clippy / tests、optional scene-extension / plugin-panels、fixtureを生成する個別gates、WASM check、Trunk release、ROS Liveを含む。
- 通常Nativeは93 passed / 2 ignored、Webのhost testsは70 passed / 2 ignored、ROS Liveは96 passed / 5 ignored。生成fixture用のignored testsはcheck.shの個別gatesでも実行した。Webのignored 2件（2.86 GB非圧縮／596 MB Zstd実録画のmanual lazy-read検証）は実行していない。
- Session APIの戻り値をFeatureRuntimeへ限定した最終状態でも、Native全testsと通常／plugin有効時Clippyを再確認。
- Chrome 143の専用headless instanceでLocal初回・2x再生中のLocal再open・Local→Remote・Remote→Localがすべて成功。各source確定後はPlay / 1x / cursor 0、7 camera cards。各切替後のSeekも成功し、browser error / unhandled rejectionは0。
- Browser Seek検査は2回のrequestAnimationFrameを待ち、入力直後のslider値だけでrestore成功を判定しない。

ログは`target/session-review/full-checks.log`、`final-native.log`、`browser-switch-final/result.json`。
ブラウザscriptはsoftware renderingによる機能確認用。Native GUIの目視操作やGPUでの比較計測、新旧のFPS / RSS比較は今回実施していない。

```sh
python3 tools/performance-probe/check_web_source_switch.py \
  --data tests/fixtures/camera-formats/camera_formats.mcap \
  --server target/session-review/recording-server \
  --output target/session-review/browser-switch
```

関連: [message-centric architecture](message_centric_architecture.md)、
[Web data plane](web_recording_data_plane.md)、[recording server](filesystem_recording_server.md)。
