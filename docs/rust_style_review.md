# Rust Style Guideを参考にした改善

2026-10-26、基準commit `3b569c0`。
[Google Rust Style Guide](https://google.github.io/styleguide/rustguide.html)を読み、
共有feature・query、MCAP I/O、Remote protocol、Server、Native/WebのAPIとimportを確認した。
全規則への一括準拠を目的にせず、現在の利用箇所で効果があるものを変更した。

## 対応した点

### Plot集約の一時Vec

`SignalOverviewReducer::samples()`は常に所有Vecを作り、`finish()`がそれを再走査して
X軸と値の2配列へコピーしていた。借用iteratorを返し、builtin Plotは最終2配列へ直接書き込む。
最終2配列は保持点数ぶん確保し、両配列の要素とsamplesをzipして書き込む。
数値queryの`NumericPlotOverview`は結果を所有する必要があるため、そこではVecを維持する。
従来と同じretained point数で先に容量を確保し、iteratorの消費による再確保を避ける。

bucket内の時刻順、同時刻のmin/max順、measurement time、有限入力数、点数上限を維持する。
0・1・奇数のbudget、定数列、部分的なiterator消費と繰り返し取得を回帰テストした。
API利用側は必要な場合だけcollectする。旧Vec APIの別名や互換wrapperは追加していない。

### Importから依存元を読み取れるようにする

MCAPのfile source・Inspector・numeric Plotのintegration testに残っていた
`viewer_core::*` / `viewer_mcap::*`を実際の型名へ置き換えた。
Webの通常起動とJPEG benchmarkも、wasm-bindgenのmacro・型・traitを明示した。
変更箇所ではモジュール別にimportを分け、1テストだけが使う`std::error::Error`はその関数へ移した。

## 今回変えなかった設計

- `Bytes`のbackingやworker/sourceの共有に必要なArcは維持した。所有関係を複雑にしてまで排除しない。
- 全公開型への`non_exhaustive`追加や、constructor／unwrapの一律renameは行わない。
  このリポジトリのpre-1.0方針、完全なmatch、証明済みの不変条件を優先する。
- Chromium固有のbuild・thread・async規則を、Cargo／Tokio／WASMへ持ち込まない。
- ファイルサイズだけでcrate/moduleを分割せず、新しいtraitやgeneric wrapperも作らない。

## 検証

targetedなPlot/core/MCAPテスト後、最終版を標準Dockerチェックでfmt、workspace Clippy/tests、
WASM、Web release、ROS Liveまで確認した。importを変更した`jpeg-benchmark` featureも
WASMで明示的にcheckした。
Nativeの代表5ケースも同じfixture／計測器で各3反復し、既存の変更前baselineからの退行判定を通過した。
別途、100万sampleを集約したPlotのfinishも変更前と交互に比較し、3つのbudgetで確認した。
[ログとNative判定](verification/2026-10-26-rust-style/README.md)を保存した。
