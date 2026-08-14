use crate::{
    CameraId, DomainPerformance, DomainRuntime, DomainState, PipelineCounters, RawMessage,
    SessionPlanError, StageTiming, StreamCatalog,
};
use std::{fmt, time::Duration};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlaybackPerformance {
    pub source_read: StageTiming,
    pub pipeline_decode: StageTiming,
    pub state_apply: StageTiming,
    pub camera_input_frames: u64,
    pub camera_presented_frames: u64,
    pub camera_presented_by_id: std::collections::BTreeMap<CameraId, u64>,
}

impl PlaybackPerformance {
    fn from_parts(source_read: StageTiming, domain: &DomainPerformance) -> Self {
        Self {
            source_read,
            pipeline_decode: domain.pipeline_decode,
            state_apply: domain.state_apply,
            camera_input_frames: domain.camera_input_frames,
            camera_presented_frames: domain.camera_presented_frames,
            camera_presented_by_id: domain.camera_presented_by_id.clone(),
        }
    }

    pub fn focused_camera_hz(&self) -> f64 {
        DomainPerformance::default().focused_camera_hz()
    }

    pub fn background_camera_hz(&self) -> f64 {
        DomainPerformance::default().background_camera_hz()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlaybackCoreError(String);

impl fmt::Display for PlaybackCoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for PlaybackCoreError {}

impl From<SessionPlanError> for PlaybackCoreError {
    fn from(error: SessionPlanError) -> Self {
        Self(error.to_string())
    }
}

/// Compatibility facade for playback callers; shared-domain work lives in `DomainRuntime`.
pub struct PlaybackCore {
    runtime: DomainRuntime,
    source_read: StageTiming,
}

impl PlaybackCore {
    pub fn new(
        catalog: &StreamCatalog,
        primary_camera_topic: &str,
    ) -> Result<Self, PlaybackCoreError> {
        Ok(Self {
            runtime: DomainRuntime::from_catalog(catalog, primary_camera_topic)?,
            source_read: StageTiming::default(),
        })
    }

    pub fn process_forward(
        &mut self,
        elapsed: Duration,
        messages: impl IntoIterator<Item = RawMessage>,
    ) {
        self.runtime.process(elapsed, messages);
    }

    pub fn reset_for_restore(&mut self) {
        self.runtime.reset_for_restore();
    }

    pub fn apply_transform_restore(&mut self, messages: impl IntoIterator<Item = RawMessage>) {
        self.runtime.apply_transform_restore(messages);
    }

    pub fn set_focused_camera(&mut self, focused_camera: Option<CameraId>) {
        self.runtime.set_focused_camera(focused_camera);
    }

    pub fn state(&self) -> &DomainState {
        self.runtime.state()
    }

    pub fn camera_topics(&self) -> &[(CameraId, String)] {
        self.runtime.camera_topics()
    }

    pub fn focused_camera(&self) -> Option<CameraId> {
        self.runtime.focused_camera()
    }

    pub fn counters(&self) -> PipelineCounters {
        self.runtime.counters()
    }

    pub fn performance(&self) -> PlaybackPerformance {
        PlaybackPerformance::from_parts(self.source_read, self.runtime.performance())
    }

    pub(crate) fn record_source_read(&mut self, elapsed: Duration) {
        self.source_read.record(elapsed);
    }

    pub(crate) fn staging_for_restore(&self) -> Self {
        Self {
            runtime: self.runtime.staging_for_restore(),
            source_read: self.source_read,
        }
    }

    pub(crate) fn commit_restore(&mut self, staged: Self) {
        self.runtime.commit_restore(staged.runtime);
    }

    #[cfg(test)]
    pub(crate) fn pending_camera_count(&self) -> usize {
        self.runtime.pending_camera_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{McapPlayback, McapSource, PlaybackCommand};

    fn camera_fixture() -> Vec<u8> {
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/camera-jpeg/camera_front_3s.mcap"),
        )
        .unwrap()
    }

    #[test]
    fn direct_message_reduction_matches_local_playback() {
        let bytes = camera_fixture();
        let mut source = McapSource::new(bytes.as_slice()).unwrap();
        let mut core =
            PlaybackCore::new(source.catalog(), "/camera/front/image/compressed").unwrap();
        let messages = source.read_until(source.time_range().1).unwrap();
        core.process_forward(Duration::from_secs(10), messages);

        let mut playback =
            McapPlayback::new(bytes.as_slice(), "/camera/front/image/compressed").unwrap();
        playback.apply_command(PlaybackCommand::Toggle).unwrap();
        playback.tick(Duration::from_secs(10)).unwrap();

        assert_eq!(core.counters(), playback.counters());
        assert_eq!(
            core.state()
                .camera
                .latest_by_arrival()
                .map(|frame| frame.arrival_time),
            playback
                .state()
                .camera
                .latest_by_arrival()
                .map(|frame| frame.arrival_time)
        );
        assert_eq!(
            core.performance().camera_input_frames,
            playback.performance().camera_input_frames
        );
        assert_eq!(
            core.performance().camera_presented_frames,
            playback.performance().camera_presented_frames
        );
    }
}
