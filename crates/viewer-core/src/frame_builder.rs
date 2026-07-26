use crate::DomainState;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BevSnapshot<'a> {
    pub revision: u64,
    pub path: &'a [[f32; 2]],
}

pub struct BevFrameBuilder<'a> {
    state: &'a DomainState,
}

impl<'a> BevFrameBuilder<'a> {
    pub fn new(state: &'a DomainState) -> Self {
        Self { state }
    }

    pub fn build(self) -> BevSnapshot<'a> {
        BevSnapshot {
            revision: self.state.bev.revision(),
            path: self
                .state
                .bev
                .latest()
                .map_or(&[], |frame| frame.points.as_slice()),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SceneSnapshot<'a> {
    pub revision: u64,
    pub cloud_revision: u64,
    pub ego_position: [f32; 2],
    pub ego_yaw: f32,
    pub path: &'a [[f32; 2]],
    pub cloud: &'a [[f32; 3]],
    pub accumulate: bool,
}

pub struct SceneFrameBuilder<'a> {
    state: &'a DomainState,
    accumulate: bool,
}

impl<'a> SceneFrameBuilder<'a> {
    pub fn new(state: &'a DomainState) -> Self {
        Self {
            state,
            accumulate: false,
        }
    }

    pub fn accumulate(mut self, accumulate: bool) -> Self {
        self.accumulate = accumulate;
        self
    }

    pub fn build(self) -> SceneSnapshot<'a> {
        let telemetry = self.state.telemetry.latest();
        let telemetry_revision = telemetry.map_or(0, |frame| frame.arrival_time.0 as u64);
        SceneSnapshot {
            revision: self.state.bev.revision().rotate_left(17) ^ telemetry_revision,
            cloud_revision: self.state.point_cloud.revision(),
            ego_position: telemetry.map_or([0.0, 0.0], |frame| {
                [frame.position_x as f32, frame.position_y as f32]
            }),
            ego_yaw: telemetry.map_or(0.0, |frame| frame.yaw_radians as f32),
            path: self
                .state
                .bev
                .latest()
                .map_or(&[], |frame| frame.points.as_slice()),
            cloud: self
                .state
                .point_cloud
                .latest()
                .map_or(&[], |frame| frame.points.as_slice()),
            accumulate: self.accumulate,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ArrivalTime, BevPathFrame, MeasurementTime, PointCloudFrame, TelemetryFrame};

    #[test]
    fn builders_select_current_domain_values() {
        let mut state = DomainState::default();
        state.bev.apply(BevPathFrame {
            measurement_time: MeasurementTime(1),
            arrival_time: ArrivalTime(2),
            frame_id: "base_link".into(),
            points: vec![[0.0, 1.0]],
        });
        state.telemetry.apply(TelemetryFrame {
            measurement_time: MeasurementTime(1),
            arrival_time: ArrivalTime(3),
            frame_id: "odom".into(),
            child_frame_id: "base_link".into(),
            position_x: 4.0,
            position_y: 5.0,
            yaw_radians: 0.25,
            forward_velocity: 0.0,
            speed: 0.0,
            yaw_rate: 0.0,
        });
        state.point_cloud.apply(PointCloudFrame {
            measurement_time: MeasurementTime(1),
            arrival_time: ArrivalTime(4),
            frame_id: "odom".into(),
            points: vec![[1.0, 2.0, 3.0]],
        });

        let bev = BevFrameBuilder::new(&state).build();
        assert_eq!(bev.path, [[0.0, 1.0]]);
        let scene = SceneFrameBuilder::new(&state).accumulate(true).build();
        assert_eq!(scene.ego_position, [4.0, 5.0]);
        assert_eq!(scene.ego_yaw, 0.25);
        assert_eq!(scene.path, bev.path);
        assert_eq!(scene.cloud, [[1.0, 2.0, 3.0]]);
        assert!(scene.accumulate);
    }
}
