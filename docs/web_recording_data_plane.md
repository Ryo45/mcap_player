# Web RecordingDataPlane

## Purpose and ownership

Web forward playback uses one source-independent path:

```text
Browser File                    Recording Server
  -> BrowserMcapWindowLoader      -> RemoteWindowLoader
              \                    /
               -> SerializedWindow
               -> RecordingDataPlane
               -> PlaybackCore
               -> DomainState
```

`PlaybackCore` owns ROS CDR reduction, camera presentation scheduling, counters, and exact domain
state. It does not know whether bytes came from HTTP, `File.slice()`, an MCAP Chunk, or a cache.
Loaders own asynchronous I/O and publish only complete logical `[start, endExclusive)` windows.
The DataPlane owns the one-second fetch plan, two-second target-ahead policy, completeness horizon,
buffering decision, and in-memory retention.

## Browser local MCAP

Opening a local file drives `mcap::sans_io::SummaryReader` with bounded `File.slice()` reads. It
does not call `File.arrayBuffer()` and does not allocate a file-sized `Vec<u8>` in WASM. A window
uses `IndexedReaderOptions` with topic and `[start, endExclusive)` log-time filters. Each
`ReadChunkRequest` is satisfied by one `File.slice()` of the indexed Chunk payload. Both
uncompressed and Zstd Chunks go through the same `IndexedReader` state machine.

The production file adapter validates range overflow, file bounds, JavaScript's exact integer
limit, and short reads. MCAPs without Summary or Chunk Index records are rejected rather than
silently scanned from the start.

## Remote recording

`RemoteWindowLoader` follows all continuation pages for one logical time window before publishing
it. Batch payloads remain `Bytes` slices of their response body, so converting a complete remote
window to `RawMessage` does not copy individual CDR payloads. HTTP pagination and cursors do not
escape the loader.

## Completeness and buffering

For stored coverage `[start, completeUntil)`, a target is serviceable only when
`target < completeUntil`. The inclusive final recording timestamp is serviceable when the final
window reaches the recording's `endExclusive`. While a requested cursor is unavailable, the
PlaybackClock and DomainState stay committed at their previous values. Empty complete windows
still advance coverage.

Seek, backfill, and TF checkpoints are intentionally absent. The Web timeline is display-only
until those semantics are implemented transactionally for both loader types.

## Memory and copies

The in-memory window budget is 256 MiB. Oldest windows are evicted after the cursor advances, while
the current window and its immediate successor are protected. Diagnostics expose load/read counts,
bytes loaded, resident bytes, window count, buffer ahead, evictions, stale results, load latency,
and MCAP Chunk processing/decompression time.

Copies are:

- Browser `Blob.arrayBuffer()` to the WASM range `Vec<u8>` (one copy per requested range).
- `IndexedReader` compressed input to its decompressed Chunk slot.
- Indexed message data to one owned `Bytes` allocation, required because the reader reuses slots.
- Remote HTTP `ArrayBuffer` to one `Bytes` allocation for the batch body.

After that boundary, `RawMessage` clones share `Bytes`; Camera CDR parsing retains JPEG with
`Bytes::slice()` and does not copy it again. No full-file allocation or JSON/Base64 payload exists.

## Compression toolchain

`viewer-web` enables the `mcap` Zstd feature so `IndexedReader` handles compressed and
uncompressed Chunks without a second compression abstraction. `zstd-sys` supports
`wasm32-unknown-unknown` through its WASM shim but needs a WASM-capable C compiler (normally
Clang) at build time. CI and release-builder images must provide it.
