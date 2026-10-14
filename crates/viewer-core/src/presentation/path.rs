//! Placement of canonical path positions; ground projection is a presentation policy.
use crate::{MeasurementTime, PathFrame, TransformState};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PathProjectionPolicy {
    /// Stable world frame for anchoring the authored path and camera-time reprojection.
    pub fixed_frame: String,
    /// Flatten in this TF frame's z=0 plane; None preserves every authored z.
    pub ground_frame: Option<String>,
}
impl Default for PathProjectionPolicy {
    fn default() -> Self {
        Self {
            fixed_frame: "odom".into(),
            ground_frame: None,
        }
    }
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FrameTransformError {
    pub source_frame: String,
    pub target_frame: String,
}
impl PathProjectionPolicy {
    pub fn positions_in_fixed_frame(
        &self,
        path: &PathFrame,
        tf: &TransformState,
    ) -> Result<Vec<[f64; 3]>, FrameTransformError> {
        let mut positions = path.points.clone();
        let mut source = path.frame_id.as_str();
        if let Some(ground) = &self.ground_frame {
            positions = transform(tf, source, ground, path.measurement_time, &positions)?;
            for p in &mut positions {
                p[2] = 0.0;
            }
            source = ground;
        }
        transform(
            tf,
            source,
            &self.fixed_frame,
            path.measurement_time,
            &positions,
        )
    }
    pub fn positions_in_frame(
        &self,
        path: &PathFrame,
        tf: &TransformState,
        target: &str,
        target_time: MeasurementTime,
    ) -> Result<Vec<[f64; 3]>, FrameTransformError> {
        let fixed = self.positions_in_fixed_frame(path, tf)?;
        transform(tf, &self.fixed_frame, target, target_time, &fixed)
    }
}
fn transform(
    tf: &TransformState,
    source: &str,
    target: &str,
    time: MeasurementTime,
    positions: &[[f64; 3]],
) -> Result<Vec<[f64; 3]>, FrameTransformError> {
    tf.transform_positions_at(source, target, time, positions)
        .ok_or_else(|| FrameTransformError {
            source_frame: source.into(),
            target_frame: target.into(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ArrivalTime, TransformBatch, TransformStamped};
    #[test]
    fn ground_frame_uses_tf_and_preserve_keeps_nonzero_height() {
        let path = PathFrame {
            frame_id: "base_link".into(),
            measurement_time: MeasurementTime(10),
            arrival_time: ArrivalTime(11),
            points: vec![[1.0, 0.0, 0.2]],
        };
        let mut tf = TransformState::default();
        tf.apply(TransformBatch {
            arrival_time: ArrivalTime(1),
            is_static: true,
            transforms: vec![TransformStamped {
                frame_id: "base_footprint".into(),
                child_frame_id: "base_link".into(),
                measurement_time: MeasurementTime(0),
                translation: [0.0, 0.0, 0.01],
                rotation: [0.0, 0.0, 0.0, 1.0],
            }],
        });
        let mut policy = PathProjectionPolicy {
            fixed_frame: "base_footprint".into(),
            ground_frame: None,
        };
        assert_eq!(
            policy.positions_in_fixed_frame(&path, &tf).unwrap()[0],
            [1.0, 0.0, 0.21000000000000002]
        );
        policy.ground_frame = Some("base_footprint".into());
        assert_eq!(
            policy.positions_in_fixed_frame(&path, &tf).unwrap()[0],
            [1.0, 0.0, 0.0]
        );
        assert_eq!(path.points[0][2], 0.2);
        policy.ground_frame = Some("missing".into());
        assert!(policy.positions_in_fixed_frame(&path, &tf).is_err());
    }
    #[test]
    fn authored_path_is_anchored_at_path_time_and_camera_uses_its_own_time() {
        let mut tf = TransformState::default();
        for (time, x) in [(10, 1.0), (20, 2.0)] {
            tf.apply(TransformBatch {
                arrival_time: ArrivalTime(time),
                is_static: false,
                transforms: vec![TransformStamped {
                    frame_id: "odom".into(),
                    child_frame_id: "base_link".into(),
                    measurement_time: MeasurementTime(time),
                    translation: [x, 0.0, 0.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                }],
            });
        }
        let path = PathFrame {
            frame_id: "base_link".into(),
            measurement_time: MeasurementTime(10),
            arrival_time: ArrivalTime(10),
            points: vec![[3.0, 0.0, 0.5]],
        };
        let policy = PathProjectionPolicy::default();
        assert_eq!(
            policy.positions_in_fixed_frame(&path, &tf).unwrap(),
            [[4.0, 0.0, 0.5]]
        );
        assert_eq!(
            policy
                .positions_in_frame(&path, &tf, "base_link", MeasurementTime(20))
                .unwrap(),
            [[2.0, 0.0, 0.5]]
        );
        assert!(
            policy
                .positions_in_frame(&path, &tf, "base_link", MeasurementTime(0))
                .is_err()
        );
    }
}
