# Rust Style Guideを参考にした変更の検証

[対応内容と判断](../../rust_style_review.md)を参照。

- Plotのtargeted 5テスト、MCAPのnumeric Plot 8／Inspector 7／file source 8テストが通過。
- `mcap-dev-check`でfmt、workspace Clippy/tests、WASM check、Web release、ROS Liveが通過。
  [標準チェックlog](checks.log.gz)に結果を保存。
- 最終のPlot書き込み版でも標準チェック全体を再実行し、
  `viewer-web --features jpeg-benchmark`のWASM checkを加えて通過した。
- Nativeの固定5ケースを各3processで測り、[既存baseline](../../performance/2026-10-25-empty-seek-candidate/raw/native-warm-after/suite.json.gz)と比較して退行判定が成功した。
  許容幅は既存の`regression.py`設定で、今回変更していない。

[Native判定](native-comparison.json)はproblems=[]。
[suite](native-suite.json.gz)にbinary／製品diff／入力／計測器のSHA256と全指標、
[個別観測](native-observations.json.gz)に本番APIのstdoutを保存。
最終のapps/crates差分が、suiteのproduct_diff_sha256と一致することも確認した。

## Plotの直接比較

100万sampleをreduceした後の`finish()`を、変更前と変更後で交互に測った。
各100回を1batch、warmup 3batch後20batch、3つのfresh process。
Rust release / thin LTOで、出力の時刻・値の全配列が変更前と一致することも検査した。
batch中央値をさらにprocess間で集計した値（µs）:

| 点数budget | 変更前 | 最終版 |
|---|---:|---:|
| 400 | 1.265 | 1.105 |
| 4,000 | 19.569 | 8.453 |
| 10,000 | 27.891 | 22.558 |

最終版の全process結果は`plot-final-{1,2,3}.json`、[判定](plot-final-summary.json)は最大10%増加を許容する設定で通過。
時間はwarm-memoryのmicrobenchmarkで、GUI全体の改善率とは扱わない。
[probe](probe/)は旧Plot実装を`before.rs`へ取り込み、現行viewer-coreと同じprocessで比較するもの。
Cargo.tomlはこのホストのパスを含む。再実行する場合はprobeをcheckout外へコピーして依存パスを合わせ、
`cargo run --release --bin style-overview-probe`を実行する。

採用前には直接pushと2回のextendも試した。直接pushは1万点で中央値27.87→29.65µsとなり、
2回extendも遅かったため採用しなかった。`plot-finish*.json`にその結果も保存し、
最後に実際のviewer-core最終版で取り直した3processを上の表に使う。
`native-draft-*`、`checks-initial.log.gz`、`intermediate-web.log.gz`も変更途中の参考記録。

Native代表ケースはRaw／JPEG／Scan／PointCloud2／再訪Seekであり、Plot単体の比較とは分ける。
bucket順・同時刻・measurement time・点数上限はテストで確認し、
数値queryの最終所有結果に必要なVec容量も従来どおり確保している。
Webはビルドとテストの確認で、ブラウザ性能の再計測は行っていない。
