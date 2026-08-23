use crate::{BevState, PointCloudState, TelemetryState, TransformState};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BevSnapshot<'a> {
    pub revision: u64,
    pub path: &'a [[f32; 2]],
}

pub struct BevFrameBuilder<'a> {
    state: &'a BevState,
}

impl<'a> BevFrameBuilder<'a> {
    pub fn new(state: &'a BevState) -> Self {
        Self { state }
    }

    pub fn build(self) -> BevSnapshot<'a> {
        BevSnapshot {
            revision: self.state.revision(),
            path: self
                .state
                .latest()
                .map_or(&[], |frame| frame.points.as_slice()),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SceneTfError {
    pub source_frame: String,
    pub target_frame: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SceneDiagnostics {
    pub tf_misses: u64,
    pub last_tf_route: Option<String>,
    pub current_tf_error: Option<SceneTfError>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SceneSnapshot<'a> {
    pub revision: u64,
    pub cloud_revision: u64,
    pub ego_position: [f32; 2],
    pub ego_yaw: f32,
    pub path: &'a [[f32; 2]],
    pub cloud: &'a [[f32; 3]],
    pub diagnostics: SceneDiagnostics,
}

#[derive(Clone, Debug, Default)]
pub struct ScenePresentationState {
    source_revision: Option<u64>,
    transform_revision: u64,
    target_frame: String,
    transformed_cloud: Vec<[f32; 3]>,
    visible_revision: u64,
    visible_cloud: Vec<[f32; 3]>,
    accumulate: bool,
    diagnostics: SceneDiagnostics,
}

const MAX_VISIBLE_SCENE_POINTS: usize = 65_536;

impl ScenePresentationState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn diagnostics(&self) -> &SceneDiagnostics {
        &self.diagnostics
    }

    pub fn build<'a>(
        &'a mut self,
        path: &'a BevState,
        telemetry: &'a TelemetryState,
        point_cloud: &'a PointCloudState,
        transforms: &'a TransformState,
        accumulate: bool,
    ) -> SceneSnapshot<'a> {
        let telemetry = telemetry.latest();
        let target_frame = telemetry.map_or("odom", |frame| frame.frame_id.as_str());
        let point_cloud_revision = point_cloud.revision();
        let transform_revision = transforms.revision();
        let source_changed =
            self.source_revision != Some(point_cloud_revision) || self.target_frame != target_frame;
        let retry_missing_tf = self.diagnostics.current_tf_error.is_some()
            && self.transform_revision != transform_revision;
        let transformed_changed = source_changed || retry_missing_tf;
        if transformed_changed {
            self.source_revision = Some(point_cloud_revision);
            self.transform_revision = transform_revision;
            self.target_frame.clear();
            self.target_frame.push_str(target_frame);
            if let Some(frame) = point_cloud.latest() {
                let source_frame = frame.frame_id.clone();
                match transforms.transform_points_at(
                    &source_frame,
                    target_frame,
                    frame.measurement_time,
                    &frame.points,
                ) {
                    Some(points) => {
                        self.transformed_cloud = points;
                        self.diagnostics.last_tf_route =
                            Some(format!("{source_frame} → {target_frame}"));
                        self.diagnostics.current_tf_error = None;
                    }
                    None => {
                        self.transformed_cloud.clear();
                        if source_changed {
                            self.diagnostics.tf_misses =
                                self.diagnostics.tf_misses.saturating_add(1);
                        }
                        self.diagnostics.current_tf_error = Some(SceneTfError {
                            source_frame,
                            target_frame: target_frame.to_owned(),
                        });
                    }
                }
            } else {
                self.transformed_cloud.clear();
                self.diagnostics.current_tf_error = None;
            }
        }

        self.update_visible_cloud(transformed_changed, accumulate);

        let telemetry_revision = telemetry.map_or(0, |frame| frame.arrival_time.0 as u64);
        SceneSnapshot {
            revision: path.revision().rotate_left(17) ^ telemetry_revision,
            cloud_revision: self.visible_revision,
            ego_position: telemetry.map_or([0.0, 0.0], |frame| {
                [frame.position_x as f32, frame.position_y as f32]
            }),
            ego_yaw: telemetry.map_or(0.0, |frame| frame.yaw_radians as f32),
            path: path.latest().map_or(&[], |frame| frame.points.as_slice()),
            cloud: &self.visible_cloud,
            diagnostics: self.diagnostics.clone(),
        }
    }

    fn update_visible_cloud(&mut self, transformed_changed: bool, accumulate: bool) {
        let mode_changed = self.accumulate != accumulate;
        let mut visible_changed = false;
        if transformed_changed {
            if self.transformed_cloud.is_empty() {
                if !accumulate && !self.visible_cloud.is_empty() {
                    self.visible_cloud.clear();
                    visible_changed = true;
                }
            } else {
                if !accumulate {
                    self.visible_cloud.clear();
                }
                self.visible_cloud
                    .extend_from_slice(&self.transformed_cloud);
                visible_changed = true;
            }
        } else if mode_changed && !accumulate {
            self.visible_cloud.clear();
            self.visible_cloud
                .extend_from_slice(&self.transformed_cloud);
            visible_changed = true;
        }
        if self.visible_cloud.len() > MAX_VISIBLE_SCENE_POINTS {
            let excess = self.visible_cloud.len() - MAX_VISIBLE_SCENE_POINTS;
            self.visible_cloud.drain(..excess);
            visible_changed = true;
        }
        self.accumulate = accumulate;
        if visible_changed {
            self.visible_revision = self.visible_revision.wrapping_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ArrivalTime, BevPathFrame, MeasurementTime, PointCloudFrame, TelemetryFrame,
        TransformBatch, TransformStamped,
    };

    fn telemetry(arrival_time: i64, position: [f64; 2]) -> TelemetryFrame {
        TelemetryFrame {
            measurement_time: MeasurementTime(arrival_time),
            arrival_time: ArrivalTime(arrival_time),
            frame_id: "odom".into(),
            child_frame_id: "base_link".into(),
            position_x: position[0],
            position_y: position[1],
            yaw_radians: 0.25,
            forward_velocity: 0.0,
            speed: 0.0,
            yaw_rate: 0.0,
        }
    }

    fn scan_transform(x: f64, y: f64, z: f64) -> TransformBatch {
        TransformBatch {
            arrival_time: ArrivalTime(1),
            is_static: true,
            transforms: vec![TransformStamped {
                measurement_time: MeasurementTime(0),
                frame_id: "odom".into(),
                child_frame_id: "scan".into(),
                translation: [x, y, z],
                rotation: [0.0, 0.0, 0.0, 1.0],
            }],
        }
    }

    #[test]
    fn builders_select_current_feature_values() {
        let mut path = BevState::default();
        path.apply(BevPathFrame {
            measurement_time: MeasurementTime(1),
            arrival_time: ArrivalTime(2),
            frame_id: "base_link".into(),
            points: vec![[0.0, 1.0]],
        });
        let mut odometry = TelemetryState::default();
        odometry.apply(telemetry(3, [4.0, 5.0]));
        let mut point_cloud = PointCloudState::default();
        point_cloud.apply(PointCloudFrame {
            measurement_time: MeasurementTime(1),
            arrival_time: ArrivalTime(4),
            frame_id: "odom".into(),
            points: vec![[1.0, 2.0, 3.0]],
        });

        let bev = BevFrameBuilder::new(&path).build();
        assert_eq!(bev.path, [[0.0, 1.0]]);
        let mut builder = ScenePresentationState::new();
        let transforms = TransformState::default();
        let scene = builder.build(&path, &odometry, &point_cloud, &transforms, true);
        assert_eq!(scene.ego_position, [4.0, 5.0]);
        assert_eq!(scene.ego_yaw, 0.25);
        assert_eq!(scene.path, bev.path);
        assert_eq!(scene.cloud, [[1.0, 2.0, 3.0]]);
    }

    #[test]
    fn transforms_each_raw_scan_once_and_keeps_it_fixed_in_world() {
        let mut transforms = TransformState::default();
        transforms.apply(scan_transform(2.0, 3.0, 0.5));
        let mut point_cloud = PointCloudState::default();
        point_cloud.apply(PointCloudFrame {
            measurement_time: MeasurementTime(10),
            arrival_time: ArrivalTime(2),
            frame_id: "scan".into(),
            points: vec![[1.0, 0.0, 0.0]],
        });
        let mut builder = ScenePresentationState::new();
        let path = BevState::default();
        let mut odometry = TelemetryState::default();
        let first = builder.build(&path, &odometry, &point_cloud, &transforms, true);
        assert_eq!(first.cloud, [[3.0, 3.0, 0.5]]);
        let revision = first.cloud_revision;
        assert_eq!(
            first.diagnostics.last_tf_route.as_deref(),
            Some("scan → odom")
        );

        odometry.apply(telemetry(20, [100.0, -50.0]));
        let second = builder.build(&path, &odometry, &point_cloud, &transforms, true);
        assert_eq!(second.cloud, [[3.0, 3.0, 0.5]]);
        assert_eq!(second.cloud_revision, revision);
        let raw = point_cloud.latest().unwrap();
        assert_eq!(raw.frame_id, "scan");
        assert_eq!(raw.points, [[1.0, 0.0, 0.0]]);
    }

    #[test]
    fn retries_a_missing_scan_transform_when_tf_arrives() {
        let path = BevState::default();
        let odometry = TelemetryState::default();
        let mut point_cloud = PointCloudState::default();
        point_cloud.apply(PointCloudFrame {
            measurement_time: MeasurementTime(10),
            arrival_time: ArrivalTime(1),
            frame_id: "scan".into(),
            points: vec![[1.0, 0.0, 0.0]],
        });
        let mut builder = ScenePresentationState::new();
        let mut transforms = TransformState::default();
        let missing = builder.build(&path, &odometry, &point_cloud, &transforms, false);
        assert!(missing.cloud.is_empty());
        assert_eq!(missing.diagnostics.tf_misses, 1);

        transforms.apply(scan_transform(2.0, 0.0, 0.0));
        let recovered = builder.build(&path, &odometry, &point_cloud, &transforms, false);
        assert_eq!(recovered.cloud, [[3.0, 0.0, 0.0]]);
        assert_eq!(recovered.diagnostics.tf_misses, 1);
        assert!(recovered.diagnostics.current_tf_error.is_none());
    }

    #[test]
    fn presentation_owns_bounded_accumulation_and_latest_only_mode() {
        let path = BevState::default();
        let odometry = TelemetryState::default();
        let transforms = TransformState::default();
        let mut point_cloud = PointCloudState::default();
        let mut presentation = ScenePresentationState::new();

        point_cloud.apply(PointCloudFrame {
            measurement_time: MeasurementTime(1),
            arrival_time: ArrivalTime(1),
            frame_id: "odom".into(),
            points: vec![[1.0, 0.0, 0.0], [2.0, 0.0, 0.0]],
        });
        assert_eq!(
            presentation
                .build(&path, &odometry, &point_cloud, &transforms, true)
                .cloud
                .len(),
            2
        );

        point_cloud.apply(PointCloudFrame {
            measurement_time: MeasurementTime(2),
            arrival_time: ArrivalTime(2),
            frame_id: "odom".into(),
            points: vec![[3.0, 0.0, 0.0]],
        });
        let accumulated = presentation.build(&path, &odometry, &point_cloud, &transforms, true);
        assert_eq!(accumulated.cloud.len(), 3);
        let revision = accumulated.cloud_revision;

        let unchanged = presentation.build(&path, &odometry, &point_cloud, &transforms, true);
        assert_eq!(unchanged.cloud.len(), 3);
        assert_eq!(unchanged.cloud_revision, revision);

        let latest = presentation.build(&path, &odometry, &point_cloud, &transforms, false);
        assert_eq!(latest.cloud, [[3.0, 0.0, 0.0]]);
        presentation.reset();
        assert!(
            presentation
                .build(
                    &path,
                    &odometry,
                    &PointCloudState::default(),
                    &transforms,
                    true
                )
                .cloud
                .is_empty()
        );
    }

    #[test]
    fn presentation_history_never_exceeds_the_fixed_point_budget() {
        let path = BevState::default();
        let odometry = TelemetryState::default();
        let transforms = TransformState::default();
        let mut point_cloud = PointCloudState::default();
        point_cloud.apply(PointCloudFrame {
            measurement_time: MeasurementTime(1),
            arrival_time: ArrivalTime(1),
            frame_id: "odom".into(),
            points: (0..MAX_VISIBLE_SCENE_POINTS + 17)
                .map(|index| [index as f32, 0.0, 0.0])
                .collect(),
        });
        let mut presentation = ScenePresentationState::new();
        let snapshot = presentation.build(&path, &odometry, &point_cloud, &transforms, true);
        assert_eq!(snapshot.cloud.len(), MAX_VISIBLE_SCENE_POINTS);
        assert_eq!(snapshot.cloud[0][0], 17.0);
    }
}
