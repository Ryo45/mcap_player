use crate::{ArrivalTime, MeasurementTime};

#[derive(Clone, Debug, PartialEq)]
pub struct TelemetryFrame {
    pub measurement_time: MeasurementTime,
    pub arrival_time: ArrivalTime,
    pub frame_id: String,
    pub child_frame_id: String,
    pub position_x: f64,
    pub position_y: f64,
    pub yaw_radians: f64,
    pub forward_velocity: f64,
    pub speed: f64,
    pub yaw_rate: f64,
}

#[derive(Clone, Debug, Default)]
pub struct TelemetryState {
    generation: u64,
    latest: Option<TelemetryFrame>,
}

impl TelemetryState {
    pub fn apply(&mut self, generation: u64, frame: TelemetryFrame) -> bool {
        if generation != self.generation {
            return false;
        }
        if self
            .latest
            .as_ref()
            .is_some_and(|current| current.arrival_time > frame.arrival_time)
        {
            return false;
        }
        self.latest = Some(frame);
        true
    }

    pub fn cold_seek(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.latest = None;
        self.generation
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn latest(&self) -> Option<&TelemetryFrame> {
        self.latest.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(arrival: i64) -> TelemetryFrame {
        TelemetryFrame {
            measurement_time: MeasurementTime(arrival - 1),
            arrival_time: ArrivalTime(arrival),
            frame_id: "odom".into(),
            child_frame_id: "base_footprint".into(),
            position_x: 1.0,
            position_y: 2.0,
            yaw_radians: 0.1,
            forward_velocity: 0.2,
            speed: 0.2,
            yaw_rate: 0.3,
        }
    }

    #[test]
    fn rejects_old_and_stale_generation() {
        let mut state = TelemetryState::default();
        assert!(state.apply(0, frame(2)));
        assert!(!state.apply(0, frame(1)));
        let generation = state.cold_seek();
        assert!(state.latest().is_none());
        assert!(!state.apply(generation - 1, frame(3)));
        assert!(state.apply(generation, frame(3)));
    }
}
