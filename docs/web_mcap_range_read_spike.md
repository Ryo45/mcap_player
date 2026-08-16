# Web MCAP range-read technical spike

> **Historical spike report.** References to Pipeline/DomainState below describe the experiment at
> that time, not the current viewer architecture. Current production ownership and restore
> semantics are documented in `docs/message_centric_architecture.md` and
> `docs/web_recording_data_plane.md`.

## 1. 目的

Browser `File.slice()`でMCAPのFooter、Summary、代表Chunkだけを取得できるかを確認し、production readerを設計する前に、現在使用している`mcap 0.23.4`の低レベルAPIと非同期境界を評価する。

この実装は診断用スパイクであり、`RangeSource`、`AsyncReadAt`、`LogicalRecording`などのproduction APIではない。

## 2. 調査開始時の全ファイル読み込み経路

このSpike開始時のWeb walking skeletonは次の経路を使っていた。

```text
File.arrayBuffer()
  → JavaScript ArrayBuffer（全ファイル）
  → Uint8Array.to_vec()（WASM heapへ全ファイルをコピー）
  → McapPlayback<Vec<u8>>
  → McapSource::new()
  → Summary::read(&[u8])
```

`Summary::read()`は渡されたslice全体を一つのMCAP fileとして扱い、その長さをfile sizeとしてFooterへseekする。このため、Summary部分だけを切り出して渡すことはできない。また、`McapSource<B: AsRef<[u8]>>`も全体sliceの存在を前提とする。

この経路は後続の`BrowserMcapWindowLoader`で撤去済みである。現在のproduction Local
playbackは`File.slice()`、`SummaryReader`、Summaryの`ChunkIndex`とowned backing collectorから`SerializedWindow`を作り、
Remoteと同じ`RecordingDataPlane`へ渡す。詳細は
[`web_recording_data_plane.md`](web_recording_data_plane.md)を参照。

## 3. 実装したrange-read経路

通常再生とは別の折り畳み式「Range Read Spike」UIと、Browser専用の具象adapterを追加した。

```text
File.size
  → File.slice(file_size - 37, file_size)
  → Footer parse
  → File.slice(summary_start, footer_start)
  → LinearReader::sans_magic(summary bytes)
  → SummaryCatalog表示
  → File.slice(first_chunk.offset, first_chunk.end)
  → Chunk record header parse

  → SummaryReader event loop
      SeekRequest → File.sliceのabsolute position更新
      ReadRequest → File.slice（summary中は256 KiB read-ahead）
  → IndexedReader(topic + arbitrary log time)
      ReadChunkRequest { absolute offset, payload length }
  → File.slice(chunk payload only)
  → IndexedReader::insert_chunk_record_data()
  → RawMessage
  → existing PipelineSet
  → DomainState
```

Spike経路は`File.arrayBuffer()`を呼ばない。各`File.slice()`が返した`Blob`にだけ`Blob.arrayBuffer()`を呼ぶ。

Browser adapterは次を検証する。

- `offset + length`のoverflow
- file size外のrange
- JavaScript safe integer範囲
- empty range（I/Oを行わず空byte列を返す）
- `Uint8Array`で表現できないrange length
- `Blob.slice()`／`Blob.arrayBuffer()`のerror
- requested lengthとactual lengthの一致

I/O adapterは`apps/viewer-web`内だけにあり、shared coreへtraitを追加していない。ファイル選択ごとにgenerationを更新し、各`File.slice()`の前後で照合する。新しい選択後に完了した古いseekはUIにも`DomainState`にも適用しない。

## 4. MCAP Footer／Summary取得結果

Footer recordの形式は`mcap 0.23.4`のsourceとMCAP record型から確認した。

```text
Footer tail: 37 bytes
  opcode                  1 byte  (0x02)
  record body length      8 bytes (little endian, value 20)
  summary_start           8 bytes (little endian)
  summary_offset_start    8 bytes (little endian)
  summary_crc             4 bytes (little endian)
  trailing magic          8 bytes
```

record framingとtrailing magicはSpike側で検証し、Footer body自体は公開`mcap::parse_record()`へ渡す。独自Footer decoderは作っていない。`mcap::read::footer()`は公開されているが、先頭magicと末尾magicを含む完全なfile sliceを要求するため、このtail-only経路には使用できない。

指定ログ：

```text
mcap/turtlebot3_7cam_fhd/turtlebot3_7cam_fhd_0.mcap
```

取得結果：

```text
File size                   596,121,452 bytes
Footer range                596,121,415..596,121,452 (37 bytes)
Summary range               595,222,093..596,121,415 (899,322 bytes)
Summary offset start        596,121,285
Summary CRC                 0x9cc5ea08
Schema count                9
Channel count               18
Chunk index count           3,517
Attachment index count      0
Metadata index count        2
Summary offset count        5
Message indexes             present
Message time range          1785591563485080407..1785592463224014194 ns
Compression                 zstd: 3,517 chunks
```

Summary rangeはSchema、Channel、Statistics、Chunk Index、Attachment Index、Metadata Index、Summary Offsetの連続record列として`LinearReader::sans_magic()`で解析できた。

## 5. 現在の`mcap` crateで再利用できたAPI

確認対象はCargo.lockで確定した`mcap 0.23.4`である。

| API | 結果 |
| --- | --- |
| `mcap::parse_record()` | 任意record bodyからFooter、Chunk、Message Indexなどを個別parse可能 |
| `mcap::read::LinearReader::sans_magic()` | magicを含まない任意の連続record rangeを反復可能。SpikeのSummary parseで使用 |
| `mcap::read::ChunkReader` | Chunk headerとpayloadを独立して受け取り、未圧縮Chunkをrecord反復可能 |
| `mcap::sans_io::SummaryReader` | 公開API。`ReadRequest`／`SeekRequest` eventでI/Oをcallerへ外出しでき、async Browser adapterに適合する |
| `mcap::sans_io::IndexedReader` | 公開API。Summaryとfilterから`ReadChunkRequest { offset, length }`を生成し、取得したcompressed chunk dataを挿入できる |
| `ChunkIndex::compressed_data_offset()` | Chunk record全体ではなくcompressed payloadだけのrangeを計算可能 |
| `mcap::read::footer()` | 公開。ただし完全file slice用であり、tail-only Spikeでは不使用 |

`SummaryReader`と`IndexedReader`はtraitを追加せず、I/OをBrowser側へ残したままshared同期state machineを再利用できる可能性が高い。

非圧縮ログで実際にこの2つを接続した結果、`IndexedReader`の`offset`はChunk record先頭ではなく`ChunkIndex::compressed_data_offset()`と一致するpayload先頭の絶対file offsetであり、`length`は`compressed_size`（非圧縮時はuncompressed sizeと同値）と一致した。local `Vec<u8>`はoffset 0からpayloadを保持したまま、元の絶対offsetと一緒に`insert_chunk_record_data()`へ渡すのが正しい。

なお`IndexedReader`はMessage Index recordそのものを追加range readしない。Summary内の`ChunkIndex.message_index_offsets`をtopic単位のChunk pruningに使い、取得・展開したChunk内を走査して内部message indexを構築する。小さい2-topic fixtureでは対象channelを含まないChunkがrequestされないことを確認した。

## 6. 再利用できなかったAPI

- `Summary::read(&[u8])`は完全file sliceと絶対file positionを要求する。Summary rangeだけには使用できない。
- `Summary::stream_chunk(mcap, index)`は`chunk_start_offset`を完全file sliceへ適用するため、取得したChunk range単体には使用できない。
- `Summary::read_message_indexes(mcap, index)`もMessage Indexの絶対offsetを完全file sliceへ適用する。
- built-in LZ4／Zstd decompressor moduleはprivateであり、単独decompressorとして直接生成する公開APIではない。公開`ChunkReader`／`IndexedReader`経由で使う設計になっている。
- sync `Read + Seek`を保持する汎用reader facadeは中心APIではない。`Summary::read()`の内部adapterと、I/Oを持たないsans-I/O readerが提供されている。
- `McapSource`／`McapPlayback`は全体`AsRef<[u8]>`前提のため、そのままBrowser range readerへ置換できない。

Message Index record自体は`parse_record(op::MESSAGE_INDEX, body)`または`LinearReader::sans_magic()`で独立parseできる。ただしSummaryに記録された絶対offsetを、取得したlocal buffer offsetへ対応付ける責務がadapter側に必要になる。

## 7. 圧縮chunkの状況

Spike実施時のworkspace共通依存は次の設定だった。

```toml
mcap = { version = "0.23.4", default-features = false }
```

後続実装で`viewer-web`は`zstd`を有効化した。production Local loaderは、message保持時の
再コピーを避けるため、SummaryのChunk Index、公開`mcap::parse_record()`、owned Chunk backingを
使用する。Spikeの`IndexedReader`検証結果はChunk選択・offset semanticsの基準として維持する。

指定ログの3,517 ChunkはすべてZstdだった。Spikeは最初のChunk record rangeを取得し、公開`parse_record()`でheaderを検証した。

```text
Selected chunk offset       565
Chunk record length         329,409 bytes
Compression                 zstd
Compressed payload          329,356 bytes
Uncompressed size           820,971 bytes
```

Spike当時のWeb feature構成では展開を試みず、次を成功結果として表示した。

```text
Footer and Summary range access succeeded
Selected chunk range access succeeded
Chunk parsing/decompression requires a different boundary
```

`mcap` crateはWASM target向けZstd featureとLZ4 wasm shimを含む。両featureをそれぞれ一時的に有効化してWASM checkを行ったが、この環境にはWASM用C compilerの`clang`がなく、`zstd-sys`と`lz4-sys`のbuild-scriptで停止した。そのため圧縮feature有効時の成果物サイズは未計測である。

現行feature無効のTrunk debug buildに含まれるWASMは3,099,447 bytesだった。圧縮をproductionで有効化する場合は、CIへclangを追加してrelease＋wasm-opt後のサイズと展開速度を別途比較する必要がある。

比較のため、Spike専用`mcap-uncompress-spike` binで全566,775 messages、9 schemas、18 channels、2 metadataを非圧縮Chunkへ再書き込みした。元ファイルは保持し、次の別ファイルを生成した。

```text
mcap/turtlebot3_7cam_fhd/turtlebot3_7cam_fhd_0_uncompressed.mcap
```

生成物は2,860,282,594 bytesで、元のZstd版596,121,452 bytesの約4.80倍だった。全3,517 Chunkが非圧縮であることと、Summaryのmessage/schema/channel/metadata/attachment件数を変換ツール内で検証した。

Browser `DecompressionStream`の利用は必須ではない。2026年時点でZstd対応には実装差があり、LZ4はMCAP用のportableな標準経路にならない。MCAPのcompression名、uncompressed size、CRC、record parsingとの接続を一箇所に保つには、`mcap::sans_io::IndexedReader`が扱うRust/WASM展開を第一候補とする。ただしWebAssembly成果物サイズとの比較後に確定する。

参考：

- [WHATWG Compression Streams](https://compression.spec.whatwg.org/)
- [MDN DecompressionStream constructor](https://developer.mozilla.org/en-US/docs/Web/API/DecompressionStream/DecompressionStream)

## 8. WASM上の制約

- Browser `File.size`と`Blob.slice`のoffsetはJavaScript numberで渡されるため、2^53−1を超えるoffsetを拒否する。
- `Uint8Array`化する単一rangeは実装上`u32::MAX`以下に制限した。productionではChunk size limitをさらに小さく設定すべきである。
- `Blob.arrayBuffer()`から`Uint8Array::to_vec()`でWASM heapへrangeごとのコピーが1回発生する。全ファイル二重保持は解消するが、zero-copyではない。
- Browser APIはasync、`parse_record`／`LinearReader`／`IndexedReader`は同期state machineである。
- 診断処理はfile/request generationを持ち、`File.slice()` awaitの前後で古いgenerationを破棄する。これはAbortSignalによる通信中断ではなく、stale resultの適用防止だけである。
- multi-segment／HTTP RangeはこのSpikeでは扱わない。

## 9. 計測結果

計測は2026-08-01にlocal filesystem上の指定ログへ、同じrangeとparserを使用するignored testで実施した。OS cacheやBrowser実装により時間は変動するため、byte数を主要結果、時間を参考値とする。

```text
File size                         596,121,452 bytes
Bytes fetched before catalog         899,359 bytes
Catalog read ratio                 0.150868417%
Range reads before catalog         2
Range reads including one Chunk    3
Total bytes including one Chunk    1,228,768 bytes

Footer read                        0.024 ms
Summary read                       0.471 ms
Summary parse                     75.793 ms
Selected Chunk read                2.017 ms
Selected Chunk header parse        0.027 ms
Selected Chunk decompress          not attempted (Zstd feature disabled)
```

別のcold-cache相当runではFooter 3.489 ms、Summary read 86.182 ms、Summary parse 33.374 ms、Chunk read 33.081 msだった。Browser UIは実際の`File.slice()`ごとの値を表示する。

非圧縮版を同じrange Spikeで再計測した結果：

```text
File size                       2,860,282,594 bytes
Bytes fetched before catalog         885,291 bytes
Catalog read ratio                 0.030951173%
Range reads before catalog         2
Range reads including one Chunk    3
Total bytes including one Chunk    1,706,311 bytes

Footer read                        0.036 ms
Summary read                       0.629 ms
Summary parse                     65.459 ms
Selected Chunk read                0.546 ms
Selected Chunk parse              10.447 ms

Selected Chunk record length      821,020 bytes
Compressed/uncompressed payload   820,971 / 820,971 bytes
Parsed records                    188
Parsed messages                   161
```

比較すると、非圧縮化によってCatalog取得量はほぼ変わらず、圧縮featureなしでも代表Chunkを最後までparseできた。一方でファイル全体は約4.80倍、代表Chunkのrange取得量は329,409 bytesから821,020 bytesへ約2.49倍になった。したがって非圧縮化はrange-read readerの機能検証と一時的なWeb互換策には有効だが、production配布形式としては転送量・保存量の代償が大きい。

非圧縮版に対する`SummaryReader → IndexedReader → PipelineSet → DomainState`の実測：

```text
SummaryReader range reads          5
SummaryReader bytes                885,328 bytes
Target topic                       /odom
Arbitrary target time              1785592013354547300 ns
First matching message             1785592013374517453 ns
IndexedReader Chunk reads          1
Chunk record start                 1,503,497,376
Requested payload offset           1,503,497,425
Requested payload length           828,513 bytes
Message Index length               3,183 bytes
Pipeline result                    Odometry → DomainState.telemetry
```

`SummaryReader`の5 readはFooterの37 bytesと、256 KiB read-aheadによるSummary 4 readである。`IndexedReader`のrequestはfile外へ出ず、対応Chunk Indexのpayload offset・sizeと一致した。返されたmessageはtarget以上かつ`/odom`だけで、既存`PipelineSet`を変更せず`DomainState`へ適用できた。

## 10. async境界の比較

| 観点 | 案A: Browser async adapter＋同期parser | 案B: shared async RangeReader | 案C: Browser cache/prefetch＋同期parser |
| --- | --- | --- | --- |
| Native mmap共通化 | parser/state machineのみ共通。Native mmapは維持 | Nativeにもasync抽象が波及しやすい | parserは共通、cacheはWeb固有 |
| Web API適合 | `File.slice()` awaitをadapterへ閉じ込められる | API形状は合うがshared全体がasync化 | Blob/HTTPをcacheへ隠せる |
| shared coreへのasync伝播 | 小さい | 大きい | 小さい |
| seek | Summary/IndexedReader eventを必要rangeへ変換 | trait越しに直接await | cache hit時は同期、miss継続設計が必要 |
| chunk prefetch | adapter coordinatorを後付け | reader側へ組み込みやすい | 最も扱いやすい |
| cancellation | Browser task generationで実装可能 | trait/API全体に設計が必要 | coordinatorで集中管理可能 |
| testability | parserとrange計画を分離しやすい | async mockが必要 | cache state machine testが必要 |
| WASM/JS copy | rangeごとに1 copy | 同じ | cache表現次第。copy自体は残る |
| multi-segment | 後からadapter拡張 | traitに早期固定される | coordinatorで扱いやすい |
| HTTP Range | File adapterと並ぶ別adapterが必要 | trait実装として追加 | source別fetchをcoordinatorへ置ける |
| complexity | 最小 | 最大 | 中〜大 |

## 11. 推奨するproduction architecture

現時点の推奨は案Aである。ただし新しい汎用traitを先に作らず、`mcap 0.23.4`のsans-I/O state machineを境界にする。

```text
Browser File adapter
  async Blob.slice(offset, length)
  generation/cancellation
        ↓ requested bytes
mcap::sans_io::SummaryReader / IndexedReader
  synchronous state machine
  emits SeekRequest / ReadRequest / ReadChunkRequest
        ↓ records/messages
Web playback coordinator
```

理由：

1. `SummaryReader`と`IndexedReader`がすでにI/O非依存の要求eventを公開している。
2. Native mmap経路をasync化する必要がない。
3. Browser固有のPromise、File、AbortSignalをshared coreへ漏らさない。
4. seekに必要なChunk rangeが`IndexedReader`から直接得られる。
5. 実装経験が得られてから案Cのcache/prefetchを追加できる。

非圧縮版によって案Aのreader全体をZstd backendから切り離して検証できた。Zstdはproduction必須だが、Source／Playback境界を決める前提条件ではなく、`IndexedReader::insert_chunk_record_data()`内部のdecode backendとして後から有効化できる。

案Bのshared async traitは、File、HTTP、multi-segmentの要件が揃う前に契約を固定するため、現段階では採用しない。

## 12. 未解決事項

- clangを含むCIでLZ4／ZstdのWASM build、runtime展開、成果物サイズを比較する。
- 実Browserで大容量非圧縮ファイルを選び、今回実装したevent loopのFile API時間とpeak memoryを計測する（WASM buildとlocal file testは完了）。
- generationはstale適用を防ぐが通信を中止しないため、productionのcancellation／同時range request方針を決める。
- Chunk cache上限と解凍済みbuffer上限を決める。
- SummaryがないMCAPのfallbackを決める。全ファイルlinear scanは大容量Webでは自動実行しない方がよい。
- CRC検証をどの段階で必須にするか決める。
- Browser実測でJS/WASM copy時間とpeak memoryを計測する。

## 13. 次の実装ステップ

1. Preview共通契約を定義する。
2. preview sidecar reader／writerを実装する。
3. Native Preview → Exactを接続する。
4. Web Workspaceを実装する。
5. clang付きCIでZstdのWASM build、runtime展開、release成果物サイズを検証する。
6. Web Exact playbackへZstdを接続する。
7. 実装経験を基にcache／prefetch／cancellationをproduction化する。

`SummaryReader → IndexedReader → PipelineSet → DomainState`の非圧縮end-to-end検証は完了したため、次はPreview共通契約へ進める。production用`RangeSource`、独自compression abstraction、汎用async traitはまだ作らない。
