use super::ProcessingCounters;
use crate::{
    RawMessage, RestoreSemantics, SessionPlan, StreamId, TelemetryFrame, TelemetryState,
    decode_odometry,
};

#[derive(Clone)]
pub struct OdometryController {
    stream: Option<StreamId>,
    state: TelemetryState,
    counters: ProcessingCounters,
}

impl OdometryController {
    pub const fn restore_semantics() -> RestoreSemantics {
        RestoreSemantics::LatestBefore
    }

    pub fn new(plan: &SessionPlan) -> Self {
        Self {
            stream: plan.odometry_stream().map(|stream| stream.id),
            state: TelemetryState::default(),
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
        match decode_odometry(&message.payload) {
            Ok(odometry) => {
                let [qx, qy, qz, qw] = odometry.orientation;
                let sin_yaw = 2.0 * (qw * qz + qx * qy);
                let cos_yaw = 1.0 - 2.0 * (qy * qy + qz * qz);
                let [vx, vy, _] = odometry.linear_velocity;
                self.state.apply(TelemetryFrame {
                    measurement_time: odometry.measurement_time,
                    arrival_time: message.arrival_time,
                    frame_id: odometry.frame_id,
                    child_frame_id: odometry.child_frame_id,
                    position_x: odometry.position[0],
                    position_y: odometry.position[1],
                    yaw_radians: sin_yaw.atan2(cos_yaw),
                    forward_velocity: vx,
                    speed: vx.hypot(vy),
                    yaw_rate: odometry.angular_velocity[2],
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
        self.state.cold_seek();
    }

    pub fn state(&self) -> &TelemetryState {
        &self.state
    }

    pub fn counters(&self) -> ProcessingCounters {
        self.counters
    }
}
