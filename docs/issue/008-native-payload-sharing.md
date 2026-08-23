# Native McapSourceでRawMessage payload backingを共有する

- Priority: P0
- 規模: M
- 状態: 完了

## 背景・課題

Nativeは`Summary::stream_chunk`が返す`message.data.into_owned()`をmessageごとに`Bytes`へ変換します。
compressed Chunkを展開した後、selected Camera JPEG等をmessage単位で再copyしています。Browser Localは
既にowned decompressed Chunk backingから`Bytes::slice`しており、platform間でownershipが異なります。

## 解決案

- Native source backingを`Bytes` ownerとして保持し、mmap lifetimeをclone可能なbackingへ結び付ける。
- uncompressed Chunkはmmap slice、compressed Chunkは一回だけdecompressした`Bytes`を使う。
- Chunk recordを走査し、RawMessage payloadをbackingのrange sliceとして作る。
- sequential、latest-before、history/persistentで同じChunk parserを使う。
- generic pool、Source trait、universal backing abstractionは作らない。

## Structural test

- 同じChunk内の複数RawMessage payload pointerが一つのbacking range内にある。
- zstd/lz4はdecompressed Chunk一allocationを共有する。
- Camera CDR payloadから切ったJPEGも同じallocation rangeにある。
- per-message copied bytesは0。

## 完了条件

- Native source pathに`message.data.into_owned()`がない。
- selected high-bandwidth messageごとのpayload copyがない。
- Chunk/cache/window lifetimeを越えて必要なBytes sliceだけがbackingを保持する。

## 実装結果

- Native mmapを`Bytes::from_owner`でsource backingにし、uncompressed Chunkはmmap sliceを直接共有する。
- zstd/lz4 Chunkは一回だけ展開し、Chunk record parserが各message payloadを同じ`Bytes`からsliceする。
- sequential/latest-before/history/persistentは同じparserを使用し、`message.data.into_owned()`経路を削除した。
- uncompressed source owner共有、zstd decompressed backing共有、indexed readの
  `per_message_copied_bytes == 0`、Camera CDR→JPEG slice共有をstructural testで固定した。
