# Preview MCAP v0

## Purpose

`preview.mcap` is a reproducible, low-resolution catalog used for log-wide exploration. The main
MCAP remains the source of truth. Preview data does not enter `DomainState`, does not replace
`LoadedSignal`, and is not bookmark storage.

## Topics and payloads

| Topic | Payload | MCAP `log_time` |
| --- | --- | --- |
| `/preview/build_info` | JSON BuildInfo | `0` |
| `/preview/camera/<camera_id>` | binary Camera envelope | frame arrival time |
| `/preview/signal/<signal_id>` | one JSON envelope bucket | bucket start |
| `/preview/trajectory` | one JSON position | point time |

BuildInfo occurs exactly once and contains `previewSchemaVersion`, generator name/version, and the
source fingerprint. Known topics reject unknown schema versions. Unknown topics are ignored for
forward-compatible additions.

Camera messages contain a four-byte little-endian JSON metadata length, the metadata JSON, and raw
JPEG bytes. JPEG bytes are never represented as Base64 or JSON numbers. Metadata records camera ID,
measurement/arrival time, frame ID, encoding, width, and height; topic ID, payload ID, and MCAP
header time must agree.

Signal messages contain one min/max envelope bucket each. v0 supports `vehicle-core::SignalId::Speed`
as `/preview/signal/speed`, with a 100 ms `bucketNs`. Exact-fidelity signals cannot be written.
Trajectory messages contain one time-tagged XY point.

Writer output uses uncompressed MCAP chunks while retaining summary, Chunk Index, and Message Index
records. Preview files are small, so avoiding compression keeps the future browser IndexedReader
path simple and makes arbitrary range access immediately useful. Compression of the main MCAP is
independent.

## Source identity

The algorithm name is `mcap-summary-identity-v1`. Its canonical value joins file size, Footer summary
CRC, message start/end time, message count, schema count, channel count, and chunk count. File paths
are excluded. This is a fast non-cryptographic stale-sidecar check, not an integrity or security
guarantee; collisions and a malicious replacement remain possible.

## Builder policy

The Builder memory-maps the input and performs one `MessageStream` traversal. For every compressed
image channel it retains the last-arriving frame in each one-second bucket, decodes that selected
JPEG, scales it without upsampling to at most 320×180, then writes JPEG quality 72 by default.
Camera IDs follow the viewer convention: the configured primary Camera topic is ID 0, and remaining
channels use channel-ID order.

`/odom` speed uses the same planar magnitude `hypot(vx, vy)` as the exact Plot loader and is
aggregated directly into 100 ms first/last/min/max/count buckets. Odometry XY is sampled at no more
than 2 Hz for trajectory. Decode failures increment the Camera warning count rather than aborting
the complete artifact.

```text
cargo run -p preview-builder -- input.mcap --output preview.mcap
```

Output overwrite requires `--force`. The default output is `preview.mcap` beside the input.

## Ownership and future Web reading

`viewer-preview-mcap` depends only on `viewer-core`, `mcap`, and Serde. It constructs
`PreviewArtifact` and `PreviewSnapshot`, never `DomainState`. A future Web source can use MCAP
Summary/Message Index ranges and the same wire reader/query rules; this change does not introduce a
production RangeSource or async reader.
