# Native: ホストのNVIDIA Vulkanで実画面を確認

2026-10-19。[ADR 0001](../../adr/0001-native-hardware-gpu-environment.md)の方針1を試した。
**ホストの既存XorgとNVIDIA VulkanでCamera・BEV・3Dを表示できた。画面電源Onが必要だった。**
GPU container、CUDA toolkit、rendererの変更、NVIDIA driverの入替えは行っていない。

## 確認したもの

- source commit `1cceb6f`、既存dev containerのRust 1.91.1でdefault featureのrelease build。
  ビルドしたbinaryだけをホストへコピーして起動。`ldd`で未解決libraryはなかった。
- RTX 5070 Ti、NVIDIA driver 610.57.04、Vulkan。
  ICDは`/usr/share/vulkan/icd.d/nvidia_icd.json`をprocess単位で指定した。
- 既存Xorg `:1`、2560×1440 / 59.95 Hz。Native clientは1920×1080 / scale=1。
  途中で1600×1000へresizeして再描画も確認した。
- [README撮影用fixture](../../screenshots.md)の5.018秒記録。
  Camera、Scan 360点、TF、Odometry、生成Pathを使う。4K/8cameraの負荷試験ではない。
- `wgpu_core::instance=debug`の**Request adapter result**から、
  `NVIDIA GeForce RTX 5070 Ti / DiscreteGpu / Vulkan / driver NVIDIA 610.57.04`を確認。
  vendor=4318（0x10de）、device=11269。adapter候補一覧だけで判定していない。
- 同じNative PIDが`nvidia-smi`に`C+G` processとして載り、各撮影時にGPU使用memoryも記録された。
  表示例は約27–35 MiB。短いsmoke中のsnapshotであり、peak VRAMの計測ではない。
- 実際のclient areaだけをXGetImageで取得した。Desktop全体や別アプリは撮っていない。
  画像は合成・描き直しではなく実ウィンドウのcapture。
- 通常binaryでCamera画像、Camera overlay、BEVのPathとfootprint、SceneのScan・Path・gridを目視確認。
  停止中のcursor維持と、再開後の画像・Scan・cursor更新、終端までの再生も確認した。

## 画面電源がOffだとFIFOが約1秒待つ

| 条件 | 同じ通常binaryのUI/render表示 | 証拠 |
|---|---:|---|
| DPMS Off / NVIDIA display inactive | 約995 ms | [standard/paused.png](standard/paused.png) |
| DPMS On / NVIDIA display active | 約16.5 ms | [standard-display-on/resume.png](standard-display-on/resume.png) |

両方ともNativeの設定は`PresentMode::Fifo`。binary SHA256も同じ。
`xrandr`はOff時にも59.95 Hzを報告していたが、`xset q`は`Monitor is Off`、
`nvidia-smi`はdisplay inactiveだった。
`xset dpms force on`で画面電源だけOnにすると、display activeがEnabledへ戻り、上の差を観測した。

このUI表示はCPU側のrender/presentを含むwall時間で、GPU timestampではない。
単発のsmokeのsnapshotから平均FPS、GPU実行時間、一般的な性能向上を主張しない。
ただし実GPUを選べたことだけでは、表示環境まで良い状態だとはいえないことが分かった。
**今後の比較ではDPMSとdisplay activeを記録し、画面電源Onを固定する。**

sessionは`LockedHint=yes` / `IdleHint=yes`だった。
ロック解除、ロック設定の無効化、`xhost +`、desktopやdriverの設定変更は行っていない。
一時的なDPMS wakeはロック解除ではない。
試験後、sessionが引き続きlocked / idleであることを確認し、画面電源を元のOffへ戻した。
[終了処理の記録](cleanup.json)を保存した。起動したNative processは残していない。
通常の対話操作や他アプリとの性能比較は、利用者が使える通常sessionで行い、
ロック画面・背面化・occlusionによる各アプリの抑制も確認する。

## 停止・再開とSeekの確認方法を分ける

通常binaryでは、対象Native windowのPIDを確認し、そのwindowに限定したTab/Spaceで停止・再開を操作した。
[停止](monitor/paused.png)と[停止維持](monitor/paused-stable.png)はcursorが同じで、
[再開](monitor/resume.png)ではcursorと画像が進む。通常レイアウトでも同じ確認をした。

一方、ロック中のsessionでXTestのbutton入力を試しても、診断したNativeの`MouseInput`としては観測できなかった。
keyboardのtimeline矢印でも秒単位のSeek確認はできなかったため、**通常GUIのmouse Seek成功とは報告しない**。
これは自動操作・session状態の制約を含む観測であり、NativeやGPU driverの一般的な入力不具合と断定しない。

GPUを含むSeek復元経路については、[一時検証patch](seek-probe.patch)を適用した別binaryを使った。
外部のrequestを読み、GUIと同じ`App::apply_frame_actions` → `PlaybackCommand::Seek`を通す。
専用のrestoreやrendererは実装していない。通常binaryとこのbinaryのhashは各result.jsonで区別する。

- [1.000秒へ戻した状態](seek-probe-display-on/seek-back-stable.png)：PAUSED、Scan360点、Camera、Path、overlayが復元。
- [4.000秒へ進めた状態](seek-probe-display-on/seek-forward-stable.png)：cursor・Scan・Cameraの更新を確認。
- その後1秒へ戻し、[通常UIのSpaceで再開](seek-probe-display-on/resume.png)して進むことも確認。
- requestのidと正確なtarget nsは[native.log](seek-probe-display-on/native.log)に保存。
  連続したcaptureで復元後の状態も確認した。pixel単位の正解fixture比較やrandom Seek latency計測ではない。

このpatchのRust変更は試験後に戻した。配布sourceへの追加はPythonのsmoke driverと文書・証拠だけである。
GPUを使ったSeek復元の確認と、通常GUIのpointer操作の確認は別物として残す。

## 保存したrun

| ディレクトリ | binary | 画面電源 | 用途 |
|---|---|---|---|
| [monitor](monitor/result.json) | 通常 | Off | Camera・Scene、停止・再開・resize |
| [standard](standard/result.json) | 通常 | Off | Camera・BEV・Scene同時表示、初期の遅いFIFOを発見 |
| [standard-display-on](standard-display-on/result.json) | 通常 | On | 同一binaryでFIFO表示の改善を確認 |
| [seek-probe](seek-probe/result.json) | 検証patch付き | Off | action経路からのSeek復元 |
| [seek-probe-display-on](seek-probe-display-on/result.json) | 検証patch付き | On | 1秒・4秒の復元と再開を確認 |

result.jsonにはbinary / fixture / Python probeのhash、source commit、起動command、撮影時刻、寸法、
Native PIDのNVIDIA process行、process memory、captureのSHA256を保存する。
JSONは撮影した全frameの記録を保持し、repositoryには[代表画像](retained-images.json)だけを保存した。
一覧にないPNGは一時撮影物で、repositoryには含めていない。
最終runでは`xset` / `xrandr` / GPU情報も記録した。
`status=captured-hardware-gpu`はadapter・process・寸法・エラー確認までの自動判定で、
画像内容やUI操作が自動的に正解と判定された意味ではない。上記の目視確認を別途行った。

## 再実行

必要なのはホストのX11/Vulkan環境と、`xwininfo` / `xprop` / `xset` / `xrandr` / `nvidia-smi` / Python 3.11以上。
Pythonはstdlibと`libX11.so.6`を使う。認証cookieの内容は保存しない。
Vulkan deviceの選択結果と実画面を直接確認できたため、今回は`vulkaninfo`のinstallは不要だった。

```sh
# ビルドはcontainer、GUI実行はホスト。
docker compose run --rm dev bash -lc \
  'cargo build --release -p viewer-native --locked && mkdir -p target/gpu-native && cp /home/dev/target/release/viewer-native target/gpu-native/viewer-native'

# 必要なら先にREADME fixtureを生成する。生成方法はdocs/screenshots.mdを参照。
# 表示sessionの所有者として、画面電源とlock状態を確認する。
DISPLAY=:1 XAUTHORITY=/run/user/1000/gdm/Xauthority xset q
nvidia-smi --query-gpu=name,driver_version,display_active --format=csv

# 出力先は新規directory。画面・認証・ICDのpathはホストに合わせる。
timeout --signal=TERM --kill-after=3s 55s \
  python3 tools/performance-probe/check_native_gpu.py \
  --data /tmp/mcap-readme-demo/recording.mcap --layout standard \
  --output /tmp/mcap-native-gpu-new
```

smokeは既存session上にwindowを開く。defaultは`:1`と当該ホストの認証path。
必要なら`--display` / `--xauthority` / `--icd` / `--binary`を指定する。
windowはspawnしたNative PIDで識別する。起動・全体に期限があり、試験後はそのprocessだけを終了する。
画面電源を自動で変更したり、sessionのlockを解除するdriverではない。

Seekのaction probeを再現する場合は、cleanな作業copyで`git apply --unidiff-zero`により
`seek-probe.patch`を適用し、別名のbinaryとしてbuild・copyした後、Rust sourceを元へ戻す。
そのbinaryを`--binary`で指定し`--probe-seek`を付ける。
patchのない通常binaryへこのflagを付けても成功判定しない。

## 今回の限界と次の比較

これは実GPUでの成立確認であり、性能benchmarkではない。
他の計算processがVRAMを使う共有ホストで、専用のidle GPUではなかった。
4K/8camera、長時間memory、GPU timestamp、GPU container、通常のmouse Seekは未検証。
small fixtureのCameraは4K画像ではない。Cameraの校正はbundled demo値である。
`VK_EXT_physical_device_drm`不足とsRGB framebufferに関する警告は残るが、今回の表示失敗やdevice lossはなかった。

比較対象の第一候補は[公開Desktop版のあるLichtblick](https://github.com/lichtblick-suite/lichtblick)。
次点は[MCAPを取り込めるRerun Viewer](https://docs.rerun.io/dev/howto/logging-and-ingestion/mcap/)。
Rerunでは取込み・内部表現への変換も含むopenのコストと、再生中のコストを分ける。
どちらも実GPU利用を個別に確認してから、同一MCAP・可視topic・画像出力寸法・画面電源・refreshを揃える。
最初は1 camera＋Scanの短い記録で正しい画像更新と操作を確認し、比較が成立した後に8camera・60秒へ進む。
比較対象のDesktopアプリがこのGPU環境で動くことはまだ確認していない。
