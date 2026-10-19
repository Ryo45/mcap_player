# ADR 0001: Nativeを実GPUで検証する環境

- 日付: 2026-10-19
- 状態: 採用方針。環境構築・Nativeの実GPU描画確認は未実施。
- 対象: Linux上の`viewer-native`。最初の検証機は現在のNVIDIA搭載ホスト。

## 背景と目的

NativeのCamera・Scene・BEVを実GPUで表示し、FHD以上の画面で再生・Seekを検証できる環境を作る。
ソフトウェア描画の結果を実GPUの性能と取り違えず、使ったadapter・driver・表示条件を記録できることを重視する。
このADRは構築方針を決めるもので、描画方式の再実装や性能改善の達成を報告するものではない。

ここで避けたいのはllvmpipe / lavapipe / SwiftShader等によるCPU上の描画である。
Mesaには実GPU用のRADVやIntel driverもあるため、「Mesaであること」を一般的な失敗条件にはしない。
今回のNVIDIAホストでは、NVIDIAのVulkan ICDを使う経路を最初の基準とする。
Vulkan loader・driver・OSを経由する通常のGPU利用であり、driverを迂回する独自の直接アクセスは導入しない。
この区別は[Mesaのdriver一覧](https://docs.mesa3d.org/systems.html)と
[llvmpipeの説明](https://docs.mesa3d.org/drivers/llvmpipe.html)に基づく。

## 調査結果

### リポジトリの現状

| 対象 | 確認した内容 | 意味 |
|---|---|---|
| [Nativeの初期化](../../apps/viewer-native/src/graphics/surface.rs) | wgpu 27.0.1、`Backends::PRIMARY`、`HighPerformance`、window互換surface、`force_fallback_adapter: false` | 既にGPU APIを使う実装。実GPUを必須とする検証条件は未実装 |
| 同上 | 選択した`AdapterInfo`の明示的な起動記録がない | 起動成功や画面が見えることだけでは実GPU利用を証明できない |
| 同上 | present modeは`Fifo`、最大frame latencyは2 | presentationはdisplay/compositorの影響を受ける。無制限FPSの測定として扱わない |
| [Graphics](../../apps/viewer-native/src/graphics.rs) | egui、Camera texture、Scene、BEVをwgpuで描画 | 新しいrendererやcrate分割は今回不要 |
| [Native camera](../../apps/viewer-native/src/graphics/camera.rs) | JPEG pixel decodeとGPU textureへのuploadは別段階 | GPU描画を有効にしてもJPEG decodeが自動的にGPU化されるわけではない |
| [開発コンテナ](../../compose.yaml) | GPU request、X11/Wayland socket、display認証を渡していない | 通常の`dev`はビルド・テスト用。GUI実行環境が成立しているとはいえない |
| [Dockerfile](../../docker/Dockerfile) | `libvulkan-dev`と`mesa-vulkan-drivers`を導入 | パッケージの存在だけでは選択adapterもhardware利用も決まらない |
| [既存Web計測](../performance/2026-10-16-web-focus-scheduling/README.md) | SwiftShaderを明示使用しSceneを非表示 | Nativeのhardware基準値として再利用できない |

`force_fallback_adapter: false`は「fallbackを強制しない」であり、CPU adapterを確実に拒否する指定ではない。
また現在の初期化は`InstanceDescriptor::with_env()`やadapter選択utilityを呼んでいない。
`WGPU_BACKEND=vulkan`や`WGPU_ADAPTER_NAME=...`を設定するだけで選択を制御できる、という手順は書かない。
実装するなら呼び出し側を明示的に変更し、指定失敗をエラーとして扱う。
調査にはCargo.lockとローカルのwgpu 27.0.1 / wgpu-types 27.0.1のsourceを使った。

### 現在のホストで確認できたこと

2026-10-19、ホスト権限で以下を読み取り専用で調べた。新しいGUIやGPU workloadは起動していない。

| 項目 | 観測結果 |
|---|---|
| GPU | NVIDIA GeForce RTX 5070 Ti、VRAM 16,303 MiB |
| NVIDIA driver | `nvidia-smi`成功、610.57.04。kernel moduleも610.57.04 |
| device | `/dev/nvidia0`、`nvidiactl`、`nvidia-modeset`、`/dev/dri/renderD128`等が存在 |
| display | Xorgが動作し、NVIDIAのprocess一覧にも載る。`:1`への`xdpyinfo`接続成功 |
| display認証 | 当該sessionの`/run/user/1000/gdm/Xauthority`で接続を確認。cookie内容は採取しない |
| Vulkan ICD | `/usr/share/vulkan/icd.d/nvidia_icd.json`が存在し、`libGLX_nvidia.so.0`を指定 |
| 診断tool | ホストに`vulkaninfo` / `glxinfo`は見つからない |
| container | NVIDIA Container Toolkit CLI 1.18.1、Dockerの`nvidia` runtimeは登録済み |
| 競合負荷 | 別の計算processが約8.4 GiBのVRAMを利用。idleな専用GPUではない |

通常のagent sandboxではGPU deviceが見えず、`nvidia-smi`が失敗した。
ホストでは成功したため、このsandbox内の失敗をdriver故障とは判断しない。
Native GUI検証は、GPUとdisplayへアクセスできるホスト実行、または明示的にそれらを渡したコンテナで行う必要がある。

**未確認:** NVIDIA Vulkan deviceの列挙成功、windowへのVulkan presentation、
現在のNativeが実際に選ぶadapter、NativeのGPU利用率・VRAM・表示性能。
`nvidia-smi`、ICD manifestの存在、Xorgへの接続成功は、それぞれ単独ではこれらの証明にならない。
過去のNative試行についてもadapter記録が不足しており、software描画だったとは断定しない。

## 決定

### 1. まずホストの既存XorgとNVIDIA Vulkanを使う

最初の成功条件を「現在のホストで、既存の`:1` sessionへNativeを表示し、NVIDIAのhardware adapterを確認する」とする。
Xvfbや新しいdisplay serverは最初の経路には使わない。
既に接続できるXorgを使うことで、GPU driver・container注入・window systemの問題を一度に調べずに済む。

ビルドは既存の開発コンテナを利用してよい。まずdefault featureのrelease binaryをビルドし、
明示したrepository内の出力先へコピーしてホストで実行する経路を用意する。
実行前に`ldd`で共有ライブラリ不足を確認し、Vulkanなどruntimeで読み込まれるdriverも別途診断する。
ホストとのABIが合わない場合はホストでの同toolchainビルドへ切り替え、使用したビルド方法を記録する。
最初からROS runtimeとの依存解決を混ぜず、`ros2-live`は基準ができてから検証する。

ホストにVulkan診断toolを用意し、Nativeと同じユーザー・display・認証・driver条件で
`vulkaninfo --summary`を実行する。必要なpackageの導入は構築作業で行う。本ADR作成時には導入しない。

NVIDIAのICDだけを調べる診断runでは、存在を確認したmanifestをprocess単位で指定する。

```sh
# ホストでの診断手順案。vulkaninfoの導入後に実行する。
timeout 20s env DISPLAY=:1 XAUTHORITY=/run/user/1000/gdm/Xauthority \
  VK_DRIVER_FILES=/usr/share/vulkan/icd.d/nvidia_icd.json \
  vulkaninfo --summary
```

このpathとdisplay番号はこのホストの観測値であり、一般向けscriptの固定値にはしない。
ICDの指定はVulkan driverの探索範囲を絞るためのもので、Nativeの実adapter確認を置き換えない。
診断はtimeoutとログ保存を付けて行う。Nativeも起動・最初の描画に期限を設け、
応答しない場合はその試験processを終了して途中のログを残す。無期限にGUIを起動したままにしない。
`VK_DRIVER_FILES`の意味は[Vulkan loaderのdriver選択仕様](https://vulkan.lunarg.com/doc/view/latest/linux/LoaderDriverInterface.html)を参照する。

### 2. Native側に小さな起動診断と実GPU必須の検証モードを加える

次の実装で`Graphics::new`周辺とCLIへ最小限の変更を加える。

- 選択後の`adapter.get_info()`を起動ログへ出す。name、backend、device type、vendor/device ID、driver情報を含める。
  surface format、present mode、windowのphysical size、scale factorも記録する。
- 実GPU検証用の明示的な指定（例: `--require-hardware-gpu`）を設ける。
  このLinux/NVIDIA試験ではVulkanかつ`DiscreteGpu` / `IntegratedGpu`であることを確認し、
  CPU・VirtualGpu・Otherやbackend不一致は検証失敗として理由を出して終了する。
  通常利用までsoftware adapterを一律禁止する変更にはしない。
- backend指定が必要ならNative起動処理で明示的に受け取る。
  adapter選択は引き続きsurfaceとの互換性を確認する。複数GPUの選択機能は具体的に必要になってから追加する。
- 名前に`NVIDIA`が含まれることだけでhardware判定しない。
  vendor/device ID、device type、backendと外部のprocess観測を合わせて確認する。

起動診断と検証条件はNativeのgraphics/bootstrapに置く。
viewer-core、MCAP読み込み、feature controller、Camera/Scene/BEVのdomain stateへGPU環境設定を持ち込まない。
大きなGPU manager、provider、profile、診断基盤は作らない。
本節のflag・診断出力は**未実装の提案**であり、現行binaryで使えるcommandではない。

### 3. ホストで成立後、必要ならGPU用Compose overrideを追加する

通常の`dev` serviceは現在のビルド・テスト用途を維持する。
GUIをコンテナ内で動かす必要がある場合だけ、別のopt-in overrideまたはserviceを追加する。

- NVIDIA runtimeで対象GPUを明示して渡す。`/dev/dri`のmountだけでNVIDIA driver一式が揃うとは考えない。
- `NVIDIA_DRIVER_CAPABILITIES=graphics,display,utility`を指定する。
  graphicsはVulkan、displayはwindow表示、utilityはNVML等の確認に使う。現在の描画のためにCUDA toolkitは導入しない。
- 対象X11 socket、必要な認証fileを渡し、`DISPLAY` / `XAUTHORITY`を設定する。
  非rootのUID/GIDとdevice accessを確認する。認証情報はread-onlyで渡し、repositoryへ保存しない。
- driver libraryはContainer Toolkitによるホストdriverの注入を使う。
  containerへ異なる版のNVIDIA kernel/display driverを重ねてinstallしない。
- `--privileged`や`xhost +`を前提とせず、必要なGPU・socket・認証だけを渡す。
  GPU-onlyの診断とwindow presentationを分け、両方が通ってからNativeの受入確認を繰り返す。

GPU requestだけではdisplay認証やgraphics libraryの注入まで完了しない。
NVIDIA Container Toolkitの[GPU指定とdriver capabilityの説明](https://docs.nvidia.com/datacenter/cloud-native/container-toolkit/latest/docker-specialized.html)を根拠に設定する。
現在のホストはruntime登録済みのため、最初からDocker daemonの再設定やdriver入替えを作業前提にしない。
Wayland、CDIへの統一、AMD/Intel向けcontainer設定は、最初のNVIDIA/X11基準ができた後に扱う。

## 構築の順番と受入条件

| 段階 | 作業 | 通過条件 |
|---|---|---|
| A: preflight | host / sandboxの境界、device、driver、display、認証、競合負荷を採取 | hardwareが使える実行場所を特定。Vulkan toolで対象deviceとWSIの情報を確認できる |
| B: 起動診断 | Nativeのadapterログと実GPU必須モードを追加 | 判定部分のfocused test、fmt / Clippy / Native testsが通る。software adapterを検証成功にしない |
| C: 最小GUI | release binaryで小さい既存fixtureを開く | FHD以上のphysical windowでCamera・Scene・BEVが表示され、resize、再生、前後Seekが動く。選択adapterとNative PIDのNVIDIA process情報が整合 |
| D: 通常条件 | 実データと既存の60秒・4K/8camera fixtureを使う | CameraやSceneを隠さず、完走・画像の正しさ・Seek後の復元・memory推移を確認。失敗も保存 |
| E: container | 必要ならGPU overrideを追加 | 同じadapter・fixture・layout・出力寸法でC/Dを再実行でき、ホスト実行との差を説明できる |

surfaceを作らないoffscreen計測は、必要なら別のcomponent testとして追加する。
それだけでC/Dのwindow presentationやcompositorまで通過したとは扱わない。
失敗した段階で切り分けを止め、Vulkan列挙が失敗しているのにNativeのdecodeやMCAPを変更しない。

## 計測条件と残す証拠

最初は既存の`Fifo`を維持し、driver側の同期設定を勝手に変更しない。
FPSを増やすためにVSyncを外す実験は、基準結果とは別条件として扱う。

- commit / dirty差分、binary hash、build方法、GPU名、driver版、adapter情報、display server、解像度・refresh rate、scale factorを保存する。
  FHD viewportだけでなくCamera・Sceneの実際の出力pixel寸法、layout、可視panelも記録する。
- 小さいfixtureから開始し、4K/8cameraへ進む。実データと合成データの結果を分ける。
  `nvidia-smi`のNative PIDの出現だけでSceneが正しく描けたことにはしない。画像・操作結果も確認する。
- CPU decode時間、texture upload、render/present、カメラ更新率・画像年齢、Seek latencyを区別する。
  CPUでのsubmit時間はGPU実行時間ではない。GPU timestamp計測は必要になった段階で別途設計する。
- process RSS/PSSとGPU memoryを別々に記録する。
  desktop compositorや別processのVRAMをNativeの消費へ加算しない。観測できないprocess別GPU memoryは未測定と書く。
- 他のGPU workloadを停止する操作は今回行わない。負荷を記録し、比較時は空いている時間帯を使うか、調整した条件を明示する。
  build・dataset生成・別のbenchmarkは計測と併走させない。
- 過去のSwiftShader/Web結果やCPU-only JPEG benchmarkを、GPU有効化前後の同条件比較として扱わない。

## 比較した選択肢

| 選択肢 | 利点 | 問題 / 判断 |
|---|---|---|
| ホスト実行＋既存Xorg＋NVIDIA Vulkan | driverと表示sessionが既に存在し、切り分ける要素が少ない | hostの依存libraryとdesktopの影響は記録が必要。最初に採用 |
| 最初からGPU container | build/run環境をまとめやすい | GPU request、driver注入、socket、認証が同時に変わる。ホスト成功後の追加経路 |
| Xvfb / software描画 | 自動化しやすく、GPUなしCIで使える | 実GPUとwindow presentationの基準にならない。既存の補助試験として維持 |
| offscreen hardware描画 | display接続と分離してGPU単体を調べられる | GUI全体の受入確認を代替できない。必要なら切り分け用に追加 |

## 今回行わない変更

本ADRでは、GPU用Compose設定、system package導入、driver/desktop変更、NativeのCLI・ログ追加を実装しない。
GPU JPEG decode、CUDA、JPEG decoder再実装、thread数・先読み・memory budget変更、renderer再設計も対象外。
次の作業はAのVulkan診断を完了し、B/Cで「実GPUの選択と実際の画面表示」を証拠付きで成立させることである。
