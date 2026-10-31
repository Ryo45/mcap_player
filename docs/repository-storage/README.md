# 計測成果物とリポジトリ容量

通常の開発に必要なソース・fixture・デモ・代表画像はGitに残し、任意の大きな生の計測成果物だけをGit LFSへ分離する。
[2026-10-29の調査](audit-2026-10-29.md)と[履歴移行手順](migration.md)を参照。

**既存150パスは履歴ごとLFSへ移行した。** [2026-10-31の移行記録](migration-2026-10-31.md)と[旧SHA対応表](commit-map-2026-10-31.csv)を参照。移行前からあるcloneには旧objectが残るため、fresh cloneを使う。

## 保存先

| 内容 | 保存方法 |
| --- | --- |
| ソース、設定、テスト用小規模fixture、同梱デモ | 通常Git |
| README・記事の代表画像、Markdown埋め込み画像、Web配信に必要なファイル | 通常Git |
| 計測手法、環境、再現コマンド、集計、比較結論、入力manifest、patch | 通常Git |
| 大きな生の時系列JSON/JSONL、圧縮ログ、補助スクリーンショット | 任意取得のLFS |
| 生成途中のデータ、巨大な再生成可能MCAP、ビルド出力 | `target/` または `.artifact-staging/`。Gitへ追加しない |
| 完結した大量の小さい成果物一式 | 必要ならRelease assetへまとめたarchive。今回はアップロードしない |

新しい計測はまず `target/` 等へ出力し、必要な結果を選んで保存する。既存のスクリプトには `docs/performance/<日付>/` へ直接書くものもあるため、実行後にフォルダ全体を無条件に `git add` しない。

新しい任意成果物の予約場所は `docs/performance/<実験>/artifacts/` と `docs/verification/<実験>/artifacts/`。
`.gitattributes` がこの場所の画像・JSON・ログ・archive等をLFS扱いにする。ソース、必須入力、代表画像をこの場所に置かない。
それ以外の場所へ大きなデータを追加する場合は、参照元と必要性を確認して明示的な属性を追加する。
既存のpatch用whitespace属性は維持している。

小さいログを何千個もLFS化するとオブジェクト数・取得コストが増える。既存の小さいgzipはGitに残す。新しい同種の大量ログは必要ならarchive一つにまとめ、Git側のmanifestにファイル名・サイズ・SHA-256・生成コマンドを残す。
古い・失敗した計測も比較の根拠になるので、再生成できると決めつけて削除しない。元のバイナリ、GPU、負荷、スケジューリングを完全には再現できない。

## cloneと必要な成果物の取得

基本的なビルド・テスト・デモ・Web配信には任意成果物の取得は不要。

```sh
GIT_LFS_SKIP_SMUDGE=1 git clone https://github.com/Ryo45/mcap_player.git
cd mcap_player
python3 tools/repository-storage/storage.py check
```

`.lfsconfig` にも `docs/performance/**,docs/verification/**` の取得除外を設定した。これを含む通常cloneはLFS payloadを自動取得しないことを隔離環境で確認済み。
ただし利用者のGit設定が優先され、古いcommitにはこのファイルがないため、明示的に取得を止める場合は上記の環境変数を使う。
Git LFS未導入でも必要なファイルは通常Gitのまま利用できる。LFSへの新規追加や任意成果物の取得をする人はGit LFSを導入して `git lfs install --local` を実行する。

生データを読む前に取得状況を確認できる。取得済みなら成功し、pointerのままなら取得コマンドを表示する。

```sh
python3 tools/repository-storage/storage.py require docs/performance/2026-10-14-4k-large-chunks
# 未取得なら、エラーに対象と取得方法を表示する。自動ダウンロードはしない。
git lfs pull --include="docs/performance/2026-10-14-4k-large-chunks/**" --exclude=""
```

`--exclude=""` は同梱の取得除外を明示的に解除するために必要。日付単位、または単一ファイルだけを指定する。
分析用スクリプトを追加するときは、必要な任意入力に対して `require` 相当の事前確認を行い、pointerを実データとして扱わないこと。
通常のテストは任意成果物へ依存させない。`docker/check.sh` で必須ファイルとLFS属性・staged pointerの整合性を確認する。

## 継続監査

```sh
python3 tools/repository-storage/storage.py audit --output target/repository-storage/audit
python3 -m unittest discover -s tools/repository-storage -p 'test_*.py'
```

監査結果にはHEADのファイル内訳、全refのblob一覧、テキスト参照、候補リストが出る。出力はGitに追加しない。
候補は `lfs-candidates.tsv` のように必要な分だけレビューして確定する。参照抽出は静的な文字列調査であり、組み立てられたパスや外部利用者を完全に検出するものではない。
履歴内のblobの代表パスは全ての過去の別名を表すとは限らない。実際の移行は全commitを照合する。

Git LFSは同じパスを維持できる一方、全バージョンのpayload保存量とダウンロードが所有者側の課金・quotaに影響する。
[GitHubのLFS課金説明](https://docs.github.com/en/billing/concepts/product-billing/git-lfs)を公開前に確認する。今回、アカウントの実際の予算設定は確認していない。

Release assetは大量の小さい結果を一括取得する用途に向くが、元の相対リンクから自動で読めない。採用する場合はGit側に取得URL・hash・展開先を残す必要がある。
既存の成果物はパスを保てるLFSを第一候補にした。
[GitHub PagesではLFSを利用できない](https://docs.github.com/en/repositories/working-with-files/managing-large-files/about-git-large-file-storage)ため、サイトが直接使うファイルをLFSへ移さない。
