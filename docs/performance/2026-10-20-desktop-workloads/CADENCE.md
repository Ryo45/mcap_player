# 表示の更新と描画呼び出しの周期

追加計測: 2026-10-21。

## 結果

**今回の6万点Sceneは実際の描画も双方ほぼ10回/秒だった。カメラにはNativeの背景5 Hzという明確な差があり、これを10 Hzへ上げると、8台とも約9.86 Hzで表示できた。** 画面の更新に役立たない再描画を増やす変更はしていない。

各条件3反復・各12秒の中央値。CPUは1 core=100%、PSSは再生中のsample最大の中央値。背景は各run内で最も低い更新頻度。Lichtblickも同じ10 Hz入力を購読するが、実際の表示が10 Hzに届くことは条件として強制しない。

| 表示 | app / admission | focus Hz | 背景最低 Hz | CPU % | PSS MiB |
|---|---|---:|---:|---:|---:|
| 4カメラ | Native 全台10 Hz | 9.86 | 9.86 | 106.6 | 291.9 |
| 4カメラ | Lichtblick 追加制限なし | 9.11 | 8.44 | 369.4 | 2142.1 |
| 8カメラ | Native focus 10 / 背景5 Hz・対照 | 9.85 | 4.93 | 106.3 | 312.4 |
| 8カメラ | Native 全台10 Hz | 9.86 | 9.86 | 177.4 | 312.4 |
| 8カメラ | Lichtblick 追加制限なし | 4.50 | 3.76 | 403.8 | 3505.5 |

8台のNativeでは、背景の更新を倍にするとCPUは約67%増えた。focusのwall-time lag P95は184→221 ms、最も遅い背景のP95は333→223 msだった。PSS中央値には大きな増加は見えなかった。短い試行・この表示面積の結果であり、記録全区間や全台を4K解像度のまま表示する能力を保証しない。

この条件では全台10 Hzも選択肢になる。一方、既定の背景5 HzにはCPU消費を抑え、focusの鮮度を優先する意味が残る。製品既定値は変更していない。Nativeの縮小IDCTとLichtblickのfull-resolution decode等の差があるため、このCPU差を更新周期だけやMCAP読み込みだけの効果へ帰属しない。

### 描画処理の呼び出し

| 表示 | Native window present /秒 | Native Scene render /秒 | Lichtblick canvasごとのcolor clear /秒 |
|---|---:|---:|---:|
| 4カメラ・Native全台10 Hz | 59.76 | 0 | 15.55〜16.38 |
| 8カメラ・Native全台10 Hz | 59.76 | 0 | 6.80〜7.05 |
| 6万点Scene | 59.92 | 9.99 | 10.06 |

Nativeのpresentはウィンドウ全体の合成を含み、Lichtblickのclearは各WebGL canvasの描画開始である。この2列を同じ処理として速度比を計算しない。カメラのNative Scene renderが0なのは、画像textureをeguiで合成し、点群用Sceneを描かないためである。

カメラは、Lichtblickの描画開始回数が実際に観測できた新画像の更新回数を上回る。受信時の描画と非同期decode完了後の描画が別に発生し得るコードとも整合する。ただし、この計測から個々のclearの原因やコストは特定できない。

6万点のScene矩形の画素CRC変化はNative 9.95 Hz / Lichtblick 10.61 Hzだった。CRC対象はウィンドウの矩形で、canvasに重なるUI等も含む。10 Hzを超えた値を点群メッセージの描画FPSとは扱わない。実Scene描画の呼び出しは双方ほぼ10回/秒であり、内部描画の周期差が今回のScene性能差の主因、という仮説は支持しない。

両アプリへ同じ入力・表示面積・目標更新頻度を与え、実際の新画像の頻度と遅れを比較する。この条件を揃えるために、内容が変わらない間のNative Sceneを余分に描き直す必要はなかった。

21件の再生試行と2件のNative再校正が成功した。[各反復と圧縮前SHA256](cadence/summary.json)、[実行順](cadence/manifest.json)、[実行物の識別情報](cadence/environment.json)、[専用patch](native-cadence.patch)を保存した。起動中の一時的なX11ウィンドウ列挙エラー1件は[別に保存](cadence-x11-failure/manifest.json)し、同じ手順で取り直した。計測終了後は専用PID 99件の消滅を確認し、locked / idleの画面を開始前のOffへ戻した（[cleanup](cadence/cleanup.json)）。

比較するのは、同じ面積・同じ入力の画像や点群を、どの頻度・遅れでユーザーへ見せられるかである。画面に変化がない間の再描画を増やして、内部の処理回数を一致させる必要はない。

| 周期 | 今回の意味 |
|---|---|
| 入力 | MCAPに記録されたメッセージのlog timeの間隔。今回のカメラ・点群は各10 Hz |
| 表示内容の更新 | 新しい画像・変化したSceneがウィンドウの画素として観測された頻度。今回の主な比較指標 |
| 描画処理 | Scene render、WebGL clear、window present等の呼び出し。何を数えたかを個別に明示する |
| ディスプレイの更新 | モニターの走査周期。今回の環境は約60 Hz。アプリの呼び出し回数と同じ意味ではない |

10 Hzの新しい画像を60 Hzの画面上へ合成しても、新しい画像が60枚あることにはならない。また、メッセージ時刻が10 frame/秒進んでも、途中の画像を飛ばしていれば見える更新は10回/秒より少なくなる。

## コード上の違い

製品の基準は59b24fa、Lichtblickの確認対象sourceは95713edb309d77589b130286642ab149995a5f7b。

- NativeのwindowはFifo / VSyncで、RedrawRequestedの後に次のredrawを要求する。画面全体の合成と、Scene textureの更新は別である。Sceneはresize・内容・camera等の変更時だけ描画し、変更がなければ既存textureを合成する。
- Nativeのカメラadmissionはフォーカス100 ms、背景200 ms。背景を間引くのはJPEG decodeより前であり、workerを増やしてもこの上限は変わらない。
- LichtblickのIterablePlayerは1 tickあたり少なくとも約16 msを目標に待機する。固定60 FPSを保証するschedulerではなく、読み込み・panel処理が重い場合は遅くなる。
- LichtblickのThreeDeeRenderはcurrentFrameMessagesが変わると描画を要求する。Rendererはcolor/depth clearとscene renderを行い、自動clearは無効。メッセージのないtickでもpanel更新が発生すれば、同じ内容を再描画し得る。Image panelもこのRendererを使う。
- LichtblickのImageRenderableは非同期decodeの完了時にもqueueAnimationFrameを呼ぶ。入力を処理する描画とdecode完了を反映する描画が別に発生し得る。固定の「毎秒60回描画」と推定せず、canvasごとの実際の呼び出しを数える。

確認箇所: Native [event loop](../../../apps/viewer-native/src/app.rs)、[surface](../../../apps/viewer-native/src/graphics/surface.rs)、[Scene更新](../../../apps/viewer-native/src/graphics.rs)、[camera admission](../../../crates/viewer-core/src/feature/controllers/camera.rs)。Lichtblickは対応する[IterablePlayer](https://github.com/Lichtblick-Suite/lichtblick/blob/95713edb309d77589b130286642ab149995a5f7b/packages/suite-base/src/players/IterablePlayer/IterablePlayer.ts)、[ThreeDeeRender](https://github.com/Lichtblick-Suite/lichtblick/blob/95713edb309d77589b130286642ab149995a5f7b/packages/suite-base/src/panels/ThreeDeeRender/ThreeDeeRender.tsx)、[Renderer](https://github.com/Lichtblick-Suite/lichtblick/blob/95713edb309d77589b130286642ab149995a5f7b/packages/suite-base/src/panels/ThreeDeeRender/Renderer.ts)。配布binaryの動作は別途呼び出し回数で確認する。

## 追加試行

4カメラ・8カメラをそれぞれ同じ配置・画像面積で表示し、Nativeの背景admissionだけ100 msへ変更する。フォーカス・背景とも入力の10 Hzを上限として表示する条件である。Lichtblick側に追加の間引きは入れない。8カメラには元の背景5 Hzを使うNativeの対照も加える。各条件3反復、各12秒、fresh process/profileで順次実行する。

6万点のSceneは元から双方10 Hz入力・全点表示の条件である。表示上の変化と描画呼び出し回数を区別して計測し、同じ点群を余分に描く変更は行わない。

主計測のharnessは変更せず、[専用wrapper](../../../tools/performance-probe/measure_desktop_cadence.py)でbinaryを差し替える。[専用build](../../../tools/performance-probe/build_desktop_cadence_probe.py)は元の表示位置調整に、admissionの実験切替と描画回数ログを追加する。製品ソースはbuild後に復元する。既定の製品方針は背景最大5 Hzのままである。

元harnessのhashと、binary pathを一箇所差し替えた実行コードのhashを両方保存する。各runの実験条件はcadence_experimentに記録する。元harnessから引き継いだcamera_policyには製品既定値の説明も含まれるため、Native全台10 Hzの条件はall_camera_10hzと合わせて読む。

Nativeはplay/pause操作間のpresent呼び出しとScene render呼び出しを数える。LichtblickはWebGLのCOLOR_BUFFER_BIT付きclearをcanvasごとに数える。layout importで破棄されたcanvasは集計から除く。clearは描画処理の開始であり、OS compositorのpresent完了やディスプレイ走査回数ではない。呼び出し回数が多くても新しい画像が見えた回数とは扱わない。実際の画像内容の変化は主計測と同じ画素観測で判定する。

観測用ログ・WebGL wrapperの負荷は含む。Nativeの縮小IDCT、Lichtblickのfull-resolution decode等の違いは維持するため、出力の画質や全画素まで同一にしたdecoder単体の比較ではない。

GPUを別用途で使用していた最初の追加試行はユーザーの指示で中断した。その際にはGPU全体100%、別のcompute processでSM利用率79%を観測し、XGetImageの待ちも増えていた。この試行の性能値は採用せず、新しい出力先で校正からやり直す。再開前のGPU利用率は0%、power 22.36 Wだった。共有ホストのため、利用率0%が全測定区間で維持される保証ではない。

[競合中の除外記録](cadence-gpu-contended/manifest.json)には中断されたrunも含め、元JSON/logとそのSHA256を残した。

再現:

~~~sh
python3 tools/performance-probe/build_desktop_cadence_probe.py
python3 tools/performance-probe/run_desktop_cadence.py --output /tmp/cadence-new --references-root /tmp/workloads-new
python3 tools/performance-probe/summarize_desktop_cadence.py --input /tmp/cadence-new
~~~

references-rootは主計測の校正済み出力先であり、Lichtblickの参照画像を再利用する。Nativeは専用binaryのhashが異なるため、4・8カメラとも再校正する。
