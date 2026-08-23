//! Camera overlay projection from semantic feature state.

use crate::{BevPathFrame, CameraFrame, TransformState};
use serde::Deserialize;
use std::{collections::BTreeMap, fmt};

const MIN_CAMERA_DEPTH_METRES: f32 = 0.05;

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct CameraCalibration {
    pub frame_id: String,
    pub projection_model: String,
    pub width: u32,
    pub height: u32,
    pub fx: f64,
    pub fy: f64,
    pub cx: f64,
    pub cy: f64,
    pub distortion_model: String,
    #[serde(default)]
    pub distortion_coefficients: Vec<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct CameraCalibrationSet {
    cameras: BTreeMap<String, CameraCalibration>,
}

#[derive(Clone, Debug, Deserialize)]
struct CalibrationFile {
    version: u32,
    cameras: Vec<CameraCalibration>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CalibrationError {
    InvalidJson(String),
    UnsupportedVersion(u32),
    InvalidCamera(String),
    DuplicateFrame(String),
}

impl fmt::Display for CalibrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson(error) => {
                write!(formatter, "invalid camera calibration JSON: {error}")
            }
            Self::UnsupportedVersion(version) => {
                write!(
                    formatter,
                    "unsupported camera calibration version {version}"
                )
            }
            Self::InvalidCamera(frame) => {
                write!(formatter, "invalid pinhole calibration for frame {frame:?}")
            }
            Self::DuplicateFrame(frame) => {
                write!(
                    formatter,
                    "duplicate camera calibration for frame {frame:?}"
                )
            }
        }
    }
}

impl std::error::Error for CalibrationError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectionError {
    MissingCalibration(String),
    MissingTransform { source: String, target: String },
}

impl fmt::Display for ProjectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingCalibration(frame) => {
                write!(formatter, "no camera calibration for {frame}")
            }
            Self::MissingTransform { source, target } => {
                write!(formatter, "TF unavailable: {source} → {target}")
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProjectedPlan {
    /// `None` breaks the polyline when a path point is behind the camera.
    pub points: Vec<Option<[f32; 2]>>,
    pub visible_points: usize,
}

impl CameraCalibrationSet {
    pub fn from_json(json: &str) -> Result<Self, CalibrationError> {
        let file: CalibrationFile = serde_json::from_str(json)
            .map_err(|error| CalibrationError::InvalidJson(error.to_string()))?;
        if file.version != 1 {
            return Err(CalibrationError::UnsupportedVersion(file.version));
        }
        let mut cameras = BTreeMap::new();
        for camera in file.cameras {
            if !valid_calibration(&camera) {
                return Err(CalibrationError::InvalidCamera(camera.frame_id));
            }
            let frame = normalize_frame(&camera.frame_id);
            if cameras.insert(frame.clone(), camera).is_some() {
                return Err(CalibrationError::DuplicateFrame(frame));
            }
        }
        Ok(Self { cameras })
    }

    pub fn get(&self, frame_id: &str) -> Option<&CameraCalibration> {
        self.cameras.get(&normalize_frame(frame_id))
    }

    pub fn len(&self) -> usize {
        self.cameras.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cameras.is_empty()
    }

    pub fn project_plan(
        &self,
        camera: &CameraFrame,
        path: &BevPathFrame,
        transforms: &TransformState,
        image_size: (u32, u32),
    ) -> Result<ProjectedPlan, ProjectionError> {
        let calibration = self
            .get(&camera.frame_id)
            .ok_or_else(|| ProjectionError::MissingCalibration(camera.frame_id.clone()))?;
        let ros_points = path
            .points
            .iter()
            .map(|[right, forward]| [*forward, -*right, 0.0])
            .collect::<Vec<_>>();
        let camera_points = transforms
            .transform_points_at(
                &path.frame_id,
                &camera.frame_id,
                camera.measurement_time,
                &ros_points,
            )
            .ok_or_else(|| ProjectionError::MissingTransform {
                source: path.frame_id.clone(),
                target: camera.frame_id.clone(),
            })?;
        Ok(project_camera_points(
            calibration,
            &camera_points,
            image_size,
        ))
    }
}

fn valid_calibration(camera: &CameraCalibration) -> bool {
    !normalize_frame(&camera.frame_id).is_empty()
        && camera.projection_model == "pinhole"
        && camera.width > 0
        && camera.height > 0
        && camera.fx.is_finite()
        && camera.fy.is_finite()
        && camera.cx.is_finite()
        && camera.cy.is_finite()
        && camera.fx > 0.0
        && camera.fy > 0.0
        && matches!(camera.distortion_model.as_str(), "none" | "plumb_bob")
        && camera
            .distortion_coefficients
            .iter()
            .all(|value| value.is_finite())
        && (camera.distortion_model == "none" || camera.distortion_coefficients.len() >= 5)
}

fn project_camera_points(
    calibration: &CameraCalibration,
    points: &[[f32; 3]],
    image_size: (u32, u32),
) -> ProjectedPlan {
    let scale_x = f64::from(image_size.0) / f64::from(calibration.width);
    let scale_y = f64::from(image_size.1) / f64::from(calibration.height);
    let mut projected = ProjectedPlan {
        points: Vec::with_capacity(points.len()),
        visible_points: 0,
    };
    for [camera_x, camera_y, depth] in points.iter().copied() {
        if !camera_x.is_finite()
            || !camera_y.is_finite()
            || !depth.is_finite()
            || depth <= MIN_CAMERA_DEPTH_METRES
        {
            projected.points.push(None);
            continue;
        }
        let normalized_x = f64::from(camera_x / depth);
        let normalized_y = f64::from(camera_y / depth);
        let [distorted_x, distorted_y] = distort(calibration, normalized_x, normalized_y);
        let pixel = [
            ((calibration.fx * distorted_x + calibration.cx) * scale_x) as f32,
            ((calibration.fy * distorted_y + calibration.cy) * scale_y) as f32,
        ];
        if pixel[0].is_finite() && pixel[1].is_finite() {
            if pixel[0] >= 0.0
                && pixel[0] < image_size.0 as f32
                && pixel[1] >= 0.0
                && pixel[1] < image_size.1 as f32
            {
                projected.visible_points += 1;
            }
            projected.points.push(Some(pixel));
        } else {
            projected.points.push(None);
        }
    }
    projected
}

fn distort(calibration: &CameraCalibration, x: f64, y: f64) -> [f64; 2] {
    if calibration.distortion_model == "none" {
        return [x, y];
    }
    let coefficients = &calibration.distortion_coefficients;
    let [k1, k2, p1, p2, k3] = [
        coefficients[0],
        coefficients[1],
        coefficients[2],
        coefficients[3],
        coefficients[4],
    ];
    let radius_squared = x * x + y * y;
    let radial = 1.0
        + k1 * radius_squared
        + k2 * radius_squared * radius_squared
        + k3 * radius_squared * radius_squared * radius_squared;
    [
        x * radial + 2.0 * p1 * x * y + p2 * (radius_squared + 2.0 * x * x),
        y * radial + p1 * (radius_squared + 2.0 * y * y) + 2.0 * p2 * x * y,
    ]
}

fn normalize_frame(frame: &str) -> String {
    frame.trim().trim_start_matches('/').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ArrivalTime, CameraId, MeasurementTime, TransformBatch, TransformStamped};

    fn camera() -> CameraFrame {
        CameraFrame {
            camera_id: CameraId(0),
            measurement_time: MeasurementTime(10),
            arrival_time: ArrivalTime(11),
            frame_id: "camera_front_optical_frame".into(),
            jpeg: vec![].into(),
        }
    }

    fn path() -> BevPathFrame {
        BevPathFrame {
            measurement_time: MeasurementTime(10),
            arrival_time: ArrivalTime(11),
            frame_id: "base_link".into(),
            points: vec![[0.0, 1.0], [0.0, -1.0]],
        }
    }

    #[test]
    fn bundled_file_contains_all_seven_pinhole_cameras() {
        let set = CameraCalibrationSet::from_json(include_str!(
            "../../../../config/camera_calibration.json"
        ))
        .unwrap();
        assert_eq!(set.len(), 7);
        assert_eq!(
            set.get("camera_front_optical_frame")
                .unwrap()
                .distortion_coefficients,
            vec![0.0; 5]
        );
    }

    #[test]
    fn projects_base_forward_to_the_optical_center_and_rejects_behind() {
        let set = CameraCalibrationSet::from_json(include_str!(
            "../../../../config/camera_calibration.json"
        ))
        .unwrap();
        let mut transforms = TransformState::default();
        transforms.apply(TransformBatch {
            arrival_time: ArrivalTime(1),
            is_static: true,
            transforms: vec![TransformStamped {
                measurement_time: MeasurementTime(0),
                frame_id: "base_link".into(),
                child_frame_id: "camera_front_optical_frame".into(),
                translation: [0.0; 3],
                rotation: [-0.5, 0.5, -0.5, 0.5],
            }],
        });
        let projected = set
            .project_plan(&camera(), &path(), &transforms, (320, 240))
            .unwrap();
        let center = projected.points[0].unwrap();
        assert!((center[0] - 159.5).abs() < 0.01);
        assert!((center[1] - 119.5).abs() < 0.01);
        assert_eq!(projected.points[1], None);
        assert_eq!(projected.visible_points, 1);
    }
}
