# ドキュメント

[README](../README.md)は紹介と最短の利用手順です。設定や開発の詳細はこちらから辿れます。

## 利用・設定

| 目的 | ドキュメント |
| --- | --- |
| Nativeの依存導入・ビルド、Webのホスト起動 | [ビルドと起動](building.md) |
| Web版の公開URL・静的配信・自動deploy | [GitHub Pages](github_pages.md) |
| 手持ちのMCAP、トピック名、WorkspaceBindings、対応形式と上限 | [記録・入力設定](recording_usage.md) |
| 表示レイアウトを選ぶ | [レイアウトギャラリー](layout_gallery.md) |
| 画像トピック・フォーカス・Preview | [Camera選択](camera_topic_selection.md) |
| 点群の形式・色分け | [PointCloud2](point_cloud2.md) |
| 車体形状・TF・Camera calibration | [物理geometryと座標系](physical_geometry.md) |
| 索引のない記録を変換する | [fastwrite対応](fastwrite.md) |
| LAN上の記録をWebで開く | [Recording Server](filesystem_recording_server.md) |
| ROS 2に直接接続する | [Native Live入力](ros2_live.md) |
| Native / Webの機能差を確認する | [対応一覧](native_web_parity.md) |

README画像の入力・撮影条件は[スクリーンショットの再現手順](screenshots.md)、
Chase・車体・方向マーカーの変更前後は[Scene視認性の比較](verification/scene-visibility/README.md)にあります。

## 性能・検証

- [性能検証の入口](performance/README.md) / [継続性能計測の実行方法](../tools/performance-probe/README.md)
- [Lichtblick Desktopとの表示パターン別比較](performance/2026-10-20-desktop-workloads/README.md)
- [Webの巨大Chunk・20GiB単一Chunkの境界試験](performance/2026-10-24-web-chunk-streaming/README.md)
- [画像トピック選択・Preview修正の最終検証](verification/2026-10-27-camera-focus-followup/README.md)
- [Native JPEG](native_jpeg.md) / [Web画像処理](web_jpeg.md) / [Native health panels](native_recording_health.md)

性能は圧縮方式、Chunk構成、選択トピック、画像形式、記録長、Seekの条件を変えて評価しています。
比較対象と表示内容を揃えた測定、成功・失敗例、リソース上限を記録しています。

## 開発・拡張

- [Docker開発環境・全検証ゲート](development_container.md)
- [アーキテクチャ](message_centric_architecture.md) / [module構成](module_structure.md)
- [Native / Webのセッション所有関係](session_ownership_review.md)
- [Scene Visualization Extension](scene_visualization_extensions.md) — Nativeの任意機能。Detection3DArray / Pathを入力に外部WasmでScene描画。
- [Wasm PluginPanel PoC](wasm_plugin_panels.md) — Nativeの任意機能。複数の独自ROSメッセージ・可変長配列の現在値と履歴グラフを外部Wasmで定義。
- [fixtureツール](../tools/ros-fixture/README.md)
- [成果物の保存・取得方針](repository-storage/README.md) / [LFS移行・旧commit SHAの対応](repository-storage/migration-2026-10-31.md)

全検証ゲートはリポジトリのルートで実行できます。

```sh
docker compose build dev
docker compose run --rm dev mcap-dev-check
```

fmt、workspace Clippy / tests、WASM check、Web release build、ROS Live tests等を実行します。
通常のホスト環境では、まず`cargo fmt --all -- --check`と`cargo test --workspace`を利用してください。
