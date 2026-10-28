use crate::{RawMessage, StreamId};

/// Latest-value admission inside one bounded source batch. No payload clones/backing retention.
/// Equal log times use final batch order; ordered-history consumers must not use this policy.
pub(super) fn latest_in_batch(
    messages: &[RawMessage],
    stream: StreamId,
) -> Option<(&RawMessage, u64)> {
    let mut latest: Option<(usize, &RawMessage)> = None;
    let mut count = 0u64;
    for (index, message) in messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.stream_id == stream)
    {
        count += 1;
        if latest.is_none_or(|(previous, old)| {
            (message.arrival_time, index) >= (old.arrival_time, previous)
        }) {
            latest = Some((index, message));
        }
    }
    latest.map(|(_, message)| (message, count - 1))
}
