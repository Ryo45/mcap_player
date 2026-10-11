# Native recording health panels

Open the query-only preset with:

```sh
cargo run -p viewer-native -- --mcap PATH --layout health
```

`config/layouts/native_health.json` contains independent `topic-health` and `diagnostics`
panels. Both use config version 1. Topic Health accepts `{"windowSeconds": 10}`;
Diagnostics accepts `{"topic": "/diagnostics", "windowSeconds": 10}`. These are also
the defaults. Windows must be 1–60 seconds; empty topics and invalid configs become
ordinary panel placeholders. This preset opens recordings without requiring Camera,
Odometry, TF, or other continuous feature streams. Live input displays recording-only
availability text; this change adds no live subscriptions or Web query implementation.

## Transport facts

Topic Health groups MCAP channels by topic and lists every catalog topic, including
unsupported message types. It shows schema names, whole-recording message count from
Statistics (unknown if any constituent count is missing), window sample count, observed
rate, age at the committed playback cursor, recent gap, and maximum gap.

All time calculations use MCAP log time. The window includes both `cursor - N seconds`
and the cursor, represented as `[cursor - N seconds, cursor + 1 ns)`. Rate is
`(sample_count - 1) / (last_sample - first_sample)`; gaps are between consecutive
samples inside the window. Fewer than two samples have no rate or gap. Equal-time
samples retain their multiplicity; a zero observation span has no rate. Maximum gap
is for this window, not for the entire recording. Age uses the exact latest log time
at or before the cursor, including a predecessor older than the window. No predecessor
is displayed as unavailable. No thresholds or OK/WARN/ERROR classification are applied.

Topic Health uses a query-owned multi-stream timestamp traversal. The Source exposes
borrowed chunk timelines and one physical `read_chunk_message_indexes` primitive,
which reads/parses the contiguous Message Index span without loading message chunks.
A frontier merges the stream timelines; each needed chunk is read once per demand,
and its entries update all selected channels before the parsed span is released.
Only one parsed index span is resident. The processed chunk-ID set is bounded by the
read budget; it stores no index entries. No recording-duration decoded/index cache,
global cache manager or generic query framework is introduced.

For predecessors outside the window, the remaining chunks' maximum end time decides
when a stream can stop. This preserves exact latest-before results with overlapping
or non-monotonic chunk time ranges; finding the first predecessor does not prove that
it is the latest. Multi-channel topics merge their samples and sort once for reduction.

Unindexed nonempty topics report an explicit row error without a scan/decode fallback.
Physical read/parse failures, source mutation and budget exhaustion fail the whole
query, so a partial timing snapshot is never committed. Read attempts reserve bytes
and reads before I/O, including failed attempts. Cancellation is checked around reads,
between channel parses, during sample reduction and before returning results. File
mutation checks remain best-effort length/mtime validation before/after positional
reads, supplemented by whole-query validation before returning a snapshot.

One Topic Health demand allows at most 2,048 topics, 4,096 streams, 200,000 timestamp
samples, 32 MiB of index input, and 4,096 index reads, including predecessor searches.
The snapshot contains reduced facts, not a retained timestamp history or payloads.
The index-input budget bounds attempted read bytes, not total heap allocation: the
current raw span, parsed entries and bounded sample vectors have separate allocations.

## Producer-reported diagnostics

Diagnostics accepts only CDR `diagnostic_msgs/msg/DiagnosticArray`. The decoder follows
[DiagnosticArray](https://github.com/ros2/common_interfaces/blob/jazzy/diagnostic_msgs/msg/DiagnosticArray.msg),
[DiagnosticStatus](https://github.com/ros2/common_interfaces/blob/jazzy/diagnostic_msgs/msg/DiagnosticStatus.msg),
and [KeyValue](https://github.com/ros2/common_interfaces/blob/jazzy/diagnostic_msgs/msg/KeyValue.msg).
Producer levels 0/1/2/3 display as OK/WARN/ERROR/STALE; other byte values remain explicit
`UNKNOWN(n)` values. These labels never come from Viewer timing or domain judgments.
The ROS header timestamp is decoded separately and does not determine the query timeline.

Statuses aggregate by `(level, name, message)`. Each group retains count, earliest and
latest log times, and hardware ID/values from the latest occurrence. Different hardware
IDs intentionally share a group under this key. Equal-time details use deterministic
stream/source traversal order. Groups sort newest first with a stable key tie break.
Values expand as details; clicking a group seeks to its latest occurrence.

Malformed arrays increment a visible skipped-message count and retain the first typed
decode error, while other valid messages remain available. A source failure or query
budget failure produces a query error rather than a partial aggregate. Budgets include
10,000 input messages, 16 MiB of selected payload, 128 MiB of compressed plus uncompressed
candidate chunk data, 4,096 chunk visits, 4,096 aggregate groups and 4 MiB of retained
string data (including aggregation keys). Each decoded array is bounded to 1 MiB,
4,096 statuses and 4,096 total key/value pairs. No decoded recording-duration history
is retained.

## Session demand and lifecycle

The data path is `NativePanel -> PanelDataRequirements -> ViewerSession ->
WindowQueryLoader -> concrete query -> McapSource -> typed snapshot -> NativePanel`.
Panels never read sources or decode CDR. Neither query contributes FeatureRuntime
routes or changes playback selection. Identical requirements are deduplicated with
ordered sets and share one `Arc` snapshot across panels. At most eight distinct window
requirements can be active; differing topics/windows remain distinct demands.

A session owns one concrete window-query worker with one pending demand and one result
slot. Normal playback requests refresh at most every 250 ms; paused cursors reuse their
result. Pending forward requests coalesce without starving the current bounded query.
While updating, panels explicitly label the last completed query cursor; Topic Health
age continues to use the current committed playback cursor. Diagnostics hides aggregates
whose counts include expired occurrences until a fresh result is available.

A successful seek in either direction invalidates snapshots and cancels the old
generation immediately. Failed seeks preserve the committed query state. Backward cursor
changes also clear results and cancel previous generations. Source replacement reuses
one worker, preserves pending source resets through coalescing, and rejects old results.
Shutdown cancels, wakes and joins that worker. There are no detached workers, callback
subscriptions, global decoded registries or event buses.

## Comparison with existing query paths

| Query | Demand and result | Work / reuse | Cursor change |
| --- | --- | --- | --- |
| Inspector | topic + cursor; one exact typed message | indexed predecessor, one-span/chunk cache and identity reuse | coalesces cursor requests; presentation excludes future messages |
| Numeric Plot | stream + field; bounded overview/detail/current | extraction and signal-specific reduction | preserves independent series/query semantics |
| Builtin Plot | Odometry topic + signals; overview/progress | one bounded signal scan, shared speed/yaw-rate extraction | recording-wide overview is independent of cursor |
| Topic Health | window; reduced per-topic timing facts | chunk-major index batch + predecessor, one resident parsed span | refreshed window; seek invalidation |
| Diagnostics | topic + window; grouped producer statuses | selected serialized range, bounded CDR decode and reduction | refreshed window; seek invalidation |

The two new queries demonstrably share pending-demand, generation and worker lifecycle,
so they share one concrete loader. Their result types, reduction rules and UI remain
separate. Existing Inspector and Plot paths have different cache and progress ownership;
this change does not force them into a QueryCoordinator or a generic query framework.
Broader sharing should target a demonstrated lifecycle or physical-read duplication
without erasing those semantics.
