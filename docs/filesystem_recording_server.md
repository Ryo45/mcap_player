# Filesystem Recording Server

## Purpose and boundary

`recording-server` exposes explicitly configured MCAP files to a Browser Viewer on a trusted LAN.
It sees only an absolute, seekable filesystem path. Local SSD, NAS, NFS/SMB, FUSE, and a mount in a
cloud VM therefore use the same code path. It neither detects the mount technology nor accepts a
path or URL from a client.

The server owns immutable startup catalogs, open file handles, request limits, CORS policy, and
bounded blocking work. It does not construct feature state, run Viewer controllers, decode ROS
messages, or transform camera data. `RemoteWindowLoader` turns returned CDR frames into exact
`RawMessage` values which are routed to the same controllers as Browser-local playback.

## Configuration and operation

Copy `config/recording-server.toml.example`, replace every recording path with an absolute path,
and list every Browser origin explicitly:

```bash
cargo run -p recording-server -- \
  --config config/recording-server.toml
```

Configuration can be checked without binding a socket:

```bash
cargo run -p recording-server -- \
  --config config/recording-server.toml \
  --validate-only
```

Startup is fail-fast: a malformed configuration, unreadable file, missing Summary/Chunk Index,
unsupported compression, or malformed Catalog stops the process. Recording IDs match
`^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$`; `..`, separators, and client-provided paths are rejected.
The configured file must remain immutable for the server lifetime. Replace a file atomically and
restart the server to publish a new revision; hot reload and file watching are intentionally absent.

## Positional reads and MCAP support

The production path does not use mmap, `fs::read`, or `read_to_end`. `FileRangeReader` performs
bounded positional reads (`FileExt::read_at` on Unix), so concurrent requests never share a mutable
seek cursor. `mcap 0.23.4` public sans-I/O APIs drive all parsing:

```text
SummaryReader → Footer/Summary ranges → immutable Catalog
IndexedReader → selected compressed Chunk ranges → CDR message frames
```

`SummaryReader` supplies the requested seek and length. `IndexedReader` supplies the compressed
Chunk data offset and length and performs `[start,end)` filtering, topic filtering, log-time
ordering, and decompression. No independent MCAP parser exists in the server.

MVP input requires a Summary, Statistics, and Chunk Index. Zstd and uncompressed Chunks are
supported. LZ4, summary-less/unindexed files, split MCAP, rosbag2 directories, attachments, and
live recordings return a clear startup or `422` error; the server never silently falls back to a
full-file linear scan. `max_chunk_bytes` bounds both compressed and uncompressed Chunk size.

## Catalog and revision

Endpoints:

```text
GET /healthz
GET /readyz
GET /v1/recordings
GET /v1/recordings/{recording_id}/catalog
GET /v1/recordings/{recording_id}/messages
GET /v1/recordings/{recording_id}/restore
```

`/restore` is the dedicated playback-seek primitive. It accepts latest-before, bounded history,
and explicitly persistent stream sets. Latest-before uses reverse indexed traversal; a recording
without Message Index records returns `422 restore_index_unavailable`. Persistent streams return
their complete expected-small archive so a Browser session can cache it once and replay only
updates valid at its target. Restore remains separate from the paginated exact range API.

Catalog time is MCAP log time with start-inclusive/end-exclusive semantics. Nanosecond values are
decimal JSON strings, avoiding JavaScript number precision loss. Only `message_encoding == "cdr"`
channels are exposed. Channels are sorted by topic, schema name, message encoding, and original
channel ID, then assigned logical stream IDs starting at one. A stream currently has one
representation, `ros2-cdr`; optimized representations can later receive separate stream IDs
without changing Batch v1.

The revision is:

```text
mcap-summary-identity-v1:<file-size>:<summary-crc>:<start>:<end>:<messages>:<schemas>:<channels>:<chunks>
```

The canonical helper is shared with Preview sidecar validation. It excludes path, mtime, and inode.
It detects obvious sidecar/catalog mix-ups but is not a cryptographic integrity guarantee.

## Binary message batch v1

Responses use `application/vnd.autonomous-viewer.batch; version=1`. All integers are little-endian.
The 16-byte `AVBT` header contains version and message count. Each frame contains:

```text
stream_id u32
sequence u32
log_time_ns u64
publish_time_ns u64
payload_length u32
payload bytes
```

Payload is the original MCAP Message CDR bytes. The shared `viewer-remote-protocol` encoder and
borrowed decoder validate magic, version, lengths, truncation, trailing data, and message count.
Messages are ordered by log time, then logical stream ID, then stable source order. To honor the
stream-ID tie break, only a same-timestamp group is staged before encoding.

## Continuation and limits

The messages query requires the current revision, one or more stream IDs, and a time window. The
server intersects that window with the recording. Soft byte/message limits return a partial batch
and:

```text
X-AV-Batch-Complete: false
X-AV-Next-Cursor: <opaque cursor>
```

The stateless cursor binds version, recording ID/revision, original time range, normalized stream
set, and next result ordinal. Its checksum detects accidental/tampered encodings; it is not an
authentication token. Reusing it with another query returns `400`, and a stale revision returns
`409`. Re-evaluating the bounded query from its start is deliberately favored over server-side
session state in this MVP. Tests verify no page duplicates or gaps, including equal-time results.

Hard limits cover window duration, response bytes, message count, Chunk size, and concurrent
blocking work. One frame may exceed a requested soft byte limit if it fits the hard response limit.
A frame larger than the hard response limit returns `413`; frame size is checked before response
allocation.

## Async, allocation, and metrics

Axum validates requests and acquires a bounded semaphore. Filesystem reads, Chunk decompression,
and MCAP parsing run in `spawn_blocking`; the request awaits that job and no detached work survives
it. The server keeps Catalog/Summary/channel maps/open handles, but no compressed or decompressed
Chunk cache.

Allowed copies are filesystem range to a range buffer, decompressed Chunk storage internal to
`IndexedReader`, a bounded same-time ordering buffer, and selected CDR bytes into the HTTP batch.
There is no full-file allocation, JSON Base64, Camera extraction, or domain-model allocation.

Structured `tracing` events include request ID, recording/revision, time and stream selection,
storage calls/bytes, Chunk count and decompression time, filter/encode time, response bytes/message
count, completion, total time, and status. Catalog startup logs include the resolved path, file
size, revision, range, stream count, Chunk count, and actual Catalog read bytes. Cache metrics are
absent because this version has no cache; LAN/NAS/FUSE measurements should justify a later LRU keyed
by recording ID, revision, and Chunk offset.

The local Zstd smoke recording (`596,121,452` bytes, 3,517 Chunks) required 899,433 bytes and
7,109 small parser-driven reads to build its Catalog (about 0.151% of the file). A one-second
`/odom` request read one 329,356-byte compressed Chunk and returned a five-message, 3,776-byte
batch in about 2.6 ms on the development machine. The uncompressed 2,860,282,594-byte variant also
validated using 885,365 Catalog bytes. These are development measurements, not NAS/FUSE claims;
NAS and FUSE smoke/performance tests remain environment-dependent follow-up work. In particular,
the number of small Summary reads is a candidate for bounded startup readahead if mount latency is
material.

## CORS, deployment, and security floor

Only configured origins and `GET`/`OPTIONS` are allowed. Revision, completion, cursor, and message
count headers are exposed to browsers. Wildcard origins are rejected. The service has no auth or
TLS and must not be exposed directly to the public Internet. Put authentication/TLS in a trusted
reverse proxy before any broader deployment. The API exposes no directory listing, arbitrary path,
or arbitrary URL, and error bodies never contain internal paths.

LAN smoke test from another machine:

```bash
curl http://SERVER_IP:8081/healthz
curl http://SERVER_IP:8081/v1/recordings
curl http://SERVER_IP:8081/v1/recordings/turtlebot-demo/catalog
curl -D /tmp/batch.headers -o /tmp/batch.bin \
  "http://SERVER_IP:8081/v1/recordings/turtlebot-demo/messages?revision=REVISION&streams=1,2&start_ns=START&end_ns=END"
```

The next PR needs only the protocol Catalog DTOs, `BatchDecoder`, and these four continuation
headers to implement Web `RemoteBatchSource`. HTTP Range, object-store SDKs, cache/prefetch,
optimized camera/numeric representations, authentication, multi-segment recordings, and preview
serving remain separate decisions. A native object-storage backend is justified only after mounted
filesystem measurements show that operational constraints or positional-read latency require it.
