# Desktop: 表示パターン別の負荷

2026-10-20開始、21日完了。mcap_player NativeとLichtblick Desktop 1.29.1を、カメラの台数・面積、Rawの配列展開、Scan点数、RGB PointCloud2点数を変えて比較する。機能の総合順位ではなく、表示内容による負荷と更新頻度の違いを調べる。

[描画と更新の周期、および全カメラの表示上限を10 Hzへ揃えた追加試行](CADENCE.md)を別にまとめた。以下の主計測66試行は製品既定のフォーカス10 Hz・背景最大5 Hzの条件であり、追加試行とは分けて集計する。

## 結果

**旧Desktop比較のCPU・PSSは集計漏れがあったため撤回し、全プロセスを含めて取り直した。** 今回は66試行を完了した。以下は各条件3反復の中央値。

- カメラ1台では双方が約10 Hzで表示した。NativeはCPU 42.7%・PSS 272 MiB、Lichtblickは133.4%・1436 MiBだった。Nativeは縮小IDCTを使うため、この差をMCAPの選択読み込みだけの効果には帰属できない。
- 4台ではNativeのフォーカス9.86 Hz・背景最低5.01 Hzに対し、Lichtblickは9.27 / 8.51 Hz。**この条件ではLichtblickの方が背景を頻繁に更新した。**
- 8台ではNativeのフォーカス9.86 Hz・背景最低4.93 Hzに対し、Lichtblickは4.60 / 3.84 Hz。フォーカスのwall timeに対する画像遅れP95は173 / 3572 msだった。Nativeのfocus優先と縮小decodeが有効なケースだが、背景を5 Hzへ抑える設計とdecode画質の差を含む。総処理能力が同じ条件の比較ではない。
- Scan 36,000点、RGB PointCloud2 60,000点のScene画素変化は双方ほぼ10 Hzだった。6万点ではNativeのCPU 7.2%・PSS 247 MiB、Lichtblickは45.8%・877 MiB。**この程度の点数ではFPS差は現れない。** 65,536点の表示上限内の検証であり、巨大PointCloudの性能は未検証。
- Raw配列展開でNativeのCPU中央値は5.3→8.3%、Lichtblickは30.8→31.0%。UIのフォント・仮想化・query方式は異なる。小さい差の因果的評価には3反復・共有ホストだけでは不足する。

今回の条件ではNativeのCPU・PSSが小さかった。一方、1台表示の画像遅れ中央値はNative 160 ms / Lichtblick 112 msで、4台ではLichtblickの背景Hzが高い。メモリ量やフォーカスHzの1指標だけで総合順位を付けない。GUI framework、読み込み・cache、JPEG decode、表示サイズ、更新方針の差が混ざった製品構成の比較であり、MCAP data plane単体のA/Bではない。

各条件3反復の中央値。CPUは1 core=100%。PSSはsample最大の中央値。背景最低Hzは各run内の背景カメラ最小値の中央値。

| 表示 | app | focus Hz | 背景最低 Hz | Scene変化 Hz | CPU % | PSS MiB |
|---|---|---:|---:|---:|---:|---:|
| camera-1 | native | 9.86 | — | — | 42.7 | 272.0 |
| camera-1 | lichtblick | 9.86 | — | — | 133.4 | 1435.8 |
| camera-4 | native | 9.86 | 5.01 | — | 73.2 | 300.8 |
| camera-4 | lichtblick | 9.27 | 8.51 | — | 368.2 | 2129.2 |
| camera-8 | native | 9.86 | 4.93 | — | 106.1 | 316.4 |
| camera-8 | lichtblick | 4.60 | 3.84 | — | 410.4 | 3423.8 |
| camera-focus-7 | native | 9.86 | 5.01 | — | 110.0 | 331.6 |
| camera-focus-7 | lichtblick | 4.51 | 3.84 | — | 402.1 | 3487.4 |
| raw-closed | native | — | — | — | 5.3 | 210.2 |
| raw-closed | lichtblick | — | — | — | 30.8 | 504.2 |
| raw-open | native | — | — | — | 8.3 | 210.6 |
| raw-open | lichtblick | — | — | — | 31.0 | 539.1 |
| scan-360 | native | — | — | 9.95 | 3.3 | 210.6 |
| scan-360 | lichtblick | — | — | 9.95 | 36.8 | 598.7 |
| scan-3600 | native | — | — | 9.95 | 3.6 | 217.6 |
| scan-3600 | lichtblick | — | — | 9.95 | 42.3 | 603.3 |
| scan-36000 | native | — | — | 9.95 | 5.7 | 244.2 |
| scan-36000 | lichtblick | — | — | 10.03 | 39.0 | 609.0 |
| cloud-10000 | native | — | — | 9.95 | 4.2 | 241.7 |
| cloud-10000 | lichtblick | — | — | 9.95 | 36.3 | 642.8 |
| cloud-60000 | native | — | — | 9.95 | 7.2 | 247.0 |
| cloud-60000 | lichtblick | — | — | 9.95 | 45.8 | 876.5 |

P95はrunごとのP95の中央値で、sampleをpoolしたP95ではない。

| 表示 | app | focus lag 中央値 / P95 ms | 最も遅い背景 lag P95 ms |
|---|---|---:|---:|
| camera-1 | native | 160 / 215 | — |
| camera-1 | lichtblick | 112 / 168 | — |
| camera-4 | native | 160 / 176 | 356 |
| camera-4 | lichtblick | 466 / 526 | 568 |
| camera-8 | native | 140 / 173 | 321 |
| camera-8 | lichtblick | 2490 / 3572 | 3674 |
| camera-focus-7 | native | 156 / 210 | 317 |
| camera-focus-7 | lichtblick | 2564 / 3722 | 3872 |

### 観測の対照試行と検証

8カメラ条件で定期PSS読取だけを外し、各アプリを追加3回ずつ実行した。画素観測と全プロセスのCPU/RSS取得は維持する。これは同時に実行したA/Bではなく、主計測後の共有ホストでの追加検証である。

| app | PSS観測あり focus / 背景最低 Hz | PSS観測なし focus / 背景最低 Hz | CPU あり / なし % |
|---|---:|---:|---:|
| Native | 9.86 / 4.93 | 9.86 / 4.93 | 106.1 / 105.8 |
| Lichtblick | 4.60 / 3.84 | 4.68 / 3.93 | 410.4 / 419.8 |

この6試行では、更新頻度の傾向を逆転させる差は見えなかった。観測の影響がゼロという証明ではない。[対照結果](pss-control.json)に各反復とwrapperのSHA256を保存した。PSS観測なしの再生中peak PSSは未測定で、0として比較しない。

主計測の各runで48〜49回のPSSを取得した。画面電源Offによって条件を満たさなかった1試行は[除外記録](excluded/excluded-dpms-2-scan-3600-native.json.gz)として保存し、同じ条件を取り直した。4 GiB PSSガードによる打ち切りはなかった。設定を無効化せず、各runでDPMS Onを要求した。

全体で264回のSeek操作を実施した。画像条件の96回は期待画素を検査し、Nativeの48回は各カメラtextureの絶対log timeも検査した。Scene/RawのSeekは同じ厳密な画像ID検証やレイテンシー測定ではない。Raw timestampは全runで単調に前進した。

[全反復・範囲](summary.json)、[主計測の実行順](measure-manifest.json)、[source・binary・dataset・script識別情報](environment.json)、[圧縮した生データとログ](runs/)を保存した。gzipの内容は元JSON/logそのもので、summary.jsonに展開後のSHA256がある。校正情報と参照画素も[保存](calibration/)した。終了後は監査した専用PID 388件すべての消滅を確認し、locked / idleだった画面を開始前のOffへ戻した（[cleanup](cleanup.json)）。

PointCloud2製品実装と深度修正はworkspace fmt / clippy / tests、WASM check、Web release build、ROS 2環境の既存Live testsを通過した。深度は実機NVIDIA GPUで重なった2点の両入力順を回帰検証した。比較用release build、Linux子プロセス集計の回帰テスト、Python compile、66件の主計測・6件の観測対照・22件の校正も確認した。比較用のRust変更は製品ソースへ残していない。

次に調べるなら、実写・一意な画像内frame ID・全60秒の連続再生を加え、I/O、decode、upload、cacheの寄与を分ける。PointCloudは65,536点を超えた場合の表示方針を先に決め、全点描画または同じ空間samplingへ揃えてからさらに大きい入力を比較する。現在の「末尾だけを残す」表示で100万点同士の速度を比較しても、同じ内容を描いた比較にはならない。



## 比較スクリーンショット

主計測1反復目の先頭0秒、Play直前の実ウィンドウ。content全体を1920×1080で撮影した。画像クリックで拡大できる。

| 表示 | Native | Lichtblick Desktop |
|---|---|---|
| camera-1 | ![Native camera-1 at time zero](screenshots/camera-1-native.png) | ![Lichtblick camera-1 at time zero](screenshots/camera-1-lichtblick.png) |
| camera-4 | ![Native camera-4 at time zero](screenshots/camera-4-native.png) | ![Lichtblick camera-4 at time zero](screenshots/camera-4-lichtblick.png) |
| camera-8 | ![Native camera-8 at time zero](screenshots/camera-8-native.png) | ![Lichtblick camera-8 at time zero](screenshots/camera-8-lichtblick.png) |
| camera-focus-7 | ![Native camera-focus-7 at time zero](screenshots/camera-focus-7-native.png) | ![Lichtblick camera-focus-7 at time zero](screenshots/camera-focus-7-lichtblick.png) |
| raw-closed | ![Native raw-closed at time zero](screenshots/raw-closed-native.png) | ![Lichtblick raw-closed at time zero](screenshots/raw-closed-lichtblick.png) |
| raw-open | ![Native raw-open at time zero](screenshots/raw-open-native.png) | ![Lichtblick raw-open at time zero](screenshots/raw-open-lichtblick.png) |
| scan-360 | ![Native scan-360 at time zero](screenshots/scan-360-native.png) | ![Lichtblick scan-360 at time zero](screenshots/scan-360-lichtblick.png) |
| scan-3600 | ![Native scan-3600 at time zero](screenshots/scan-3600-native.png) | ![Lichtblick scan-3600 at time zero](screenshots/scan-3600-lichtblick.png) |
| scan-36000 | ![Native scan-36000 at time zero](screenshots/scan-36000-native.png) | ![Lichtblick scan-36000 at time zero](screenshots/scan-36000-lichtblick.png) |
| cloud-10000 | ![Native cloud-10000 at time zero](screenshots/cloud-10000-native.png) | ![Lichtblick cloud-10000 at time zero](screenshots/cloud-10000-lichtblick.png) |
| cloud-60000 | ![Native cloud-60000 at time zero](screenshots/cloud-60000-native.png) | ![Lichtblick cloud-60000 at time zero](screenshots/cloud-60000-lichtblick.png) |

6万点の校正画像（0.31秒）では、入力の3色が両アプリの実画素に存在し、点群の画面上の外接範囲も最大1 pxの差だった。[画素検査](cloud-pixel-check.json)は表示範囲とRGBの確認であり、全画面の一致や連続再生中のメッセージ時刻を証明するものではない。

## 条件

| 表示 | 入力と表示 |
|---|---|
| camera-1 | 4K JPEG 10 Hzを1台、画像1600×900 |
| camera-4 | 同4台を2×2 |
| camera-8 | 同8台を4列×2行 |
| camera-focus-7 | 同8台、左80%にフォーカス、右20%に背景7台 |
| raw-closed / raw-open | LaserScan 64点の配列を閉じる / rangesを展開 |
| scan-360 / 3600 / 36000 | LaserScanを360 / 3,600 / 36,000点、10 Hz |
| cloud-10000 / 60000 | XYZ + packed RGB + intensityを持つPointCloud2、1万 / 6万点、10 Hz。RGBで表示 |

各条件・各アプリをfresh process/profileで3回、各12秒。反復ごとにアプリの実行順を入れ替える。専用ウィンドウを順次実行し、同時起動しない。起動・openと4回のSeekは測定区間外。カメラは25.31 / 48.71 / 16.11 / 0秒、それ以外は13.31 / 18.71 / 6.11 / 0秒へ戻す。記録全区間の連続再生やcold I/O性能の測定ではない。

両アプリの内容領域は1920×1080、DPR=1。Lichtblickのcanvasから画像・Sceneの表示領域を採り、Nativeの測定専用ビルドへ与える。Electronの枠と影だけを撮影から除く。アプリ画面はFHDだが、各カメラをそれぞれFHDで表示する条件ではない。UI、metadata、sidebar、縮小フィルタは各アプリのまま。Rawのフォントと行間も異なり、展開時に同時に見える要素数まで一致させた測定ではない。

Nativeは製品のフォーカス10 Hz・背景最大5 Hzのadmissionを維持する。Lichtblickには表示対象すべての10 Hz streamを与える。**総CPUが小さくても背景が5 Hzなら同じ処理量ではない。** フォーカスと背景の最低更新頻度・遅れを一緒に報告する。

NativeはTurboJPEGの表示サイズに応じたIDCT縮小、Lichtblickは既存のcreateImageBitmap経路を使う。同じ表示面積でもdecode解像度と縮小画質は同一ではない。NativeのGPU_BENCH_FRAMEに記録されるsizeは元画像の寸法であり、縮小後のtexture寸法の証拠ではない。これらの差をMCAPの選択読み込みだけの効果と解釈しない。

Sceneは10×10 mのgrid、間隔1 m、距離20 m、仰角30°、方位45°、垂直FOV45°、near0.5 / far5000 m。物理geometryの倍率補正はしない。点は5 pxの円、decay0。Scanはcyan・alpha0.88、PointCloud2は入力RGB・alpha1。TF軸とラベル、platform形状、Path、カメラを含めない。Nativeの65,536点の表示上限より少なくし、両者とも全入力点を表示する。これを超える大規模点群での同品質比較は今回の対象外。

Rawは同じ64要素のrangesを扱う。NativeのInspectorはbounded queryと64要素のページ、Lichtblickはstream subscriptionと仮想化UIを使う。展開した配列の全要素が同時に画面に見えるわけではない。これは大きな配列を無制限に全描画する実験ではない。

## 入力と環境

カメラは60秒・8台・3840×2160 JPEG・各10 Hzの既存fixture（14,944,878,140 bytes）。32種類の画像を3.2秒周期で繰り返し、カメラごとにvariantを7ずつずらす。JPEG quality=85の合成テクスチャで、実写の圧縮率・decode負荷を代表しない。MCAPは無圧縮・目標8 MiB Chunk・トピック混在。Chunkはメッセージ分だけ目標値を超え得る。各runの事前SHA256読取でcacheは温まり得る。

Scan/PointCloud2はperformance-probeのgenerate-geometryで作る20秒・10 Hzの合成fixture。Scanは2.3〜3.7 mの波形を時間変化させる。PointCloud2は約8×8 mの格子に高さ0.5〜1.5 mの波形と時間変化する3色の帯を与える。frameはbase_scan、単位はmetre。PointCloud2は20 bytes/point、x/y/z FLOAT32、rgb UINT32、intensity FLOAT32。全点が有限値で、drop/downsampleを必要としない。

RTX 5070 Ti / NVIDIA 610.57.04を使用。NativeはNVIDIA Vulkan、LichtblickはNVIDIA ANGLE OpenGL / WebGL enabledを各runで確認する。Mesaソフトウェアレンダリングとの比較ではない。物理画面2560×1440・約60 Hz、DPMS Onを要求する。Lichtblickには--force-color-profile=srgbを指定し、入力RGB値を実画素で確認する。共有ホストで、他用途のGPUメモリ使用がある。小さいCPU差の因果的な優劣や専有環境の上限には一般化しない。

CPUは全子プロセスを含み1 core=100%。Linuxの全threadのchildrenを辿り、Electronのrenderer / GPU / utilityを含める。別threadから起動した子の集計を回帰テストし、Lichtblickではrendererが含まれることも各runで要求する。PSSは別の観測threadで約250 ms周期を目標に取得し、計測区間内に取得完了したsampleの最大値を使う。取得開始・完了時刻と所要時間、sample数も保存する。遅いsmaps_rollupを画素観測と同じthreadで待たない。Python観測プロセス、X server、compositor、GPU VRAMはPSSに含めない。4 GiB PSSと16 GiBのsum RSSは実験の停止ガードで、製品の上限ではない。PSSガードは取得完了時に判定するため、瞬間的な超過を防ぐhard limitではない。起動時peakと短い瞬間的peakは捕捉しない場合がある。

## 観測と限界

カメラは別の校正runで、アプリ・表示領域ごとに32枚の画素参照を作る。Nativeはtextureの絶対log time、Lichtblickは校正中だけJPEG入力SHA256を確認する。主計測ではJPEG hash hookを外す。Seek後に期待画素を確認し、Nativeは絶対log timeも確認する。

XGetImageで各画像内88か所を20 Hz目標で観測する。画像高150 px未満は単一画素、それ以外は5×5 patch平均を使う。平均絶対誤差18未満、次候補との差1超、95%以上の識別成功、識別sample間gap1.5秒未満、前進差分16以下を要求する。更新Hzは観測できた変化の下限。画像が周期的なので、大きな時刻飛びを画素だけで排除する証明にはならない。遅れはPlay指示からのwall timeと画像の相対log timeの差で、最初の1秒を除く。撮像からdisplay走査までの遅延ではない。

Sceneはviewport全体のCRC変化を観測する。これはGPU FPSや点群の絶対log timeではない。校正時にLichtblickのPOINTS draw数、Nativeではfeatureに到達した点数を確認し、代表スクリーンショットも検査する。Rawは実際のメッセージtimestampの単調な前進を確認するが、約250 ms間隔の観測なので10 Hz更新回数を直接数えられない。

カメラとSceneの主計測中はCDPによるUI文字列の取得を行わない。CPU/RSSはsmaps_rollupを使わない軽い/proc読取を使う。CPUは停止指示前に取得した値と実際の経過時間で計算し、観測区間が予定時間を1秒超えた試行は失敗にする。画面読取とprocess情報取得も負荷になる。sampleごとのobserver_work_msとrunごとの分布を保存する。特に8画像や大きなSceneのreadbackを伴うため、無観測の製品性能とは区別する。

## 再現

製品基準は59b24fa。build_desktop_workload_probe.pyは表示位置・camera・grid、操作注入、ログ、Rawの初期展開を測定専用patchとして適用し、release binaryをコピーしてから製品ソースを復元する。I/O、query、JPEG、worker、admission、cacheのアルゴリズムを置き換えない。追加のファイル読取とログ負荷は含む。

~~~sh
python3 tools/performance-probe/prepare_desktop_workloads.py
python3 tools/performance-probe/build_desktop_workload_probe.py
# New fixture paths are required; generate-geometry does not overwrite.
mkdir -p /tmp/mcap-point-cloud2-data /tmp/mcap-web-60s-data
target/gpu-native/performance-probe generate-geometry /tmp/mcap-point-cloud2-data/scan-64-20s.mcap scan 64 20
target/gpu-native/performance-probe generate-geometry /tmp/mcap-point-cloud2-data/scan-360-20s.mcap scan 360 20
target/gpu-native/performance-probe generate-geometry /tmp/mcap-point-cloud2-data/scan-3600-20s.mcap scan 3600 20
target/gpu-native/performance-probe generate-geometry /tmp/mcap-point-cloud2-data/scan-36000-20s.mcap scan 36000 20
target/gpu-native/performance-probe generate-geometry /tmp/mcap-point-cloud2-data/cloud-10000-20s.mcap cloud 10000 20
target/gpu-native/performance-probe generate-geometry /tmp/mcap-point-cloud2-data/cloud-60000-20s.mcap cloud 60000 20
target/gpu-native/performance-probe generate /tmp/mcap-web-60s-data/3840x2160-8cam-10hz-none-8MiB-60s.mcap none 8388608 mixed 60 jpeg 3840 2160 32 10 8
target/gpu-native/performance-probe extract-jpeg /tmp/mcap-web-60s-data/3840x2160-8cam-10hz-none-8MiB-60s.mcap /tmp/mcap-gpu-comparison-jpegs /camera/0 32
python3 tools/performance-probe/run_desktop_workloads.py --output /tmp/workloads-new --stage calibrate
python3 tools/performance-probe/run_desktop_workloads.py --output /tmp/workloads-new --stage smoke --case camera-8 --case raw-open --case cloud-60000
python3 tools/performance-probe/run_desktop_workloads.py --output /tmp/workloads-new --stage measure
python3 tools/performance-probe/summarize_desktop_workloads.py --input /tmp/workloads-new
~~~

入力fixtureとLichtblick配布版を上記のpathに用意する。Pillow、libX11、既存のCDP helperを使用する。prepareは校正済み表示領域を上書きするため、校正後に再実行しない。runごとのbinary/data/harness/patch/reference SHA256、GPU、購読、DPMS、sampleと実行順を保存する。

LichtblickはLinux x64配布版1.29.1、対応sourceは95713edb309d77589b130286642ab149995a5f7b。sourceは前回の比較と同じローカルcheckoutを確認した。配布版の起動には--no-sandboxを使用する。専用profileを使い、終了時は自分で起動したprocess groupだけを停止する。

**旧Desktop比較のCPU・PSSの結論は撤回した。** main threadだけから子を辿る旧helperはElectron renderer / utilityを取りこぼしていた。このレポートは修正後の全プロセス集計で取り直し、旧値と混ぜない。

主計測前の計測ツールの試作では、CDPポート再利用、停止中のReact propsが古い時刻を指すこと、同期的な資源情報取得が重い画面で待たされることを検出した。これらを修正してから条件を固定し、主計測には試作runの性能値を混ぜない。[校正試作の失敗記録](pilot-failures.json)も残す。
