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
uses Summary `ChunkIndex` entries with the same topic and `[start, endExclusive)` pruning rules as
`IndexedReader`. Each selected Chunk payload is satisfied by one `File.slice()`. The local owned
collector uses the public `ChunkIndex::compressed_data_offset()` and `mcap::parse_record()` APIs;
it exists because `IndexedReader` only lends data from private, reusable decompression slots.
It does not implement whole-file parsing or Summary parsing.

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
source and decompressed bytes, logical payload bytes, unique retained backing bytes, their
retention ratio, per-message copied bytes, window count, buffer ahead, evictions, stale results,
load latency, and MCAP Chunk processing/decompression time.

Copies are:

- Browser `Blob.arrayBuffer()` to the WASM range `Vec<u8>` (one copy per requested range).
- Zstd input to one decompressed `Vec<u8>` backing when the Chunk is compressed.
- Remote HTTP `ArrayBuffer` to one `Bytes` allocation for the batch body.

The range/decompressed `Vec<u8>` moves into `Bytes` without copying. Every retained Local message
is a `Bytes::slice()` of that Chunk backing; every Remote message is a slice of its batch body.
Camera CDR parsing then retains JPEG with another `Bytes::slice()`. No per-message payload copy,
full-file allocation, or JSON/Base64 payload exists. `residentBytes` counts unique Chunk/page
backings pinned by a Window, while `logicalPayloadBytes` sums only visible message payloads. No
automatic compaction is performed when a small payload pins a large backing.

For the first one-second window of the Turtlebot 7-camera recordings (debug build), the owned
collector measured:

| Input | Messages | Reads | Source bytes | Decompressed bytes | Logical bytes | Resident bytes | Per-message copied bytes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Zstd | 181 | 5 | 1,472,278 | 4,029,425 | 3,437,516 | 4,029,425 | 0 |
| Uncompressed | 181 | 5 | 4,029,425 | 0 | 3,437,516 | 4,029,425 | 0 |

The measured debug-build load/Chunk-processing times were 45.05/6.44 ms for Zstd and
42.18/0.75 ms for uncompressed. These values are diagnostic baselines, not performance targets.

## Compression toolchain

`viewer-web` uses the same Zstd implementation already selected by the `mcap` dependency and
decompresses each selected compressed Chunk exactly once into its final shared backing.
`zstd-sys` supports
`wasm32-unknown-unknown` through its WASM shim but needs a WASM-capable C compiler (normally
Clang) at build time. CI and release-builder images must provide it.
