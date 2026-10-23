# Web RecordingDataPlane

## Purpose and ownership

Shared MCAP byte processing and indexed helpers live in `viewer-mcap`, which
depends on the source-neutral `viewer-core`. See
[viewer_mcap_boundary.md](viewer_mcap_boundary.md) for the crate boundary.

The current Web monitor workspace and the remaining Native/Web feature differences
are documented in [native_web_parity.md](native_web_parity.md).

Web forward playback uses one source-independent path:

```text
Browser File                    Recording Server
  -> BrowserMcapWindowLoader      -> RemoteWindowLoader
              \                    /
               -> SerializedWindow
               -> RecordingDataPlane
               -> RawMessage routes
               -> concrete feature controllers
```

Feature controllers own ROS CDR decode, admission, scheduling, counters, and bounded feature state.
They do not know whether bytes came from HTTP, `File.slice()`, an MCAP Chunk, or a cache.
Loaders own asynchronous I/O and publish only complete logical `[start, endExclusive)` windows.
The DataPlane owns `FetchProfile`, fetch planning, the completeness horizon, buffering decisions,
and in-memory retention. Local and Remote loaders use the same planner; loaders do not implement
source-specific prefetch policy.

The default profile keeps one-second logical windows, two seconds of real-time reserve, and a
256 MiB serialized-backing eviction budget. The planner converts the real-time reserve into
log-time ahead from the selected playback speed:

| Playback speed | Target log-time ahead |
| --- | ---: |
| 0.25x | 0.5 s |
| 0.5x | 1 s |
| 1x | 2 s |
| 2x | 4 s |

This keeps approximately two seconds of playback time buffered without changing the loader or
window ownership model. `FetchProfile` can be supplied at DataPlane construction when a source
needs different tuning, but the planning algorithm remains shared. Only one load may be in flight.

Fetch intent is explicit. While playing, `PlaybackAhead` keeps filling the profile target one
window at a time. While paused, `RequiredOnly` fetches the cursor window but does not continue
filling target-ahead. An already in-flight window may finish after Pause because it remains useful,
but completion does not start another request. Local and Remote use these same rules.

## Browser local MCAP

Opening a local file drives `mcap::sans_io::SummaryReader` with bounded `File.slice()` reads. It
does not call `File.arrayBuffer()` and does not allocate a file-sized `Vec<u8>` in WASM. A window
uses Summary `ChunkIndex` entries with the same topic and `[start, endExclusive)` pruning rules as
`IndexedReader`. In the owned-backing path, each selected Chunk payload is satisfied by one `File.slice()`. The local owned
collector uses the public `ChunkIndex::compressed_data_offset()` and `mcap::parse_record()` APIs;
it exists because `IndexedReader` only lends data from private, reusable decompression slots.
It does not implement whole-file parsing or Summary parsing.

Chunks larger than 64 MiB (uncompressed size) use a different physical read path
for Zstd and uncompressed data. With complete Message Index coverage for the
selected channels, the loader resolves the requested window's physical record
positions. Uncompressed messages are read directly at those offsets. Zstd uses
`viewer_mcap::McapChunkStream`: bounded input, at most 128 KiB of expanded output
per step, and a cooperative browser yield after approximately 8 ms of work.
This still runs on the main thread; it does not add a Worker or another WASM heap.

One physical Chunk continuation retains the decoder, up to 4 MiB of unread input,
an incomplete record, and bounded Message Index metadata. It continues into the
next logical window without re-expanding the same prefix. If the next requested
window needs an earlier physical position (grouped/out-of-order data or seek),
it rewinds instead of assuming that record order equals log-time order. Without
complete selected-channel Message Index coverage, it streams the selected Chunk
to completion. It never falls back to a whole-file scan.

Large streamed Zstd Chunks copy only selected in-window CDR payloads. This costs
memory bandwidth but avoids keeping a whole giant Chunk alive for a few messages.
Small Chunks and LZ4 keep the shared owned-backing path. Each streamed record is
limited to 256 MiB, selected window payload/message admission to 1 GiB/200,000,
the Zstd decoder window to 128 MiB, and physical Chunk input/output work to 64 GiB
each. These are separate bounds; they are not a total process-memory allowance.
Chunk lengths, file offsets and cumulative I/O/decompression counters remain u64
on WASM32. An individual retained payload must still fit the address space.

Cancellation is checked after asynchronous reads and between decode steps.
Results remain staged until all required indexed positions have been found and
matched to their channel/log time. A retired generation cannot publish its
window or return a continuation into the current source. Prefix streaming relies
on the Message Index; it does not prevalidate the CRC of unread Chunk suffixes.

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
PlaybackClock and controller state stay committed at their previous values. Empty complete windows
still advance coverage.

Diagnostics report the speed-adjusted target ahead, actual buffer ahead at the committed cursor,
and buffer-underrun count. An underrun is counted once when playing cannot advance; initial paused
loading is not an underrun, and repeated ticks during the same underrun do not inflate the count.

## Seek lifecycle

The Web timeline performs a source-independent transactional seek:

```text
seek intent
  -> cancel old loader generation
  -> keep committed Clock and controller state visible while loading
  -> resolve latest-before/history/persistent inputs through the source-specific restore loader
  -> only after success, discard/rebase playback windows at target
  -> atomically reset/replay exact state and commit Clock
  -> resume PlaybackAhead only when playback is running
```

Rapid replacement seeks cancel Remote HTTP with `AbortController`; both Remote and Browser-file
loaders increment generation so a result that finishes after cancellation cannot enter the Store.
Partial continuation pages never reach the DataPlane. The seek window is fetched even while paused,
but it does not trigger unrelated target-ahead fetches after completion.

Camera/Path/Odometry/Scan use MCAP Message Index predecessor lookup (`latest log_time <= target`),
not a message-count lookback heuristic. Dynamic TF restores the same one-second history that normal
playback retains. Static TF is explicitly persistent: Browser Local reads its indexed archive and
Remote receives the complete archive from `/restore`; the Web session keeps it once and filters
updates by each target. Missing Message Index support is an explicit restore error, never a hidden
prefix scan. The committed presentation is invalidated only after all restore data has loaded and
`PlaybackEffect::Seeked` is emitted.

Large uncompressed Chunks also restore directly from indexed Message records;
the restore work allowance counts the records actually read, not the declared
size of the whole Chunk. The existing 256 MiB indexed-restore allowance remains.
Large compressed seek/restore still uses the owned backing and can exceed that
allowance; streaming forward playback does not imply equally fast compressed seek.

## Memory and copies

The in-memory window budget is 256 MiB. Budget enforcement on load completion evicts older
windows without discarding current or future coverage. This is an eviction budget, not a strict
allocation or process-memory limit: required windows, Chunk overlap, feature-owned `Bytes`,
loader temporaries and the allocator can retain additional memory. Prefetch remains time-based.

Fully consumed windows are also retired when their messages are delivered, even when no further
load completes. Delivery of the final inclusive timestamp retires the final window. Returned
messages retain their `Bytes`, and feature state may keep backing allocations alive; retirement
does not guarantee an immediate RSS decrease or WASM linear-memory shrink. The exclusive
completeness horizon remains monotonic, so subsequent windows remain adjacent and delivery does
not repeat consumed messages. Diagnostics expose load/read counts,
source and decompressed bytes, logical payload bytes, unique retained backing bytes, their
retention ratio, per-message copied bytes, window count, target/actual buffer ahead, buffer
underruns, evictions, stale results, load latency, and MCAP Chunk processing/decompression time.

Copies are:

- Browser `Blob.arrayBuffer()` to the WASM range `Vec<u8>` (one copy per requested range).
- Zstd or LZ4 input to one decompressed `Vec<u8>` backing when the Chunk is compressed.
- Remote HTTP `ArrayBuffer` to one `Bytes` allocation for the batch body.

For the owned-backing path, the range/decompressed `Vec<u8>` moves into `Bytes` without copying.
Those Local messages are `Bytes::slice()` values of that Chunk backing; Remote messages are
slices of their batch body. The large streamed-Zstd path instead copies admitted payloads
out of the reusable record buffer; direct uncompressed reads retain only one Message body.
Camera CDR parsing then retains JPEG with another `Bytes::slice()`. Payload copies on the
streamed path are included in `perMessageCopiedBytes`. No full-file allocation or
JSON/Base64 payload exists in the recording data plane. Camera pixel
presentation makes bounded compact JPEG copies for Web Workers separately; see
[Browser JPEG presentation](web_jpeg.md). `residentBytes` counts unique Chunk/page
backings pinned by a Window, while `logicalPayloadBytes` sums only visible message payloads. No
automatic compaction is performed in the small-Chunk owned-backing path; the large-Chunk
path avoids that retention explicitly as described above.

For the first one-second window of the Turtlebot 7-camera recordings (debug build), the owned
collector measured:

| Input | Messages | Reads | Source bytes | Decompressed bytes | Logical bytes | Resident bytes | Per-message copied bytes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Zstd | 181 | 5 | 1,472,278 | 4,029,425 | 3,437,516 | 4,029,425 | 0 |
| Uncompressed | 181 | 5 | 4,029,425 | 0 | 3,437,516 | 4,029,425 | 0 |

The measured debug-build load/Chunk-processing times were 45.05/6.44 ms for Zstd and
42.18/0.75 ms for uncompressed. These values are diagnostic baselines, not performance targets.

## Compression toolchain

Browser Local also supports LZ4 through `viewer-mcap`'s `mcap-lz4-rust` feature
and the portable `lz4_flex` frame decoder. It uses the same bounded shared-backing
window and restore paths as small Zstd Chunks. Truncated frames and declared-size mismatches
are errors; the native LZ4 C dependency is not needed by the Browser build.

`viewer-web` uses the same Zstd implementation already selected by the `mcap` dependency and
bulk-decodes small Chunks into shared backing. Large Zstd Chunks use the incremental
decoder continuation described above.
`zstd-sys` supports
`wasm32-unknown-unknown` through its WASM shim but needs a WASM-capable C compiler (normally
Clang) at build time. CI and release-builder images must provide it.

## Chunk processing threads and large-Chunk limits

Browser Local uses `spawn_local` for async File I/O, then synchronously decompresses and parses
Chunks on the browser main thread. It does not use the JPEG Web Workers for MCAP processing.
Remote MCAP processing runs in the Recording Server's bounded `spawn_blocking` jobs; the browser
receives serialized messages rather than decompressing MCAP Chunks itself.

The Local forward loader currently lacks an explicit per-Chunk compressed/uncompressed byte cap
like Native's 256 MiB active-backing bound. Its window eviction budget does not prevent a large
individual read or bulk decompression allocation. Seek/restore has separate work budgets.
The [1 GiB Chunk experiment](performance/2026-10-16-web-gib-chunk/README.md) was stopped by an
independent Chrome PSS guard before normal playback could be verified. Moving processing into
a Worker addresses main-thread blocking, but does not by itself reduce retained decompressed
backing or the input/output allocations. No such worker change has been made yet.
