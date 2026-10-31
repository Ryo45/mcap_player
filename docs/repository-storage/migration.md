# 履歴整理の実行計画（本番未実施・承認待ち）

推奨は[レビュー済み150パス](lfs-candidates.tsv)を `git lfs migrate import` で移す案。
先端への追加commitや `--no-rewrite` だけでは古いblobが残る。`git filter-repo` は削除・外部archive移設に向くが、元パスのデータとリンクを別途管理する必要がある。今回は同じパスを保つLFSを選ぶ。
[git-lfs-migrate仕様](https://raw.githubusercontent.com/git-lfs/git-lfs/main/docs/man/git-lfs-migrate.adoc)、[git-filter-repo仕様](https://raw.githubusercontent.com/newren/git-filter-repo/main/Documentation/git-filter-repo.txt)も参照。

## 影響

現時点では全141 commit、main、feature/scene-visualization-extension、masterの3ブランチが書き換わる。tagは0。過去commitに属性が追加されるため、候補のなかった古いmasterの先端SHAも変わる。
一時実験のmainは `50024b45…` → `82d6bf57…` だが、これは公開先SHAの予約ではない。設定commitを追加した後は再計算する。

featureの未公開コードをmainへ混ぜない。先に今回の保存方針・必要なtoolだけを通常commitとしてレビューし、mainとfeatureへ反映する。featureで作業中の人と更新停止時間を合意する。
この作業や既存branch削除は自動では行わない。tagやPRが増えた場合は再調査して承認範囲を更新する。

commit URL、署名、PRのdiff・レビュー文脈に影響する。計測ログにある元SHAは証拠なので書き換えず、old→new CSVを保存して解決する。
既存スクリプトに旧mcap_player SHAを固定した `git show` 呼び出しは見つからなかったが、過去レポートの手動再現コマンドにはSHAがある。

## 1. 更新停止・バックアップ

以下は実行計画。変数の保存先はアクセス制限した作業場所を指定する。

```sh
TASK_STORAGE=/absolute/private/storage-migration
mkdir -p "$TASK_STORAGE"
git ls-remote https://github.com/Ryo45/mcap_player.git > "$TASK_STORAGE/remote-before.txt"
git clone --mirror https://github.com/Ryo45/mcap_player.git "$TASK_STORAGE/backup.git"
git -C "$TASK_STORAGE/backup.git" bundle create "$TASK_STORAGE/before.bundle" --all
git -C "$TASK_STORAGE/backup.git" show-ref > "$TASK_STORAGE/refs-before.txt"
git -C "$TASK_STORAGE/backup.git" fsck --full
```

作業中cloneの未push commit・未commitファイルは別にバックアップする。bundleはそれらやLFS payloadを含まない。
既にLFSがある場合は `git lfs fetch --all` で全payloadも保存する（この操作は取得フィルタを無視する）。mirror全体とLFS cacheを独立した安全な場所にも複製し、checksumとrefs一覧を保管する。
旧履歴のbackup refを本番remoteに公開してはいけない。旧blobを再び到達可能にしてしまう。
GitHub上のruleset・branch protectionと実際のLFS予算・quotaも管理者が確認する。

## 2. manifest確認と隔離実験

方針commit適用済みの最新mainで監査を再実行し、候補を人が確認する。今回のTSVには元データのSHA-256も含むので、変化があれば実験は停止する。

```sh
python3 tools/repository-storage/storage.py audit --output target/repository-storage/audit
python3 tools/repository-storage/experiment.py \
  --source "$TASK_STORAGE/backup.git" \
  --manifest docs/repository-storage/lfs-candidates.tsv \
  --output target/repository-storage/rehearsal.json
```

このtoolは必ず新規 `/tmp/mcap-storage-*` を作り、その中だけで履歴を書き換える。非bare・shallow・heads/tags以外のrefは拒否する。temp mirrorのremoteを削除し、upload/pushしない。
内部の主要コマンドは次の通り（`<...>` は説明用で、そのまま実行しない）。

```text
git clone --mirror --no-local <backup.git> <新しい一時mirror>
git -C <一時mirror> remote remove origin
git -C <一時mirror> lfs install --local --skip-smudge
git -C <一時mirror> lfs migrate import --everything --skip-fetch \
  --include=<TSVの完全一致パスをカンマ区切り> --object-map=<commit-map.csv>
```

`--everything` は全refの到達履歴を対象にするが、remote-tracking refの更新とは同義ではない。完全なmirrorでheads/tagsだけを扱う。実リポジトリでこのコマンドを直接実行しない。
toolは全mapped commitで非対象blob不変・対象payload hash一致・既存属性保持を確認し、さらにfresh cloneのfsckを実行する。

## 3. 公開前検証

- 調査時の全ref・PR・tag数と一致するか再確認。移行中に増えたものがあれば停止して再調査。
- LFS全payloadのhash、必要ファイル、READMEの画像、既存のpatch属性を確認。
- 通常cloneでoptional payload取得0、明示pullで選択取得、取得前preflight失敗を確認。
- payloadなしcloneで `docker/check.sh`。Native/Web/Live、fixture、Web静的ビルドを確認。
- fresh Git packとLFS全履歴量を別々に記録する。今回の予測は145.79 → 87.03 MiB、LFS約78.16 MiB。
- old→new CSVとrefs一覧をbackupへ保管。mapは検証後の通常commitで文書から案内するか、明示的に承認したRelease assetに保存する。map自体を移行入力にして自己参照させない。

```sh
python3 tools/repository-storage/storage.py resolve 50024b45 --map /path/to/commit-map.csv
```

## 4. 承認地点：ここで必ず停止

**本番へのLFS upload・ref更新は未実施。この段階で管理者の承認を得る。**
承認対象は対象150パス、更新する3 heads、全履歴SHA変更、LFS保存・転送コスト、保護ルール変更、更新停止時間。
以下は承認後にだけ実行する具体的な公開コマンド案。`TASK_MIRROR` は検証済み一時mirror、OLD変数は凍結後の `refs-before.txt` にある完全SHAを設定する。調査時のSHAをそのまま再利用しない。

```sh
TASK_MIRROR=/tmp/mcap-storage-REVIEWED/migrated.git
OLD_MAIN=REPLACE_WITH_FROZEN_MAIN_SHA
OLD_FEATURE=REPLACE_WITH_FROZEN_FEATURE_SHA
OLD_MASTER=REPLACE_WITH_FROZEN_MASTER_SHA
git -C "$TASK_MIRROR" remote add publish https://github.com/Ryo45/mcap_player.git
git -C "$TASK_MIRROR" lfs push --all publish
git -C "$TASK_MIRROR" push --atomic \
  --force-with-lease="refs/heads/main:$OLD_MAIN" \
  --force-with-lease="refs/heads/feature/scene-visualization-extension:$OLD_FEATURE" \
  --force-with-lease="refs/heads/master:$OLD_MASTER" \
  publish \
  refs/heads/main:refs/heads/main \
  refs/heads/feature/scene-visualization-extension:refs/heads/feature/scene-visualization-extension \
  refs/heads/master:refs/heads/master
```

先にLFSをuploadし、失敗したらrefsを更新しない。refsは明示した3本だけ、lease付きで同時更新する。`push --mirror`、一律の `--force`、branch削除は使わない。
凍結後にtag等が生じていれば、このコマンドを拡張して勝手に公開せず再承認する。
新しい通常cloneと明示LFS取得を本番から再確認してから更新停止を解除する。

## 5. 既存clone・GitHub上の旧object

作業者にはfresh cloneを配布する。未公開の変更はpatchまたは内容を確認したcherry-pickで移す。旧branchをmerge/pushしたり、旧cloneから新branchを作ったりして旧履歴を戻さない。
旧commit URLは失効する可能性があり、commit-mapで新SHAを案内する。fork所有者も別に対応が必要。

GitHubのPR用ref・fork・SHA直リンク・cacheには旧objectが残る場合がある。fresh cloneの縮小とGitHub表示のrepo size低下は別で、即時のサーバー削除は保証できない。
[GitHubの履歴削除説明](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/removing-sensitive-data-from-a-repository)では、Supportによる非機密データの除去には対応しないと明記されている。性能成果物を機密データ扱いして削除依頼する計画にはしない。

## 6. ロールバック

更新停止中に失敗したら、private backup mirrorにpublisherを設定し、同じ3refを**移行後の完全SHAをlease**にして元のrefsへatomic pushする。これは再度の破壊的更新なので管理者判断で行う。
最初のupload後にrefs更新が失敗した場合は旧履歴がそのまま残る。既にuploadしたLFS payloadは自動で消えず、課金・quotaも自動で元には戻らない。
ロールバック完了後も全員のcloneを揃える。検証済みbackup・mapは新運用が安定するまで削除しない。
