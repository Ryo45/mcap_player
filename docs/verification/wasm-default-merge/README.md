# Wasm統合候補：既定動作の検証（2026-10-31）

**判定：mainへの統合を撤回。Web Raw Image Seekの性能退行を調査する。**
main `d3a851fc8b6383c8138fdcd1862b585a9e770e98` と、Wasm branch
`0a0a718fa3bce922c040fec8726ca6aa24cebe81` の統合候補を測った。
mainのWorkspaceBindingsとfeatureの両方を維持して競合解消した作業ツリーであり、Wasm branch単体を測った結果とは区別する。
統合候補はコミットしていない。mainは元のHEADのまま。競合解消済み全体のpatchは作業環境の
`target/wasm-merge-verification/withdrawn-candidate.patch` に退避した。

## 対象と方法

- Native / Webともfeatures指定なしのrelease build。必要な依存は通常Git、任意LFS未取得で実行。
- 同一fixture、同一マシン、Linux / NVIDIA RTX 5070 Ti / Chrome WebGPU hardware。
- 既存 `tools/performance-probe/regression.py`、Native 5ケース＋Web 3ケース、各3独立process。
- ビルド・テスト・GUI smokeと性能計測を同時実行しない。NativeはGPUを含まない本番データ経路、Webは実ブラウザの画像表示。Native可視FPSの比較ではない。
- 基準のfull suiteは24/24成功、候補も24/24成功。当初のsuite比較は既存閾値内だったが、Web Raw Seekが境界に近いため追加計測した。

[基準suite](before-suite.json)、[候補suite](candidate-suite.json)にはfixture / tool / source hashと機械情報を含む。
[追加Seek計測](seek-recheck.json)は同じRaw fixture・viewport・GPU・4地点Seek条件で、before/afterの順番を交互に変え各3試行。

## 性能結果（各3試行の中央値）

| 指標 | main | 統合候補 |
| --- | ---: | ---: |
| Native FHD Raw 処理時間 ms | 792.407 | 785.098 |
| Native 4K JPEG / 8 camera 処理時間 ms | 5006.219 | 4983.950 |
| Native 6万点 PointCloud 処理時間 ms | 26.291 | 26.145 |
| Native Scan選択 処理時間 ms | 1091.164 | 1088.543 |
| Native 512 MiB Chunk 再訪Seek ms | 0.008 | 0.008 |
| Web FHD Raw focus content Hz | 28.866 | 29.009 |
| Web 4K / 8 camera focus content Hz | 9.628 | 9.690 |
| Web 512 MiB Chunk focus content Hz | 9.693 | 9.645 |
| Web 512 MiB Chunk 背景最低content Hz | 2.296 | 2.031 |
| Web 4K / 8 camera peak PSS MiB | 1668.804 | 1700.241 |
| Web FHD Raw Seek ms：最初のsuite | 92.050 | 114.500 |
| Web FHD Raw Seek ms：追加交互計測 | 88.400 | 119.500 |

追加計測のRaw Seekは **+31.1 ms / +35.2%**。既存のSeek閾値（25%かつ20 ms超）を超える。
最初のsuiteでは相対差が約24.4%で閾値をわずかに下回っていた。閾値通過だけで統合を決めない。
各processのSeek指標は複数地点の画像canvas反映までの値を集約したものであり、CDR decode単独時間ではない。

背景最低更新の約11.6%低下、4K時PSSの約31.4 MiB増加も観測したが、既存閾値内。
Native peak RSSは各ケース約0.4〜1.0 MiB増えた。原因を確定した値ではなく、Nativeの処理時間や再訪Seekに明確な悪化は見られなかった。

## 振る舞い・無効時の確認

- workspace fmt / Clippy / tests、WASM check、Web release build、Native release build成功。
- 実fixtureの外部Bindings、Seek、Camera再選択テスト成功。mainで追加された設定機能を維持。
- 通常Native / Web依存ツリーに `viewer-extension` / `wasmi` なし。Wasm実行経路とPlugin Query moduleはfeature gateの外側へ入らない。
- feature有効でも設定なしなら、有効なsource/cursorを渡してもQuery worker / requestを作らないテスト成功。
- 未選択Pathに壊れたpayloadを渡しても追加decode/errorを出さないテスト成功。
- Native Forwardの12組すべてで、delivered / feature decode / JPEG decode / camera pixel decode数が前後一致。
- Web UIで画像Topic候補7個と4種類の画素値、Topic切替、Source切替、Seek、Path 31点 / Scan 360点 / TF / 0 errorsを確認。
- 通常Nativeを実GPUで起動し、Camera・Path・点群、停止/再開・resizeの画面を確認。Native GUIからのSeek操作はこのsmokeでは自動検証しておらず、Seekは実fixture APIテストと性能probeで検証。
- Native成果物は+32,800 bytes、Web配信物は+28,601 bytes。共通のScene Overlayデータ・空のGPU buffer等の小さい定数コストはあり、「コストが完全にゼロ」とは主張しない。
- ROS Liveとプラグイン有効時の全実動作は今回の確認対象外。

## 再計測と保留課題

[018 Web Seek性能退行](../../issue/018-wasm-default-web-seek-regression/README.md)を参照。
計測手順は[継続性能計測](../../../tools/performance-probe/README.md)。
`regression.py run --engine both --probe <各候補のprobe> --dist <各候補のdist> ...` で、同じdataを使う。
追加計測は `measure_web_recording.py --data <raw-fhd-30hz.mcap> --dist <候補dist> --gpu hardware --cameras 1 --duration-seconds 2.966666666 --seconds 4 --seeks --wake-display --vulkan-icd /usr/share/vulkan/icd.d/nvidia_icd.json --output <新規path>` を前後交互に各3回。

[詳細ログarchive](artifacts/raw-results.tar.gz)は任意取得のLFS。summaryを読むだけなら不要。
```sh
python3 tools/repository-storage/storage.py require docs/verification/wasm-default-merge/artifacts
git lfs pull --include="docs/verification/wasm-default-merge/artifacts/**" --exclude=""
```
巨大な入力MCAP・binary・全スクリーンショットは新たにGitへ追加していない。
