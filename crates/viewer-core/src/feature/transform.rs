//! Feature-specific transform history and lookup.

use crate::{ArrivalTime, MeasurementTime, TransformStamped};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    time::Duration,
};

/// One policy controls both normal-playback retention and seek history restoration.
pub const DYNAMIC_TF_HISTORY: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq)]
pub struct TransformBatch {
    pub arrival_time: ArrivalTime,
    pub is_static: bool,
    pub transforms: Vec<TransformStamped>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct RigidTransform {
    translation: [f64; 3],
    rotation: [f64; 4],
}

impl RigidTransform {
    const IDENTITY: Self = Self {
        translation: [0.0; 3],
        rotation: [0.0, 0.0, 0.0, 1.0],
    };

    fn new(translation: [f64; 3], rotation: [f64; 4]) -> Option<Self> {
        let norm = rotation
            .iter()
            .map(|value| value * value)
            .sum::<f64>()
            .sqrt();
        if !norm.is_finite() || norm <= f64::EPSILON {
            return None;
        }
        Some(Self {
            translation,
            rotation: rotation.map(|value| value / norm),
        })
    }

    fn compose(self, child_from_source: Self) -> Self {
        Self {
            translation: add(
                self.translation,
                rotate(self.rotation, child_from_source.translation),
            ),
            rotation: quaternion_multiply(self.rotation, child_from_source.rotation),
        }
    }

    fn inverse(self) -> Self {
        let rotation = [
            -self.rotation[0],
            -self.rotation[1],
            -self.rotation[2],
            self.rotation[3],
        ];
        Self {
            translation: rotate(rotation, scale(self.translation, -1.0)),
            rotation,
        }
    }

    fn transform_point(self, point: [f32; 3]) -> [f32; 3] {
        let point = [
            f64::from(point[0]),
            f64::from(point[1]),
            f64::from(point[2]),
        ];
        let transformed = add(rotate(self.rotation, point), self.translation);
        [
            transformed[0] as f32,
            transformed[1] as f32,
            transformed[2] as f32,
        ]
    }
}

#[derive(Clone, Debug)]
struct Edge {
    parent: String,
    transform: RigidTransform,
    arrival_time: ArrivalTime,
}

#[derive(Clone, Debug, Default)]
pub struct TransformState {
    static_edges: HashMap<String, Edge>,
    dynamic_edges: HashMap<String, BTreeMap<MeasurementTime, Edge>>,
    revision: u64,
}

impl TransformState {
    pub fn clear(&mut self) {
        self.static_edges.clear();
        self.dynamic_edges.clear();
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn apply(&mut self, batch: TransformBatch) {
        self.revision = self.revision.wrapping_add(1);
        for transform in batch.transforms {
            let child = normalize_frame(&transform.child_frame_id);
            let parent = normalize_frame(&transform.frame_id);
            if child.is_empty() || parent.is_empty() || child == parent {
                continue;
            }
            let measurement_time = transform.measurement_time;
            let Some(rigid_transform) =
                RigidTransform::new(transform.translation, transform.rotation)
            else {
                continue;
            };
            let edge = Edge {
                parent,
                transform: rigid_transform,
                arrival_time: batch.arrival_time,
            };
            if batch.is_static {
                if self
                    .static_edges
                    .get(&child)
                    .is_none_or(|current| current.arrival_time <= batch.arrival_time)
                {
                    self.static_edges.insert(child, edge);
                }
            } else {
                let history = self.dynamic_edges.entry(child).or_default();
                if history
                    .get(&measurement_time)
                    .is_none_or(|current| current.arrival_time <= batch.arrival_time)
                {
                    history.insert(measurement_time, edge);
                }
                let history_start = batch.arrival_time.0.saturating_sub(
                    i64::try_from(DYNAMIC_TF_HISTORY.as_nanos()).unwrap_or(i64::MAX),
                );
                history.retain(|_, edge| edge.arrival_time.0 >= history_start);
            }
        }
    }

    pub fn clear_dynamic(&mut self) {
        self.dynamic_edges.clear();
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn static_len(&self) -> usize {
        self.static_edges.len()
    }

    pub fn dynamic_len(&self) -> usize {
        self.dynamic_edges.len()
    }

    pub fn transform_points(
        &self,
        source_frame: &str,
        target_frame: &str,
        points: &[[f32; 3]],
    ) -> Option<Vec<[f32; 3]>> {
        let transform = self.resolve(source_frame, target_frame, None)?;
        Some(transform_points(transform, points))
    }

    /// Resolves dynamic edges at or immediately before `measurement_time`.
    /// Static edges are timeless. No future dynamic sample is used.
    pub fn transform_points_at(
        &self,
        source_frame: &str,
        target_frame: &str,
        measurement_time: MeasurementTime,
        points: &[[f32; 3]],
    ) -> Option<Vec<[f32; 3]>> {
        let transform = self.resolve(source_frame, target_frame, Some(measurement_time))?;
        Some(transform_points(transform, points))
    }

    fn resolve(
        &self,
        source_frame: &str,
        target_frame: &str,
        measurement_time: Option<MeasurementTime>,
    ) -> Option<RigidTransform> {
        let source = normalize_frame(source_frame);
        let target = normalize_frame(target_frame);
        if source == target {
            return Some(RigidTransform::IDENTITY);
        }
        let source_ancestors = self.ancestors(&source, measurement_time)?;
        let mut current = target;
        let mut common_from_target = RigidTransform::IDENTITY;
        let mut visited = HashSet::new();
        for _ in 0..128 {
            if !visited.insert(current.clone()) {
                return None;
            }
            if let Some(common_from_source) = source_ancestors.get(&current) {
                return Some(common_from_target.inverse().compose(*common_from_source));
            }
            let edge = self.edge(&current, measurement_time)?;
            common_from_target = edge.transform.compose(common_from_target);
            current = edge.parent.clone();
        }
        None
    }

    fn ancestors(
        &self,
        frame: &str,
        measurement_time: Option<MeasurementTime>,
    ) -> Option<HashMap<String, RigidTransform>> {
        let mut result = HashMap::new();
        let mut visited = HashSet::new();
        let mut current = frame.to_owned();
        let mut current_from_source = RigidTransform::IDENTITY;
        for _ in 0..128 {
            if !visited.insert(current.clone()) {
                return None;
            }
            result.insert(current.clone(), current_from_source);
            let Some(edge) = self.edge(&current, measurement_time) else {
                return Some(result);
            };
            current_from_source = edge.transform.compose(current_from_source);
            current = edge.parent.clone();
        }
        None
    }

    fn edge(&self, child: &str, measurement_time: Option<MeasurementTime>) -> Option<&Edge> {
        self.dynamic_edges
            .get(child)
            .and_then(|history| match measurement_time {
                Some(time) => history.range(..=time).next_back().map(|(_, edge)| edge),
                None => history.last_key_value().map(|(_, edge)| edge),
            })
            .or_else(|| self.static_edges.get(child))
    }
}

fn transform_points(transform: RigidTransform, points: &[[f32; 3]]) -> Vec<[f32; 3]> {
    points
        .iter()
        .copied()
        .map(|point| transform.transform_point(point))
        .collect()
}

fn normalize_frame(frame: &str) -> String {
    frame.trim().trim_start_matches('/').to_owned()
}

fn add(left: [f64; 3], right: [f64; 3]) -> [f64; 3] {
    [left[0] + right[0], left[1] + right[1], left[2] + right[2]]
}

fn scale(value: [f64; 3], scalar: f64) -> [f64; 3] {
    [value[0] * scalar, value[1] * scalar, value[2] * scalar]
}

fn quaternion_multiply(left: [f64; 4], right: [f64; 4]) -> [f64; 4] {
    let [lx, ly, lz, lw] = left;
    let [rx, ry, rz, rw] = right;
    [
        lw * rx + lx * rw + ly * rz - lz * ry,
        lw * ry - lx * rz + ly * rw + lz * rx,
        lw * rz + lx * ry - ly * rx + lz * rw,
        lw * rw - lx * rx - ly * ry - lz * rz,
    ]
}

fn rotate(rotation: [f64; 4], point: [f64; 3]) -> [f64; 3] {
    let vector = [point[0], point[1], point[2], 0.0];
    let inverse = [-rotation[0], -rotation[1], -rotation[2], rotation[3]];
    let rotated = quaternion_multiply(quaternion_multiply(rotation, vector), inverse);
    [rotated[0], rotated[1], rotated[2]]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MeasurementTime;

    fn transform(parent: &str, child: &str, translation: [f64; 3]) -> TransformStamped {
        timed_transform(0, parent, child, translation)
    }

    fn timed_transform(
        time: i64,
        parent: &str,
        child: &str,
        translation: [f64; 3],
    ) -> TransformStamped {
        TransformStamped {
            measurement_time: MeasurementTime(time),
            frame_id: parent.into(),
            child_frame_id: child.into(),
            translation,
            rotation: [0.0, 0.0, 0.0, 1.0],
        }
    }

    #[test]
    fn resolves_chain_and_inverse() {
        let mut state = TransformState::default();
        state.apply(TransformBatch {
            arrival_time: ArrivalTime(1),
            is_static: true,
            transforms: vec![
                transform("base", "sensor", [1.0, 0.0, 0.5]),
                transform("odom", "base", [2.0, 3.0, 0.0]),
            ],
        });
        let in_odom = state
            .transform_points("sensor", "odom", &[[0.0, 0.0, 0.0]])
            .unwrap();
        assert_eq!(in_odom, vec![[3.0, 3.0, 0.5]]);
        let in_sensor = state.transform_points("odom", "sensor", &in_odom).unwrap();
        assert_eq!(in_sensor, vec![[0.0, 0.0, 0.0]]);
    }

    #[test]
    fn seek_clear_preserves_static_only() {
        let mut state = TransformState::default();
        state.apply(TransformBatch {
            arrival_time: ArrivalTime(1),
            is_static: true,
            transforms: vec![transform("base", "sensor", [0.0; 3])],
        });
        state.apply(TransformBatch {
            arrival_time: ArrivalTime(2),
            is_static: false,
            transforms: vec![transform("odom", "base", [0.0; 3])],
        });
        state.clear_dynamic();
        assert_eq!(state.static_len(), 1);
        assert_eq!(state.dynamic_len(), 0);
    }

    #[test]
    fn resolves_dynamic_transform_at_measurement_time_without_using_future() {
        let mut state = TransformState::default();
        for (arrival, measurement, x) in [(10, 100, 1.0), (20, 200, 2.0)] {
            state.apply(TransformBatch {
                arrival_time: ArrivalTime(arrival),
                is_static: false,
                transforms: vec![timed_transform(measurement, "odom", "base", [x, 0.0, 0.0])],
            });
        }

        assert!(
            state
                .transform_points_at("base", "odom", MeasurementTime(99), &[[0.0; 3]])
                .is_none()
        );
        assert_eq!(
            state
                .transform_points_at("base", "odom", MeasurementTime(150), &[[0.0; 3]])
                .unwrap(),
            vec![[1.0, 0.0, 0.0]]
        );
        assert_eq!(
            state
                .transform_points_at("base", "odom", MeasurementTime(200), &[[0.0; 3]])
                .unwrap(),
            vec![[2.0, 0.0, 0.0]]
        );
    }

    #[test]
    fn dynamic_retention_uses_the_same_time_window_as_seek_restore() {
        let mut state = TransformState::default();
        let history_ns = i64::try_from(DYNAMIC_TF_HISTORY.as_nanos()).unwrap();
        state.apply(TransformBatch {
            arrival_time: ArrivalTime(0),
            is_static: false,
            transforms: vec![timed_transform(0, "odom", "base", [1.0, 0.0, 0.0])],
        });
        state.apply(TransformBatch {
            arrival_time: ArrivalTime(history_ns + 1),
            is_static: false,
            transforms: vec![timed_transform(
                history_ns + 1,
                "odom",
                "base",
                [2.0, 0.0, 0.0],
            )],
        });

        assert!(
            state
                .transform_points_at("base", "odom", MeasurementTime(0), &[[0.0; 3]])
                .is_none(),
            "normal playback must not retain more dynamic TF history than seek restores"
        );
        assert_eq!(
            state
                .transform_points_at("base", "odom", MeasurementTime(history_ns + 1), &[[0.0; 3]],)
                .unwrap(),
            vec![[2.0, 0.0, 0.0]]
        );
    }
}
