use super::ProcessingCounters;
use crate::{
    PointCloudFrame, PointCloudState, RawMessage, RestoreSemantics, SessionPlan, StreamId,
    decode_laser_scan, decode_point_cloud2,
};

#[derive(Clone, Debug)]
pub struct PointCloudController {
    stream: Option<StreamId>,
    point_cloud2: bool,
    point_cloud: PointCloudState,
    counters: ProcessingCounters,
}

impl PointCloudController {
    pub const fn restore_semantics() -> RestoreSemantics {
        RestoreSemantics::LatestBefore
    }

    pub fn new(plan: &SessionPlan) -> Self {
        Self {
            stream: plan.point_cloud_stream().map(|stream| stream.id),
            point_cloud2: plan
                .point_cloud_stream()
                .is_some_and(|s| s.schema == "sensor_msgs/msg/PointCloud2"),
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
        let decoded = if self.point_cloud2 {
            decode_point_cloud2(&message.payload).map(|cloud| PointCloudFrame {
                measurement_time: cloud.measurement_time,
                arrival_time: message.arrival_time,
                frame_id: cloud.frame_id,
                points: cloud.points,
                attributes: cloud.attributes,
            })
        } else {
            decode_laser_scan(&message.payload).map(|scan| {
                let mut points = Vec::with_capacity(scan.ranges.len());
                for (index, range) in scan.ranges.iter().copied().enumerate() {
                    if !range.is_finite() || range < scan.range_min || range > scan.range_max {
                        continue;
                    }
                    let angle = scan.angle_min + index as f32 * scan.angle_increment;
                    points.push([range * angle.cos(), range * angle.sin(), 0.0]);
                }
                PointCloudFrame {
                    measurement_time: scan.measurement_time,
                    arrival_time: message.arrival_time,
                    frame_id: scan.frame_id,
                    points,
                    attributes: Default::default(),
                }
            })
        };
        match decoded {
            Ok(frame) => {
                self.point_cloud.apply(frame);
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
