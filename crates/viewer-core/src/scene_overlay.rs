//! Validated metric Scene objects, independent of Wasm and GPU resources.
use crate::RigidTransform;
use std::{error::Error, fmt};

pub const MAX_OVERLAY_LINES: usize = 4096;
pub const MAX_OVERLAY_BOXES: usize = 256;

/// Stable object identity within one extension, shared by its lines and boxes.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SceneObjectId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneLine {
    pub id: SceneObjectId,
    pub start: [f32; 3],
    pub end: [f32; 3],
    /// Opaque linear RGB; initial renderer uses one-pixel depth-tested lines.
    pub color: [f32; 3],
}

/// An oriented physical box. Size is the full local-axis extent in metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneBox {
    pub id: SceneObjectId,
    pub center: [f32; 3],
    /// Unit quaternion, ROS x/y/z/w order, relative to the snapshot's source frame.
    pub orientation: [f32; 4],
    pub size: [f32; 3],
    pub color: [f32; 3],
}

impl SceneBox {
    pub fn corners(&self) -> [[f32; 3]; 8] {
        std::array::from_fn(|i| {
            let p = std::array::from_fn::<_, 3, _>(|axis| {
                self.size[axis] * if i & (1 << axis) == 0 { -0.5 } else { 0.5 }
            });
            let rotated = crate::feature::rotate(self.orientation.map(f64::from), p.map(f64::from));
            std::array::from_fn(|axis| (f64::from(self.center[axis]) + rotated[axis]) as f32)
        })
    }
}

/// Complete replacement snapshot: absent IDs are removed, not retained as history.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SceneOverlay {
    lines: Vec<SceneLine>,
    boxes: Vec<SceneBox>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SceneOverlayError;
impl fmt::Display for SceneOverlayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(
            "overlay exceeds limits or contains invalid geometry, colors or duplicate object IDs",
        )
    }
}
impl Error for SceneOverlayError {}
fn position_valid(p: &[f32; 3]) -> bool {
    p.iter().all(|v| v.is_finite() && v.abs() <= 1e6)
}
fn color_valid(p: &[f32; 3]) -> bool {
    p.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v))
}

impl SceneOverlay {
    pub fn new(
        mut lines: Vec<SceneLine>,
        mut boxes: Vec<SceneBox>,
    ) -> Result<Self, SceneOverlayError> {
        let mut ids = std::collections::BTreeSet::new();
        if lines.len() > MAX_OVERLAY_LINES
            || boxes.len() > MAX_OVERLAY_BOXES
            || lines.iter().any(|line| {
                !ids.insert(line.id)
                    || !position_valid(&line.start)
                    || !position_valid(&line.end)
                    || !color_valid(&line.color)
            })
            || boxes.iter().any(|b| {
                let norm: f64 = b.orientation.iter().map(|v| f64::from(*v).powi(2)).sum();
                !ids.insert(b.id)
                    || !position_valid(&b.center)
                    || !color_valid(&b.color)
                    || !b.orientation.iter().all(|v| v.is_finite())
                    || (norm - 1.).abs() > 1e-5
                    || !b.size.iter().all(|v| v.is_finite() && *v > 0. && *v <= 1e6)
                    || !b.corners().iter().all(position_valid)
            })
        {
            return Err(SceneOverlayError);
        }
        lines.sort_unstable_by_key(|line| line.id);
        boxes.sort_unstable_by_key(|b| b.id);
        Ok(Self { lines, boxes })
    }
    pub fn lines(&self) -> &[SceneLine] {
        &self.lines
    }
    pub fn boxes(&self) -> &[SceneBox] {
        &self.boxes
    }

    /// A rigid placement preserves sizes and object identities; no display scale is applied.
    pub fn transformed(&self, transform: RigidTransform) -> Result<Self, SceneOverlayError> {
        let lines = self
            .lines
            .iter()
            .map(|line| SceneLine {
                start: transform.transform_point(line.start),
                end: transform.transform_point(line.end),
                ..*line
            })
            .collect();
        let boxes = self
            .boxes
            .iter()
            .map(|b| SceneBox {
                center: transform.transform_point(b.center),
                orientation: transform.transform_orientation(b.orientation),
                ..*b
            })
            .collect();
        Self::new(lines, boxes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn box_object() -> SceneBox {
        SceneBox {
            id: SceneObjectId(1),
            center: [1., 2., 3.],
            orientation: [0., 0., 0., 1.],
            size: [2., 4., 6.],
            color: [1., 0., 0.],
        }
    }
    #[test]
    fn box_extents_and_rotation_are_metric() {
        let b = box_object();
        assert_eq!(b.corners()[0], [0., 0., 0.]);
        assert_eq!(b.corners()[7], [2., 4., 6.]);
        let b = SceneBox {
            center: [0.; 3],
            orientation: [
                0.,
                0.,
                std::f32::consts::FRAC_1_SQRT_2,
                std::f32::consts::FRAC_1_SQRT_2,
            ],
            ..b
        };
        let p = b.corners()[0];
        assert!((p[0] - 2.).abs() < 1e-5);
        assert!((p[1] + 1.).abs() < 1e-5);
        assert!(SceneOverlay::new(vec![], vec![b]).is_ok());
    }
    #[test]
    fn identity_is_stable_across_reorder_update_and_replacement() {
        let a = box_object();
        let b = SceneBox {
            id: SceneObjectId(u64::MAX),
            center: [10.; 3],
            ..a
        };
        let first = SceneOverlay::new(vec![], vec![a, b]).unwrap();
        assert_eq!(SceneOverlay::new(vec![], vec![b, a]).unwrap(), first);
        let updated = SceneOverlay::new(
            vec![],
            vec![
                SceneBox {
                    center: [5.; 3],
                    ..a
                },
                b,
            ],
        )
        .unwrap();
        assert_eq!(updated.boxes()[0].id, a.id);
        assert_ne!(updated, first);
        let removed = SceneOverlay::new(vec![], vec![a]).unwrap();
        assert_eq!(removed.boxes().len(), 1);
        assert_eq!(removed.boxes()[0].id, a.id);
    }
    #[test]
    fn rigid_placement_preserves_object_dimensions_and_rotates_orientation() {
        use crate::{
            ArrivalTime, MeasurementTime, TransformBatch, TransformStamped, TransformState,
        };
        let mut tf = TransformState::default();
        let q = std::f64::consts::FRAC_1_SQRT_2;
        tf.apply(TransformBatch {
            is_static: true,
            arrival_time: ArrivalTime(0),
            transforms: vec![TransformStamped {
                measurement_time: MeasurementTime(0),
                frame_id: "world".into(),
                child_frame_id: "sensor".into(),
                translation: [10., 20., 30.],
                rotation: [0., 0., q, q],
            }],
        });
        let input = SceneOverlay::new(vec![], vec![box_object()]).unwrap();
        let placed = input
            .transformed(
                tf.resolve_at("sensor", "world", MeasurementTime(0))
                    .unwrap(),
            )
            .unwrap();
        let b = placed.boxes()[0];
        assert_eq!(b.id, input.boxes()[0].id);
        assert_eq!(b.size, input.boxes()[0].size);
        assert_eq!(b.center, [8., 21., 33.]);
        assert!((b.orientation[2] - q as f32).abs() < 1e-6);
        let original_corners = input.boxes()[0].corners();
        let placed_corners = b.corners();
        let expected = tf
            .transform_points_at("sensor", "world", MeasurementTime(0), &original_corners)
            .unwrap();
        assert!(
            placed_corners
                .iter()
                .zip(expected)
                .all(|(a, b)| a.iter().zip(b).all(|(a, b)| (*a - b).abs() < 1e-5))
        );
    }

    #[test]
    fn validates_all_external_geometry_and_ids_across_object_kinds() {
        let b = box_object();
        let line = SceneLine {
            id: b.id,
            start: [0.; 3],
            end: [1.; 3],
            color: [1.; 3],
        };
        assert!(SceneOverlay::new(vec![line], vec![]).is_ok());
        assert!(SceneOverlay::new(vec![line, line], vec![]).is_err());
        assert!(SceneOverlay::new(vec![line], vec![b]).is_err());
        for bad in [
            SceneBox { size: [0.; 3], ..b },
            SceneBox {
                orientation: [0.; 4],
                ..b
            },
            SceneBox {
                center: [f32::NAN; 3],
                ..b
            },
            SceneBox {
                color: [2.; 3],
                ..b
            },
            SceneBox {
                size: [f32::INFINITY; 3],
                ..b
            },
        ] {
            assert!(SceneOverlay::new(vec![], vec![bad]).is_err());
        }
        assert!(SceneOverlay::new(vec![line; MAX_OVERLAY_LINES + 1], vec![]).is_err());
        assert!(SceneOverlay::new(vec![], vec![b; MAX_OVERLAY_BOXES + 1]).is_err());
    }
}
