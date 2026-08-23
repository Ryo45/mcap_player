//! Playback state machines, bounded windows, and performance accounting.

mod clock;
mod data_window;
mod engine;
mod timing;

pub use clock::{PlaybackClock, PlaybackCommand, PlaybackLoadState, PlaybackSpeed, PlaybackView};
pub use data_window::{
    DataWindowError, FetchDemand, FetchIntent, FetchPlanner, FetchProfile, MemoryWindowStore,
    SerializedWindow, TimeRange as DataWindowTimeRange,
};
pub use engine::{McapPlayback, McapPlaybackError, McapSeekError, PlaybackEffect};
pub use timing::StageTiming;
