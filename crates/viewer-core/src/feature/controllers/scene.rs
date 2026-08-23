use super::ProcessingCounters;
use crate::{
    PointCloudFrame, PointCloudState, RawMessage, RestoreSemantics, SessionPlan, StreamId,
    decode_laser_scan,
};

#[derive(Clone, Debug)]
pub struct SceneController {
    stream: Option<StreamId>,
    point_cloud: PointCloudState,
    counters: ProcessingCounters,
}

impl SceneController {
    pub const fn restore_semantics() -> RestoreSemantics {
        RestoreSemantics::LatestBefore
    }

    pub fn new(plan: &SessionPlan) -> Self {
        Self {
            stream: plan.point_cloud_stream().map(|stream| stream.id),
            point_cloud: PointCloudState::default(),
            counters: ProcessingCounters::default(),
        }
    }

    pub fn process(&mut self, message: &RawMessage) -> bool {
        if Some(message.stream_id) != self.stream {
            return false;
        }
        let _ = self.decode_and_apply(message);
        true
    }

    pub fn restore(&mut self, message: &RawMessage) -> Result<bool, crate::DecodeError> {
        if Some(message.stream_id) != self.stream {
            return Ok(false);
        }
        self.decode_and_apply(message)?;
        Ok(true)
    }

    fn decode_and_apply(&mut self, message: &RawMessage) -> Result<(), crate::DecodeError> {
        match decode_laser_scan(&message.payload) {
            Ok(scan) => {
                let mut points = Vec::with_capacity(scan.ranges.len());
                for (index, range) in scan.ranges.iter().copied().enumerate() {
                    if !range.is_finite() || range < scan.range_min || range > scan.range_max {
                        continue;
                    }
                    let angle = scan.angle_min + index as f32 * scan.angle_increment;
                    points.push([range * angle.cos(), range * angle.sin(), 0.0]);
                }
                self.point_cloud.apply(PointCloudFrame {
                    measurement_time: scan.measurement_time,
                    arrival_time: message.arrival_time,
                    frame_id: scan.frame_id,
                    points,
                });
                self.counters.decoded = self.counters.decoded.saturating_add(1);
                Ok(())
            }
            Err(error) => {
                self.counters.errors = self.counters.errors.saturating_add(1);
                Err(error)
            }
        }
    }

    pub fn reset_for_restore(&mut self) {
        self.point_cloud.cold_seek();
    }

    pub fn point_cloud(&self) -> &PointCloudState {
        &self.point_cloud
    }

    pub fn counters(&self) -> ProcessingCounters {
        self.counters
    }
}
