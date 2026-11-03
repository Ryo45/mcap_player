# Web Seek指標の誤判定を修正（2026-11-01）

**結論：前回の約35%退行は、描画時間へ外部poll待ちを混ぜた指標による誤判定。Rust本体の性能退行を示す根拠として撤回する。**
計測器を修正し、同一の既存成果物で交互計測した結果、Raw Seekはmain 70.65 ms、候補73.30 ms（+2.65 ms / 約3.8%）。Web全ケースで既存の回帰閾値内だった。
mainへの統合はこの作業では行わない。

## 原因

v1の `measure_web_recording.py` は次の手順だった。

1. ブラウザの状態をCDPで取得して `state.now` を保存。
2. 別のCDP呼び出しで、該当Seek tokenの描画イベントがあるか確認。
3. 最初の状態取得時刻からSeek時間を計算。次の確認は50 ms後。

描画そのものが終わっていてもpoll確認までの待ちが含まれ、二つのCDP呼び出しの間に描画されると、逆に描画前の `state.now` で完了を報告する競合もあった。
旧archiveの差分 `state.now - draw_event.t` は約 **-25.7〜130.1 ms**。実際の描画遅延の変動より大きく、case順を交互にしてもこの指標の問題は解消しなかった。

[旧データ再解析](v1-reanalysis.json)は、保存された各targetの最終sample（v1 waitの返り値）から `issued = sample.now - reported_ms` を復元し、同じtokenの最初のCamera描画時刻と対応づけたもの。
最初のsuiteを描画時刻基準で再集計するとmain 68.30 ms、候補71.80 ms。追加交互計測はmain 68.90 ms、候補66.15 msだった。両群を合わせた各6 processの中央値はmain 68.525 ms、候補68.575 msで、前回の約35%という差を支持しない。
前回の88.40→119.50 msという観測値は保存するが、アプリの描画性能差とは解釈しない。
再解析はv1のdispatch後の開始時刻を使うため、dispatch直前から測る新v2と直接混ぜて比較しない。

## 対策

- JS observerが、現在のSeek tokenに対応する最初のCamera `drawImage`完了イベントを保持する。poll前に追加frameが来ても最初の時刻を失わない。
- 状態とイベントを一度のCDP snapshotで確認。cursor一致、token一致、issue以後のイベントという条件を守る。
- 指標はブラウザ内の `first_draw.t - issued.t`。開始時刻はinput dispatch直前へ統一し、同期handlerも含める。
- issue / draw / poll時刻、job ID、token、`observation_delay_ms`を保存し、将来の再解析を可能にする。
- この時刻はcanvasのdrawImage返却であり、GPU compositorや物理display scanoutを測ってはいない。16×9画像readbackを使う既存observerの負荷もある。
- 単体テストでpollが遅れてもlatencyが増えないこと、旧token・issue前描画・未確定cursorを受け入れないことを検証。Docker checkへ性能ツールの単体テストを追加。
- 旧baselineとの混用はしない。既存比較はharness hash変更で拒否する。閾値は変更していない。

アプリコード・decode・read-ahead・キャッシュ・Wasm feature gateは変更していない。根拠のないRust最適化は追加しなかった。

## 再計測

前回と同じmain成果物（source `d3a851f`）と、撤回した統合候補の成果物を使用。
[候補のasset検証](candidate-assets.json)で旧entry HTML・JS・Wasm・worker・CSSのSHA-256一致を確認した。
再構築した候補distのinventoryには未使用のbaselineファイルもあるが、実際のentryと参照する候補コードは元のbytesのまま。
計測器だけを変えたA/Bであり、修正前後で製品コードまで変える比較ではない。
Wasm branch単体の新規build比較とは区別する。Nativeの再計測やRust全テスト再実行はしていない（Rust変更なし）。

同じMCAP hash、Linux / NVIDIA RTX 5070 Ti / Chrome hardware WebGPU、1920×1080・focus 960×540。
3 cases × 前後2製品 × 各3独立process = 18試行。caseと製品の順序を交互に変え、compile・別のGUI試験を同時実行しなかった。
`git_commit`は計測器checkoutの情報であり、製品の識別は各resultのasset hashで行う。

| 指標（各3試行の中央値） | main | 統合候補 |
| --- | ---: | ---: |
| Raw FHD Seek / ms | 70.65 | 73.30 |
| Raw FHD focus content Hz | 29.313 | 29.035 |
| Raw FHD peak PSS / MiB | 1166.052 | 1189.110 |
| 4K JPEG / 8 camera focus content Hz | 9.641 | 9.698 |
| 4K JPEG / 8 camera 背景最低content Hz | 2.032 | 2.042 |
| 4K JPEG / 8 camera peak PSS / MiB | 1678.209 | 1652.226 |
| 512 MiB Chunk focus content Hz | 9.616 | 9.679 |
| 512 MiB Chunk 背景最低content Hz | 2.033 | 2.038 |
| 512 MiB Chunk peak PSS / MiB | 1633.846 | 1645.804 |

18/18成功、[既存閾値での比較](comparison.json)はproblemsなし。
[main suite](baseline-suite.json)、[候補suite](candidate-suite.json)を保存。
これは限られた代表ケースで「前回の35%という退行判断を支持しない」結果であり、全記録・全GPUで性能が同一という保証ではない。

## 再現

```sh
python3 -m unittest discover -s tools/performance-probe/tests -p 'test_*.py'
python3 docs/verification/wasm-seek-timing/recheck.py \
  --baseline-dist /path/to/main/dist --candidate-dist /path/to/candidate/dist \
  --data target/performance-regression/data --output target/seek-paired-new
```

両distは先にbuildし、fixtureを同一に固定する。scriptは専用Chromeを使い、各process前にSHA-256で入力を読み、hardware fallbackを拒否する。実GPU・X11・NVIDIA ICDのあるこの環境向けの再現script。
途中失敗・閾値超過で非zero終了し、baselineを上書きしない。

[詳細ログarchive](artifacts/raw-results.tar.gz)は任意取得のLFS。summary・script・テストは通常Git。

```sh
git lfs pull --include="docs/verification/wasm-seek-timing/artifacts/**" --exclude=""
# 旧観測値を改変せず、描画イベントから再解析する場合：
git lfs pull --include="docs/verification/wasm-default-merge/artifacts/**" --exclude=""
python3 docs/verification/wasm-seek-timing/reanalyze_v1.py \
  --archive docs/verification/wasm-default-merge/artifacts/raw-results.tar.gz \
  --output target/seek-v1-reanalysis.json
```
