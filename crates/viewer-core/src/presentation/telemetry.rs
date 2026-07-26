use crate::TelemetryFrame;

#[derive(Clone, Debug, PartialEq)]
pub struct TelemetryPresentation {
    pub frame_id: String,
    pub child_frame_id: String,
    pub position_x: f64,
    pub position_y: f64,
    pub yaw_radians: f64,
    pub forward_velocity: f64,
    pub speed: f64,
    pub yaw_rate: f64,
}

impl From<&TelemetryFrame> for TelemetryPresentation {
    fn from(frame: &TelemetryFrame) -> Self {
        Self {
            frame_id: frame.frame_id.clone(),
            child_frame_id: frame.child_frame_id.clone(),
            position_x: frame.position_x,
            position_y: frame.position_y,
            yaw_radians: frame.yaw_radians,
            forward_velocity: frame.forward_velocity,
            speed: frame.speed,
            yaw_rate: frame.yaw_rate,
        }
    }
}
