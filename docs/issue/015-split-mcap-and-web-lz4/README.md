# 015: 分割MCAPの時間軸とWeb LZ4

## 課題と範囲

ROS 2の記録は複数MCAPへ分割される。従来Nativeは1ファイル、Browser Localは非圧縮・Zstdだけを扱い、分割境界を越える再生やブラウザでのLZ4表示ができなかった。

今回の範囲はNativeの分割記録とBrowser LocalのLZ4。Webの複数ファイル選択、rosbag2 metadata.yamlの解釈、Recording Serverの分割公開は含まない。Serverの対応圧縮形式も変更しない。

## 調査と選択した構成

- `McapSource`はSummary/Chunk Indexから必要なChunkを選び、重複するChunkを時間順にマージする。Inspector、Health、Plot、Seekの索引処理も同じsourceを使う。
- 各partでchannel/schema IDが再利用されるため、その数字のまま結合すると別topicを誤配送する。
- Native/Web/Serverにはすでに索引必須の方針がある。欠けた索引を全走査で補う必要はない。
- Webには明示的な圧縮形式制限と、WASM用のLZ4展開実装の追加が必要だった。

`viewer-mcap::recording::segments`で各partのSummaryを統合し、内部read用のaddress空間と論理channel IDを作る。実データは元のファイルに残し、必要なChunk/Message Indexをpositional readする。

既存の`McapSource`と`McapPlayback`を使い、partごとのcontroller・時計・query wrapperは増やさない。事前に全ファイルを結合する方法は全走査と追加ストレージを必要とし、別のcomposite playback/query層はSeekと索引cacheを重複実装することになるため採用しなかった。

統合した`McapSource::summary()`は内部addressを含む索引であり、実在する1個のMCAPのSummaryとして外部readerへ渡すものではない。payloadは書き換えず、読み出した`RawMessage`とMessage Indexのchannel IDを対応付ける。`Bytes` backingの共有を維持する。

## Nativeの動作

~~~sh
cargo run -p viewer-native -- --mcap /path/to/bag_directory --layout monitor
~~~

- ディレクトリ直下の拡張子`.mcap`の候補を開く。既知の副ファイル名`preview.mcap`はpart候補から除く。再帰探索やmetadata.yamlによる探索は行わない。ディレクトリのドロップも同じ経路を使う。
- 最大1024part。空ディレクトリ、重複path、非regular file、索引不足のpartをエラーにし、黙って省略しない。
- part順は最初のMessage Index付きChunkのlog time、同じ開始時刻ならcanonical path順。メッセージのないChunkは開始時刻の判定から除く。
- 元のlog timeを維持する。隙間を詰めず、partの重なりを許し、同時刻のメッセージを勝手に重複排除しない。
- 同じtopicはschema名・schema encoding・定義bytes・message encodingが同じ場合に統合する。channel metadataは最初のpartの値を保持する。topicのschema/encoding変更、1part内の同名topicの複数channelは初回実装ではエラーとする。
- 論理channel/schema IDはu16の範囲内。件数をchecked-addし、いずれかのStatisticsが不明なら統合件数も不明とする。
- forwardの同時刻順は論理stream ID、統合Chunk番号、record順。latest-beforeのtieは既存どおり後のChunk/recordを採用する。
- Seekは全partのlatest-before・dynamic TF history・persistent TFを集め、候補source位置とfeature restoreが成功してからcommitする。後半partでの失敗で前半の途中状態を見せない。
- 重複Chunk backing上限256 MiB、query/cancellation予算、1-spanの索引cache、1-ChunkのInspector cacheは全part共通。partごとのpayload cacheを常駐させない。Summary/channel map/ファイルhandleはpartと索引量に応じて増える。
- fingerprintは順序付けた各partのサイズとSummary CRCを集約し、統合した時刻・件数と組み合わせる。path文字列はhashに入れない。同時刻tieの並びが変わればidentityも変わり得る。暗号学的な内容hashではない。
- Previewとbookmarksは指定ディレクトリ内の副ファイルを参照する。親ディレクトリの別記録用ファイルは使わない。既存preview-builderは単一ファイル入力用で、分割記録全体のPreview生成は今回の対象外。Previewなしでもexact Seekは動作する。

## 非インデックス入力

自動の全走査や不完全なSeekへフォールバックしない。Native/WebのSummary/Chunk Index不足エラーに、明示的なreindex/convertが必要な旨を表示する。Message Index不足も全走査で補わない。

[MCAP CLI](https://mcap.dev/guides/cli)の`recover`等で、別ファイルへ索引とSummaryを再生成できる。使用するCLIのhelpと出力を確認する。変換はユーザーが明示的に実行する独立した処理で、入力全体のI/Oを伴う。

~~~sh
mcap recover input.mcap -o indexed.mcap
~~~

壊れた入力の回復ではレコードが失われる場合があるため、CLIの終了結果を確認する。`ros2 bag reindex`によるrosbag metadataの再生成と、MCAP内部のChunk/Message Index生成は区別する。

## Browser LocalのLZ4

`mcap-lz4-rust` featureで`lz4_flex`のframe decoderを使い、Webの許可形式へLZ4を追加する。MCAPの`lz4`はraw blockではなく[frame形式](https://mcap.dev/docs/python/_modules/mcap/writer)。Nativeは既存のC decoderを維持する。

必要なChunkだけを展開し、1個の共有backingを作る。宣言サイズ+1で打ち切り、サイズ不一致を拒否する。純Rust decoderがEOFを許容する箇所は厳密な入力readerで補い、終端不足をエラーにする。Nativeもfinishを確認して同じ切り詰め入力を拒否する。

画像形式は今回変更しない。Raw Image/PNGは既存Cameraのadmission・状態・パネルに入力形式として追加する次段階。深度画像・compressedDepth・動画codecはさらに分けて検討する。

## 検証

- 異なるchannel/schema ID、逆のファイル名順、重なる時刻、同時刻tie、`[start,end)`、exact predecessor、逆方向Seek、統合fingerprint。
- openで2 MiBの無関係なpayloadを読まないこと、再生で無関係なChunkをpruneすること、payload copyが0であること。
- 後半Chunk破損・part変更時のcursor/source/feature commit中止。
- 実fixtureを2partへ分割し、4秒→1秒→5秒のSeekで7カメラ・15静的TF・3動的TF・scanの座標変換を復元すること。
- LZ4の共有backing・exact restore、サイズ不一致・破損header・終端不足。portable backend単独のテストも実行。
- Native encoderで作ったLZ4記録を実ブラウザで開く機能確認。SwiftShaderを使い、性能比較とは区別する。

ヘッドレスのアクセラレートCanvasで透明な画像が残る事例も見つかった。通常ビルド＋ソフトウェアCanvasでは3回成功したが、原因と実GPUでの再現性は未検証。詳細と失敗記録は下のverificationに分けて残す。

結果は[verification](../../verification/2026-10-21-split-mcap/)へ保存する。
