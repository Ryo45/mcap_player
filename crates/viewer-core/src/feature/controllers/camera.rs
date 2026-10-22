use super::ProcessingCounters;
use crate::{
    CameraFrame, CameraId, CameraState, RawMessage, RestoreSemantics, SessionPlan, StreamId,
    decode_compressed_image_bytes,
};
use std::{
    collections::{BTreeMap, HashMap},
    time::Duration,
};

const BACKGROUND_CAMERA_INTERVAL: Duration = Duration::from_millis(200);

/// Shared Camera capability used by all Camera panels for one session.
///
/// Admission/coalescing happens on serialized messages. Pixel decoding remains a presentation
/// concern and the retained image data is a `Bytes` slice of the admitted CDR payload.
/// Focus materializes the latest pending frame on each advance; only backgrounds have a rate limit.
#[derive(Clone)]
pub struct CameraController {
    raw_streams: std::collections::HashSet<StreamId>,
    stream_to_camera: HashMap<StreamId, CameraId>,
    topics: Vec<(CameraId, String)>,
    state: CameraState,
    focused_camera: Option<CameraId>,
    presentation_elapsed: Duration,
    next_presentation: BTreeMap<CameraId, Duration>,
    pending: BTreeMap<CameraId, RawMessage>,
    counters: ProcessingCounters,
    input_frames: u64,
    presented_frames: u64,
    presented_by_id: BTreeMap<CameraId, u64>,
}

impl CameraController {
    pub const fn restore_semantics() -> RestoreSemantics {
        RestoreSemantics::LatestBefore
    }

    pub fn new(plan: &SessionPlan) -> Self {
        let stream_to_camera = plan
            .camera_routes()
            .iter()
            .map(|route| (route.stream.id, route.camera_id))
            .collect();
        let topics = plan.camera_topics();
        let mut controller = Self {
            raw_streams: plan
                .camera_routes()
                .iter()
                .filter(|r| r.stream.schema == "sensor_msgs/msg/Image")
                .map(|r| r.stream.id)
                .collect(),
            stream_to_camera,
            topics,
            state: CameraState::default(),
            focused_camera: plan.primary_camera(),
            presentation_elapsed: Duration::ZERO,
            next_presentation: BTreeMap::new(),
            pending: BTreeMap::new(),
            counters: ProcessingCounters::default(),
            input_frames: 0,
            presented_frames: 0,
            presented_by_id: BTreeMap::new(),
        };
        controller.reset_schedule();
        controller
    }

    /// Routes and coalesces a serialized Camera message without decoding it.
    pub fn admit(&mut self, message: &RawMessage) -> bool {
        let Some(camera_id) = self.stream_to_camera.get(&message.stream_id).copied() else {
            return false;
        };
        self.input_frames = self.input_frames.saturating_add(1);
        if self.pending.insert(camera_id, message.clone()).is_some() {
            self.counters.dropped = self.counters.dropped.saturating_add(1);
        }
        true
    }

    pub fn advance(&mut self, elapsed: Duration) {
        self.presentation_elapsed = self.presentation_elapsed.saturating_add(elapsed);
        let due = self
            .pending
            .keys()
            .copied()
            .filter(|camera_id| self.is_due(*camera_id))
            .collect::<Vec<_>>();
        for camera_id in due {
            let message = self
                .pending
                .remove(&camera_id)
                .expect("due Camera came from the pending map");
            let _ = self.decode_and_apply(camera_id, message);
            if Some(camera_id) != self.focused_camera {
                // Keep background phases; missed periods do not trigger catch-up decoding.
                let deadline = self.next_presentation[&camera_id];
                let lateness = self.presentation_elapsed.saturating_sub(deadline);
                let remainder = Duration::from_nanos(
                    (lateness.as_nanos() % BACKGROUND_CAMERA_INTERVAL.as_nanos()) as u64,
                );
                self.next_presentation.insert(
                    camera_id,
                    self.presentation_elapsed
                        .saturating_add(BACKGROUND_CAMERA_INTERVAL - remainder),
                );
            }
            self.presented_frames = self.presented_frames.saturating_add(1);
            let count = self.presented_by_id.entry(camera_id).or_default();
            *count = count.saturating_add(1);
        }
    }

    /// Applies one exact seek predecessor immediately, bypassing playback rate scheduling.
    ///
    /// Unlike forward admission, restore is strict: a routed malformed predecessor is an error
    /// so a staging [`crate::FeatureRuntime`] can discard the whole candidate state.
    pub fn restore(&mut self, message: &RawMessage) -> Result<bool, crate::DecodeError> {
        let Some(camera_id) = self.stream_to_camera.get(&message.stream_id).copied() else {
            return Ok(false);
        };
        self.input_frames = self.input_frames.saturating_add(1);
        self.decode_and_apply(camera_id, message.clone())?;
        self.presented_frames = self.presented_frames.saturating_add(1);
        let count = self.presented_by_id.entry(camera_id).or_default();
        *count = count.saturating_add(1);
        Ok(true)
    }

    pub fn reset_for_restore(&mut self) {
        self.state.cold_seek();
        self.presentation_elapsed = Duration::ZERO;
        self.pending.clear();
        self.reset_schedule();
    }

    pub fn set_focused_camera(&mut self, focused_camera: Option<CameraId>) {
        let focused_camera = focused_camera
            .filter(|camera_id| self.topics.iter().any(|(id, _)| id == camera_id))
            .or_else(|| self.topics.first().map(|(camera_id, _)| *camera_id));
        if self.focused_camera == focused_camera {
            return;
        }
        if let Some(previous) = self.focused_camera {
            self.next_presentation.insert(
                previous,
                self.presentation_elapsed
                    .saturating_add(BACKGROUND_CAMERA_INTERVAL),
            );
        }
        self.focused_camera = focused_camera;
        if let Some(camera_id) = focused_camera {
            self.next_presentation.remove(&camera_id);
        }
    }

    pub fn state(&self) -> &CameraState {
        &self.state
    }

    pub fn topics(&self) -> &[(CameraId, String)] {
        &self.topics
    }

    pub fn focused_camera(&self) -> Option<CameraId> {
        self.focused_camera
    }

    pub fn counters(&self) -> ProcessingCounters {
        self.counters
    }

    pub fn input_frames(&self) -> u64 {
        self.input_frames
    }

    pub fn presented_frames(&self) -> u64 {
        self.presented_frames
    }

    pub fn presented_by_id(&self) -> &BTreeMap<CameraId, u64> {
        &self.presented_by_id
    }

    pub fn background_hz() -> f64 {
        1.0 / BACKGROUND_CAMERA_INTERVAL.as_secs_f64()
    }

    fn is_due(&self, camera_id: CameraId) -> bool {
        Some(camera_id) == self.focused_camera
            || self
                .next_presentation
                .get(&camera_id)
                .is_none_or(|deadline| self.presentation_elapsed >= *deadline)
    }

    fn reset_schedule(&mut self) {
        self.next_presentation.clear();
        let background = self
            .topics
            .iter()
            .map(|(camera_id, _)| *camera_id)
            .filter(|camera_id| Some(*camera_id) != self.focused_camera)
            .collect::<Vec<_>>();
        let background_count = background.len().max(1) as f64;
        for (index, camera_id) in background.into_iter().enumerate() {
            let phase = Duration::from_secs_f64(
                BACKGROUND_CAMERA_INTERVAL.as_secs_f64() * index as f64 / background_count,
            );
            self.next_presentation.insert(camera_id, phase);
        }
    }

    fn decode_and_apply(
        &mut self,
        camera_id: CameraId,
        message: RawMessage,
    ) -> Result<(), crate::DecodeError> {
        let decoded = if self.raw_streams.contains(&message.stream_id) {
            crate::decode_raw_image_bytes(message.payload).map(|image| CameraFrame {
                camera_id,
                arrival_time: message.arrival_time,
                measurement_time: image.measurement_time,
                frame_id: image.frame_id,
                format: crate::CameraImageFormat::Raw(image.layout),
                data: image.data,
            })
        } else {
            decode_compressed_image_bytes(message.payload).map(|image| CameraFrame {
                camera_id,
                arrival_time: message.arrival_time,
                measurement_time: image.measurement_time,
                frame_id: image.frame_id,
                format: if image.format == "png" {
                    crate::CameraImageFormat::Png
                } else {
                    crate::CameraImageFormat::Jpeg
                },
                data: image.data,
            })
        };
        match decoded {
            Ok(image) => {
                self.state.apply(image);
                self.counters.decoded = self.counters.decoded.saturating_add(1);
                Ok(())
            }
            Err(error) => {
                self.counters.errors = self.counters.errors.saturating_add(1);
                Err(error)
            }
        }
    }
}
