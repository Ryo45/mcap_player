# 2026-10-31：LFS履歴移行記録

バックアップを別ディレクトリへ保存してから開始する指示を受け、調査時に提示した150パスを移行した。
既存の記事画像・fixture・ソースは通常Gitのまま。featureの製品コードはmainへ統合していない。

## 復旧用バックアップ

ローカルの `/home/ryo/development/mcap_player-backup-20261031-151727/` に保存。
`original.git` は元のローカルGitディレクトリ全体、`before.bundle` は全refのbundle、`worktree.tar` は追跡ファイルと非ignoredの未追跡ファイルを含む。
利用者の未追跡instructionsも保護した。ignoredのビルド出力・生成MCAPは保存対象外。
fsck、アーカイブ全通常ファイルのSHA-256照合を実施した。`SHA256SUMS` と復元方法はバックアップ内にある。
このパスはローカルの復旧先であり、公開される成果物の取得先ではない。

## 履歴と入力

公開元のrefは調査時から変化なし、PR・tagは0。
保存方針commitをmainとfeatureへ適用し、その後の隔離mirrorで全143 commitを移行した。
[commit-map](commit-map-2026-10-31.csv)は移行直前から直後への143行で、公開元の141 commitも含む。移行後に追加した記録commitはmapの対象外。
元の計測ログ中のSHAは改変していない。

```sh
python3 tools/repository-storage/storage.py resolve 50024b45 \
  --map docs/repository-storage/commit-map-2026-10-31.csv
```

元のmain `50024b45e7a402d900b27e5149d0027daed49184` の対応先は `82d6bf57a47567d28b40c1b37908914f3bd3c7c4`。
保存方針commitを含むmainの移行直後は `28c155923b3472bcc8ba4e0a2ebbc3b1517d8c2e`。
featureの移行直後は `f13d4eb153deca3dad82fd7880337278aa4b266d`、masterは `08ef50733c3b031b2792c68018c27247fc6af756`。
最終先端は記録commitの追加でさらに進むため、現在のGit refを確認すること。

## 検証と容量

- 全143 commitの非対象blob・パスが不変、既存attributes保持を検証。
- 144個のLFS payload versionのSHA-256とサイズを検証、fresh mirrorでfsck成功。
- 取得対象は150パス、LFS全履歴81,959,657 bytes（約78.16 MiB）。
- 設定を含む再実験のfresh Git packは86.76 MiB。調査時の145.79 MiBから約40%減。packのまとめ方・最終記録commitで多少変動する。
- 通常cloneでLFS payload取得0、150パスすべてpointerのままを確認。
- その状態で `docker/check.sh` が完走。toolテスト5件、policy、fmt、workspace clippy/test、実fixtureのNative bindings、WASM check、Web release build、ROS Live（95 passed / 5 ignored）が成功。feature製品コードの全テスト再実行はしていないが、履歴移行による非対象blob不変はfeatureを含む全commitで検証した。
- 取得前のpreflight失敗を確認後、単一JSONLをローカルLFSから明示取得。payload 1個だけを取得し、SHA-256とpreflight成功を確認。公開後にはGitHubからの明示取得でも同じSHA-256一致を確認した。

GitHub側の古いobjectの消去や表示容量の即時低下は保証しない。既存cloneには旧履歴が残るため、基本はfresh cloneを使う。
他の作業cloneから旧branchをmerge/pushしないこと。未公開変更は内容を確認したpatch等で移す。
公開は[移行手順](migration.md)にあるLFS upload後・3 headsのみのatomicなforce-with-lease更新を使う。branch削除や無条件force pushはしない。

## 公開後の確認

144 objectsのLFS upload後、main・feature・masterの3 headsを旧SHA指定のatomic force-with-leaseで更新した。branch削除は実施していない。
GitHubから改めて通常cloneし、3 refs一致、fsck、storage policyを確認した。自動LFS取得は0個、単一JSONLを明示pullすると1個だけ取得され、元payloadのSHA-256と一致した。
この公開後fresh cloneの `git count-objects -vH` は **size-pack: 87.03 MiB**。調査時の145.79 MiBから約40.3%減った。
[公開直後の確認結果](verification-2026-10-31.json)はその時点のSHAで、後続の記録commitにより現在の先端は進む。

元の作業ディレクトリのmain・feature・origin参照も新履歴へ揃えた。未追跡の利用者ファイルは保持した。
この既存cloneには旧object・reflogを復旧の余地として残しており、破壊的なGCはしていない。ローカルの保存容量とfresh cloneの転送量を混同しないこと。
