#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProcessingCounters {
    pub decoded: u64,
    pub errors: u64,
    pub unknown_streams: u64,
    /// High-bandwidth inputs coalesced before an expensive decode.
    pub dropped: u64,
}

impl ProcessingCounters {
    pub fn merge(&mut self, other: Self) {
        self.decoded = self.decoded.saturating_add(other.decoded);
        self.errors = self.errors.saturating_add(other.errors);
        self.unknown_streams = self.unknown_streams.saturating_add(other.unknown_streams);
        self.dropped = self.dropped.saturating_add(other.dropped);
    }
}
