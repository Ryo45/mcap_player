//! CPU-owned BEV and scene presentation state.

use crate::{
    FrameTransformError, MeasurementTime, PathProjectionPolicy, PathState, PlatformGeometry,
    PointCloudState, TelemetryState, TransformState,
};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BevSnapshot<'a> {
    pub revision: u64,
    /// right/forward metres in the platform reference frame.
    pub path: &'a [[f32; 2]],
    pub footprint: Option<[f32; 4]>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SceneDiagnostics {
    pub extension_error: Option<String>,
    pub tf_misses: u64,
    pub point_cloud_color_error: Option<&'static str>,
    pub last_tf_route: Option<String>,
    pub current_tf_error: Option<FrameTransformError>,
    pub path_tf_error: Option<FrameTransformError>,
    pub path_geometry_error: Option<&'static str>,
    pub platform_tf_error: Option<FrameTransformError>,
    pub platform_geometry_error: Option<&'static str>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SceneSnapshot<'a> {
    pub revision: u64,
    pub cloud_revision: u64,
    pub overlay_revision: u64,
    pub overlay: &'a [crate::SceneLine],
    pub focus: [f32; 3],
    pub forward: [f32; 3],
    pub view_radius: f32,
    pub platform: Option<[[f32; 3]; 8]>,
    pub path: &'a [[f32; 3]],
    pub bev: BevSnapshot<'a>,
    pub cloud: &'a [[f32; 3]],
    /// Linear RGBA, aligned with cloud positions.
    pub cloud_colors: &'a [[f32; 4]],
    pub diagnostics: SceneDiagnostics,
}

#[derive(Clone, Debug, PartialEq)]
struct PathPlacementKey {
    path_revision: u64,
    transform_revision: u64,
    reference_frame: String,
    reference_time: MeasurementTime,
    policy: PathProjectionPolicy,
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
    coloring: Option<crate::PointCloudColoring>,
    transformed_colors: Vec<[f32; 4]>,
    visible_colors: Vec<[f32; 4]>,
    diagnostics: SceneDiagnostics,
    path_key: Option<PathPlacementKey>,
    world_path: Vec<[f32; 3]>,
    bev_path: Vec<[f32; 2]>,
    platform: Option<[[f32; 3]; 8]>,
    geometry_revision: u64,
    focus: [f32; 3],
    forward: [f32; 3],
    footprint: Option<[f32; 4]>,
}

/// Source facts and independent presentation choices; no renderer-specific scale.
pub struct ScenePresentationInput<'a> {
    pub path: &'a PathState,
    pub telemetry: &'a TelemetryState,
    pub point_cloud: &'a PointCloudState,
    pub transforms: &'a TransformState,
    pub platform: Option<&'a PlatformGeometry>,
    pub path_projection: &'a PathProjectionPolicy,
    pub accumulate: bool,
    pub coloring: crate::PointCloudColoring,
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

    pub fn build(&mut self, input: ScenePresentationInput<'_>) -> SceneSnapshot<'_> {
        let ScenePresentationInput {
            path,
            telemetry,
            point_cloud,
            transforms,
            platform,
            path_projection,
            accumulate,
            coloring,
        } = input;
        let invalid_coloring = !coloring.is_valid();
        let coloring = if invalid_coloring {
            crate::PointCloudColoring {
                mode: coloring.mode,
                ..Default::default()
            }
        } else {
            coloring
        };
        let telemetry = telemetry.latest();
        let target_frame = path_projection.fixed_frame.as_str();
        let color_changed = self.coloring != Some(coloring);
        if color_changed {
            self.visible_cloud.clear();
            self.visible_colors.clear();
            self.coloring = Some(coloring);
            self.visible_revision = self.visible_revision.wrapping_add(1);
        }
        // Existing accumulated positions are never mixed across different world frames.
        if self.target_frame != target_frame && !self.visible_cloud.is_empty() {
            self.visible_cloud.clear();
            self.visible_colors.clear();
            self.visible_revision = self.visible_revision.wrapping_add(1);
        }
        let point_cloud_revision = point_cloud.revision();
        let transform_revision = transforms.revision();
        let source_changed =
            self.source_revision != Some(point_cloud_revision) || self.target_frame != target_frame;
        let retry_missing_tf = self.diagnostics.current_tf_error.is_some()
            && self.transform_revision != transform_revision;
        let transformed_changed = source_changed || retry_missing_tf || color_changed;
        self.diagnostics.point_cloud_color_error = point_cloud.latest().and_then(|frame| {
            if invalid_coloring {
                Some("Invalid intensity range; using 0..255")
            } else {
                coloring.missing_attribute(&frame.attributes)
            }
        });
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
                        self.transformed_colors = coloring.colors(&frame.attributes, points.len());
                        self.transformed_cloud = points;
                        self.diagnostics.last_tf_route =
                            Some(format!("{source_frame} → {target_frame}"));
                        self.diagnostics.current_tf_error = None;
                    }
                    None => {
                        self.transformed_cloud.clear();
                        self.transformed_colors.clear();
                        if source_changed {
                            self.diagnostics.tf_misses =
                                self.diagnostics.tf_misses.saturating_add(1);
                        }
                        self.diagnostics.current_tf_error = Some(FrameTransformError {
                            source_frame,
                            target_frame: target_frame.to_owned(),
                        });
                    }
                }
            } else {
                self.transformed_cloud.clear();
                self.transformed_colors.clear();
                self.diagnostics.current_tf_error = None;
            }
        }

        self.update_visible_cloud(transformed_changed, accumulate);

        let reference = platform.map_or_else(
            || telemetry.map_or("base_link", |t| t.child_frame_id.as_str()),
            PlatformGeometry::reference_frame,
        );
        let time = telemetry.map_or_else(
            || {
                point_cloud.latest().map_or_else(
                    || {
                        path.latest()
                            .map_or(MeasurementTime(0), |p| p.measurement_time)
                    },
                    |p| p.measurement_time,
                )
            },
            |t| t.measurement_time,
        );
        let placement_changed = self.path_key.as_ref().is_none_or(|k| {
            k.path_revision != path.revision()
                || k.transform_revision != transforms.revision()
                || k.reference_frame != reference
                || k.reference_time != time
                || k.policy != *path_projection
        });
        let mut path_changed = false;
        if placement_changed {
            self.path_key = Some(PathPlacementKey {
                path_revision: path.revision(),
                transform_revision: transforms.revision(),
                reference_frame: reference.into(),
                reference_time: time,
                policy: path_projection.clone(),
            });
            self.diagnostics.path_tf_error = None;
            self.diagnostics.path_geometry_error = None;
            let mut world_path = Vec::new();
            let mut bev_path = Vec::new();
            if let Some(p) = path.latest() {
                match path_projection.positions_in_fixed_frame(p, transforms) {
                    Ok(points) => {
                        world_path = points.into_iter().map(|v| v.map(|x| x as f32)).collect()
                    }
                    Err(e) => self.diagnostics.path_tf_error = Some(e),
                }
                match path_projection.positions_in_frame(p, transforms, reference, time) {
                    Ok(points) => {
                        bev_path = points
                            .into_iter()
                            .map(|[x, y, _]| [-y as f32, x as f32])
                            .collect()
                    }
                    Err(e) => self.diagnostics.path_tf_error = Some(e),
                }
            }
            if !world_path
                .iter()
                .flatten()
                .chain(bev_path.iter().flatten())
                .all(|v| v.is_finite())
            {
                world_path.clear();
                bev_path.clear();
                self.diagnostics.path_geometry_error =
                    Some("Path exceeds the renderer coordinate range");
            }
            path_changed = self.world_path != world_path || self.bev_path != bev_path;
            self.world_path = world_path;
            self.bev_path = bev_path;
        }
        let mut body = None;
        self.diagnostics.platform_tf_error = None;
        self.diagnostics.platform_geometry_error = None;
        let mut focus = telemetry
            .filter(|t| t.frame_id.trim_start_matches('/') == target_frame.trim_start_matches('/'))
            .map_or([0.0; 3], |t| {
                [t.position_x as f32, t.position_y as f32, 0.0]
            });
        let mut forward = telemetry.map_or([1.0, 0.0, 0.0], |t| {
            [t.yaw_radians.cos() as f32, t.yaw_radians.sin() as f32, 0.0]
        });
        let mut view_radius = 0.5;
        if let Some(g) = platform {
            let mut corners = g.corners().to_vec();
            corners.extend([[0.0; 3], [1.0, 0.0, 0.0]]);
            match transforms.transform_positions_at(
                g.reference_frame(),
                target_frame,
                time,
                &corners,
            ) {
                Some(points) if points.iter().flatten().all(|v| (*v as f32).is_finite()) => {
                    let bounds = std::array::from_fn(|i| points[i].map(|v| v as f32));
                    focus = std::array::from_fn(|axis| {
                        (bounds.iter().map(|p| f64::from(p[axis])).sum::<f64>() / 8.0) as f32
                    });
                    forward = std::array::from_fn(|i| (points[9][i] - points[8][i]) as f32);
                    view_radius =
                        (g.extent()[0].hypot(g.extent()[1]).hypot(g.extent()[2]) / 2.0) as f32;
                    body = Some(bounds);
                }
                Some(_) => {
                    self.diagnostics.platform_geometry_error =
                        Some("Platform exceeds the renderer coordinate range")
                }
                None => {
                    self.diagnostics.platform_tf_error = Some(FrameTransformError {
                        source_frame: g.reference_frame().into(),
                        target_frame: target_frame.into(),
                    })
                }
            }
        }
        let footprint = platform.and_then(|g| {
            let min = g.min();
            let max = g.max();
            let bounds = [-max[1] as f32, min[0] as f32, -min[1] as f32, max[0] as f32];
            if bounds.iter().all(|v| v.is_finite()) {
                Some(bounds)
            } else {
                self.diagnostics.platform_geometry_error =
                    Some("Platform exceeds the renderer coordinate range");
                None
            }
        });
        if self.footprint != footprint
            || path_changed
            || self.platform != body
            || self.focus != focus
            || self.forward != forward
        {
            self.geometry_revision = self.geometry_revision.wrapping_add(1);
            self.platform = body;
            self.focus = focus;
            self.forward = forward;
            self.footprint = footprint;
        }
        SceneSnapshot {
            revision: self.geometry_revision,
            cloud_revision: self.visible_revision,
            overlay_revision: 0,
            overlay: &[],
            focus,
            forward,
            view_radius,
            platform: body,
            path: &self.world_path,
            bev: BevSnapshot {
                revision: self.geometry_revision,
                path: &self.bev_path,
                footprint,
            },
            cloud: &self.visible_cloud,
            cloud_colors: &self.visible_colors,
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
                    self.visible_colors.clear();
                    visible_changed = true;
                }
            } else {
                if !accumulate {
                    self.visible_cloud.clear();
                    self.visible_colors.clear();
                }
                self.visible_cloud
                    .extend_from_slice(&self.transformed_cloud);
                self.visible_colors
                    .extend_from_slice(&self.transformed_colors);
                visible_changed = true;
            }
        } else if mode_changed && !accumulate {
            self.visible_cloud.clear();
            self.visible_colors.clear();
            self.visible_cloud
                .extend_from_slice(&self.transformed_cloud);
            self.visible_colors
                .extend_from_slice(&self.transformed_colors);
            visible_changed = true;
        }
        if self.visible_cloud.len() > MAX_VISIBLE_SCENE_POINTS {
            let excess = self.visible_cloud.len() - MAX_VISIBLE_SCENE_POINTS;
            self.visible_cloud.drain(..excess);
            self.visible_colors.drain(..excess);
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
        ArrivalTime, MeasurementTime, PathFrame, PointCloudFrame, TelemetryFrame, TransformBatch,
        TransformStamped,
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
        let mut path = PathState::default();
        path.apply(PathFrame {
            measurement_time: MeasurementTime(1),
            arrival_time: ArrivalTime(2),
            frame_id: "base_link".into(),
            points: vec![[1.0, -(0.0), 0.0]],
        });
        let mut odometry = TelemetryState::default();
        odometry.apply(telemetry(3, [4.0, 5.0]));
        let mut point_cloud = PointCloudState::default();
        point_cloud.apply(PointCloudFrame {
            attributes: Default::default(),
            measurement_time: MeasurementTime(1),
            arrival_time: ArrivalTime(4),
            frame_id: "odom".into(),
            points: vec![[1.0, 2.0, 3.0]],
        });

        let mut builder = ScenePresentationState::new();
        let transforms = TransformState::default();
        let scene = builder.build(ScenePresentationInput {
            path: &path,
            telemetry: &odometry,
            point_cloud: &point_cloud,
            transforms: &transforms,
            platform: None,
            path_projection: &PathProjectionPolicy::default(),
            accumulate: true,
            coloring: Default::default(),
        });
        assert_eq!(scene.focus, [4.0, 5.0, 0.0]);
        assert!(scene.path.is_empty());
        assert!(scene.diagnostics.path_tf_error.is_some());
        assert_eq!(scene.cloud, [[1.0, 2.0, 3.0]]);
    }

    #[test]
    fn transforms_each_raw_scan_once_and_keeps_it_fixed_in_world() {
        let mut transforms = TransformState::default();
        transforms.apply(scan_transform(2.0, 3.0, 0.5));
        let mut point_cloud = PointCloudState::default();
        point_cloud.apply(PointCloudFrame {
            attributes: Default::default(),
            measurement_time: MeasurementTime(10),
            arrival_time: ArrivalTime(2),
            frame_id: "scan".into(),
            points: vec![[1.0, 0.0, 0.0]],
        });
        let mut builder = ScenePresentationState::new();
        let path = PathState::default();
        let mut odometry = TelemetryState::default();
        let first = builder.build(ScenePresentationInput {
            path: &path,
            telemetry: &odometry,
            point_cloud: &point_cloud,
            transforms: &transforms,
            platform: None,
            path_projection: &PathProjectionPolicy::default(),
            accumulate: true,
            coloring: Default::default(),
        });
        assert_eq!(first.cloud, [[3.0, 3.0, 0.5]]);
        let revision = first.cloud_revision;
        assert_eq!(
            first.diagnostics.last_tf_route.as_deref(),
            Some("scan → odom")
        );

        odometry.apply(telemetry(20, [100.0, -50.0]));
        let second = builder.build(ScenePresentationInput {
            path: &path,
            telemetry: &odometry,
            point_cloud: &point_cloud,
            transforms: &transforms,
            platform: None,
            path_projection: &PathProjectionPolicy::default(),
            accumulate: true,
            coloring: Default::default(),
        });
        assert_eq!(second.cloud, [[3.0, 3.0, 0.5]]);
        assert_eq!(second.cloud_revision, revision);
        let raw = point_cloud.latest().unwrap();
        assert_eq!(raw.frame_id, "scan");
        assert_eq!(raw.points, [[1.0, 0.0, 0.0]]);
    }

    #[test]
    fn retries_a_missing_scan_transform_when_tf_arrives() {
        let path = PathState::default();
        let odometry = TelemetryState::default();
        let mut point_cloud = PointCloudState::default();
        point_cloud.apply(PointCloudFrame {
            attributes: Default::default(),
            measurement_time: MeasurementTime(10),
            arrival_time: ArrivalTime(1),
            frame_id: "scan".into(),
            points: vec![[1.0, 0.0, 0.0]],
        });
        let mut builder = ScenePresentationState::new();
        let mut transforms = TransformState::default();
        let missing = builder.build(ScenePresentationInput {
            path: &path,
            telemetry: &odometry,
            point_cloud: &point_cloud,
            transforms: &transforms,
            platform: None,
            path_projection: &PathProjectionPolicy::default(),
            accumulate: false,
            coloring: Default::default(),
        });
        assert!(missing.cloud.is_empty());
        assert_eq!(missing.diagnostics.tf_misses, 1);

        transforms.apply(scan_transform(2.0, 0.0, 0.0));
        let recovered = builder.build(ScenePresentationInput {
            path: &path,
            telemetry: &odometry,
            point_cloud: &point_cloud,
            transforms: &transforms,
            platform: None,
            path_projection: &PathProjectionPolicy::default(),
            accumulate: false,
            coloring: Default::default(),
        });
        assert_eq!(recovered.cloud, [[3.0, 0.0, 0.0]]);
        assert_eq!(recovered.diagnostics.tf_misses, 1);
        assert!(recovered.diagnostics.current_tf_error.is_none());
    }

    #[test]
    fn presentation_owns_bounded_accumulation_and_latest_only_mode() {
        let path = PathState::default();
        let odometry = TelemetryState::default();
        let transforms = TransformState::default();
        let mut point_cloud = PointCloudState::default();
        let mut presentation = ScenePresentationState::new();

        point_cloud.apply(PointCloudFrame {
            attributes: Default::default(),
            measurement_time: MeasurementTime(1),
            arrival_time: ArrivalTime(1),
            frame_id: "odom".into(),
            points: vec![[1.0, 0.0, 0.0], [2.0, 0.0, 0.0]],
        });
        assert_eq!(
            presentation
                .build(ScenePresentationInput {
                    path: &path,
                    telemetry: &odometry,
                    point_cloud: &point_cloud,
                    transforms: &transforms,
                    platform: None,
                    path_projection: &PathProjectionPolicy::default(),
                    accumulate: true,
                    coloring: Default::default(),
                })
                .cloud
                .len(),
            2
        );

        point_cloud.apply(PointCloudFrame {
            attributes: Default::default(),
            measurement_time: MeasurementTime(2),
            arrival_time: ArrivalTime(2),
            frame_id: "odom".into(),
            points: vec![[3.0, 0.0, 0.0]],
        });
        let accumulated = presentation.build(ScenePresentationInput {
            path: &path,
            telemetry: &odometry,
            point_cloud: &point_cloud,
            transforms: &transforms,
            platform: None,
            path_projection: &PathProjectionPolicy::default(),
            accumulate: true,
            coloring: Default::default(),
        });
        assert_eq!(accumulated.cloud.len(), 3);
        let revision = accumulated.cloud_revision;

        let unchanged = presentation.build(ScenePresentationInput {
            path: &path,
            telemetry: &odometry,
            point_cloud: &point_cloud,
            transforms: &transforms,
            platform: None,
            path_projection: &PathProjectionPolicy::default(),
            accumulate: true,
            coloring: Default::default(),
        });
        assert_eq!(unchanged.cloud.len(), 3);
        assert_eq!(unchanged.cloud_revision, revision);

        let latest = presentation.build(ScenePresentationInput {
            path: &path,
            telemetry: &odometry,
            point_cloud: &point_cloud,
            transforms: &transforms,
            platform: None,
            path_projection: &PathProjectionPolicy::default(),
            accumulate: false,
            coloring: Default::default(),
        });
        assert_eq!(latest.cloud, [[3.0, 0.0, 0.0]]);
        presentation.reset();
        assert!(
            presentation
                .build(ScenePresentationInput {
                    path: &path,
                    telemetry: &odometry,
                    point_cloud: &PointCloudState::default(),
                    transforms: &transforms,
                    platform: None,
                    path_projection: &PathProjectionPolicy::default(),
                    accumulate: true,
                    coloring: Default::default(),
                })
                .cloud
                .is_empty()
        );
    }

    #[test]
    fn presentation_history_never_exceeds_the_fixed_point_budget() {
        let path = PathState::default();
        let odometry = TelemetryState::default();
        let transforms = TransformState::default();
        let mut point_cloud = PointCloudState::default();
        point_cloud.apply(PointCloudFrame {
            attributes: Default::default(),
            measurement_time: MeasurementTime(1),
            arrival_time: ArrivalTime(1),
            frame_id: "odom".into(),
            points: (0..MAX_VISIBLE_SCENE_POINTS + 17)
                .map(|index| [index as f32, 0.0, 0.0])
                .collect(),
        });
        let mut presentation = ScenePresentationState::new();
        let snapshot = presentation.build(ScenePresentationInput {
            path: &path,
            telemetry: &odometry,
            point_cloud: &point_cloud,
            transforms: &transforms,
            platform: None,
            path_projection: &PathProjectionPolicy::default(),
            accumulate: true,
            coloring: Default::default(),
        });
        assert_eq!(snapshot.cloud.len(), MAX_VISIBLE_SCENE_POINTS);
        assert_eq!(snapshot.cloud_colors.len(), MAX_VISIBLE_SCENE_POINTS);
        assert_eq!(snapshot.cloud[0][0], 17.0);
    }
}
