# Web RecordingDataPlane

## Purpose and ownership

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
256 MiB resident-memory limit. The planner converts the real-time reserve into log-time ahead from
the selected playback speed:

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

## Memory and copies

The in-memory window budget is 256 MiB. Oldest windows are evicted after the cursor advances, while
the current window and its immediate successor are protected. Diagnostics expose load/read counts,
source and decompressed bytes, logical payload bytes, unique retained backing bytes, their
retention ratio, per-message copied bytes, window count, target/actual buffer ahead, buffer
underruns, evictions, stale results, load latency, and MCAP Chunk processing/decompression time.

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
