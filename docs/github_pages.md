# GitHub PagesでWeb版を公開する

公開URL: **https://ryo45.github.io/mcap_player/**

Open MCAPで選んだ記録はブラウザ内で処理する。記録のアップロード先や、再生用バックエンドは不要。
GitHub Pagesへ公開するのは、Trunkが生成するHTML / JS / CSS / Wasm / 画像デコードWorkerだけ。
利用者のMCAP、性能計測成果物、fixture、リポジトリ全体は配信artifactへ入れない。

## 自動公開

[Pages workflow](../.github/workflows/pages.yml)は、mainにWeb・共通crate・設定・Cargo依存・workflow自身の変更がpushされた時に実行する。
Actionsの**Deploy Web viewer to GitHub Pages**から手動実行もできる。

1. Ubuntu 24.04でclang/lld 18、Rust 1.91.1とWasm target、Trunk 0.21.14を準備する。ROS 2やNativeのGUI依存は不要。
2. Pagesから取得したbase pathを`trunk build --release --locked --public-url`へ渡す。
3. `apps/viewer-web/dist`だけをPages artifactとしてuploadする。
4. build成功後に`github-pages`環境へdeployする。

公式Actionsをcommit SHAへ固定し、Trunkの公式配布archiveはSHA-256を検査する。
checkout時にoptional LFS成果物を取得しない。権限はbuild側がcontents / pagesのread、deploy側がpages / id-tokenのwrite。
実行中のdeployはキャンセルせず、同時公開を直列化する。

リポジトリをforkして公開する場合は、Settings → Pages → Build and deploymentで**GitHub Actions**を選び、workflowを実行する。
URLのbase pathはPagesから取得するため、fork先の名前をRustへ埋め込む必要はない。
参考: [GitHubのcustom workflow手順](https://docs.github.com/en/pages/getting-started-with-github-pages/using-custom-workflows-with-github-pages)。

## ローカルで同じ配信構造を検証する

通常の`trunk serve`は引き続き`/`で使える。公開ビルドはproject siteの`/mcap_player/`へ配置する。

```sh
mkdir -p target/pages/site
docker compose run --rm dev bash -lc \
  'cd apps/viewer-web && trunk build --release --locked --public-url /mcap_player/ --dist ../../target/pages/site/mcap_player'
python3 -m http.server 8094 --bind 127.0.0.1 --directory target/pages/site
```

`http://localhost:8094/mcap_player/`を開く。
HTMLの`<base data-trunk-public-url>`をTrunkが書き換えるので、Wasm / CSSだけでなく、`document.baseURI`を使うcamera Workerもこの配下から読み込む。
参考: [Trunkのpublic URL](https://trunkrs.dev/advanced/paths/)。

別terminalから、既存のChrome/CDP検証ツールを同じURLへ実行できる。

```sh
python3 tools/performance-probe/check_web_camera.py --camera-formats \
  --data tests/fixtures/camera-formats/camera_formats.mcap \
  --url http://localhost:8094/mcap_player/ \
  --output target/pages/camera-check.json
```

専用Chromeで7種類の画像の画素、前後Seek、Worker URLとbrowser errorを確認する。出力先は未作成のpathを指定する。
公開URLを`--url`へ渡して同じ確認もできる。これは機能確認用で、software renderingによるGPU性能比較ではない。

## 利用上の範囲

- PagesのHTTPS上で、WebGPU対応ブラウザと対応GPUドライバーを使う。対応範囲は[Native / Web一覧](native_web_parity.md)を参照。
- この公開方式にSharedArrayBufferやcross-origin isolation用の追加ヘッダーは不要。画像WorkerへはArrayBufferをtransferする。
- Recording Serverは別途起動する機能で、Pagesには含まれない。Remote利用ではPagesのorigin `https://ryo45.github.io`をサーバーのCORS設定へ許可する。HTTPSページからHTTPサーバーへの接続にはブラウザ側の制約もある。
- 上記の公開はWebアプリの継続配信で、Nativeの配布バイナリやGitHub Releaseタグは作成しない。
