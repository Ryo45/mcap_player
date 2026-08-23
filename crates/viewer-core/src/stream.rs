use crate::ArrivalTime;

/// Identifies one stream within a source catalog and the Viewer session built from it.
///
/// This is a source-local runtime token, not a persistent or global identity. Equal numeric
/// values from different recordings or Local/Remote sources need not describe the same stream.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct StreamId(pub u32);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamDescriptor {
    pub id: StreamId,
    pub topic: String,
    pub schema: String,
    pub message_encoding: String,
    /// Number of messages observed for this stream in the complete recording.
    ///
    /// This is a source fact used for coarse planning. It is not a declared or
    /// runtime-observed publishing frequency. Live sources generally leave it unknown.
    pub timing: StreamTimingSummary,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StreamTimingSummary {
    pub message_count: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecordingTimeRange {
    pub start: ArrivalTime,
    pub end_exclusive: ArrivalTime,
}

impl RecordingTimeRange {
    pub fn new(start: ArrivalTime, end_exclusive: ArrivalTime) -> Option<Self> {
        (start < end_exclusive).then_some(Self {
            start,
            end_exclusive,
        })
    }

    pub fn duration_ns(self) -> u64 {
        self.end_exclusive.0.saturating_sub(self.start.0) as u64
    }
}

#[derive(Clone, Debug, Default)]
pub struct SourceCatalog {
    /// Recording-wide MCAP log-time range. Push/live sources may not have one.
    pub time_range: Option<RecordingTimeRange>,
    pub streams: Vec<StreamDescriptor>,
    pub capabilities: SourceCapabilities,
}

/// Product-visible operations supported by an opened source.
///
/// This is a closed declaration, not a generic source trait. Exact seek and both restore modes
/// are deliberately distinct so an adapter cannot defer an indexed-restore limitation until the
/// first timeline interaction.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SourceCapabilities {
    pub catalog: bool,
    pub forward_playback: bool,
    pub exact_seek: bool,
    pub history_restore: bool,
    pub persistent_restore: bool,
}

impl SourceCapabilities {
    pub const INDEXED_RECORDING: Self = Self {
        catalog: true,
        forward_playback: true,
        exact_seek: true,
        history_restore: true,
        persistent_restore: true,
    };

    pub const LIVE: Self = Self {
        catalog: true,
        forward_playback: true,
        exact_seek: false,
        history_restore: false,
        persistent_restore: false,
    };
}

impl SourceCatalog {
    pub fn by_id(&self, id: StreamId) -> Option<&StreamDescriptor> {
        self.streams.iter().find(|stream| stream.id == id)
    }

    pub fn by_topic(&self, topic: &str) -> Option<&StreamDescriptor> {
        self.streams.iter().find(|stream| stream.topic == topic)
    }
}
