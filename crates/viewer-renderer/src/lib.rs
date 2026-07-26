//! Platform-neutral camera preparation and persistent native GPU textures.

mod camera_texture;
mod image;
mod overlay;

pub use camera_texture::{CameraTextureSlot, TextureMetrics};
pub use image::{DecodedImage, ImageDecodeError, decode_jpeg};
pub use overlay::draw_plan_overlay;

use viewer_core::{
    ArrivalTime, BevPathFrame, CameraCalibrationSet, CameraFrame, CameraId, OverlayStatus,
    TransformState,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedCameraFrame {
    pub camera_id: CameraId,
    pub arrival_time: ArrivalTime,
    pub image: DecodedImage,
    pub overlay_status: OverlayStatus,
}

pub fn decode_camera_frame(frame: &CameraFrame) -> Result<DecodedImage, ImageDecodeError> {
    decode_jpeg(&frame.jpeg)
}

pub fn prepare_camera_frame(
    frame: &CameraFrame,
    mut image: DecodedImage,
    path: Option<&BevPathFrame>,
    transforms: &TransformState,
    calibrations: &CameraCalibrationSet,
) -> PreparedCameraFrame {
    let overlay_status = path.map_or(OverlayStatus::Waiting, |path| {
        match calibrations.project_plan(frame, path, transforms, (image.width, image.height)) {
            Ok(projected) => {
                draw_plan_overlay(&mut image, &projected.points);
                OverlayStatus::Ready {
                    visible_points: projected.visible_points,
                }
            }
            Err(error) => OverlayStatus::Error(error.to_string()),
        }
    });
    PreparedCameraFrame {
        camera_id: frame.camera_id,
        arrival_time: frame.arrival_time,
        image,
        overlay_status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use viewer_core::{MeasurementTime, TransformBatch, TransformStamped};

    #[test]
    fn prepares_identity_and_waiting_status_without_a_plan() {
        let frame = CameraFrame {
            camera_id: CameraId(4),
            measurement_time: MeasurementTime(1),
            arrival_time: ArrivalTime(2),
            frame_id: "camera".into(),
            jpeg: vec![],
        };
        let image = DecodedImage {
            width: 2,
            height: 1,
            rgba: vec![0; 8],
        };
        let prepared = prepare_camera_frame(
            &frame,
            image.clone(),
            None,
            &TransformState::default(),
            &CameraCalibrationSet::default(),
        );
        assert_eq!(prepared.camera_id, CameraId(4));
        assert_eq!(prepared.arrival_time, ArrivalTime(2));
        assert_eq!(prepared.image, image);
        assert_eq!(prepared.overlay_status, OverlayStatus::Waiting);
    }

    #[test]
    fn projects_and_draws_a_ready_plan() {
        let frame = CameraFrame {
            camera_id: CameraId(0),
            measurement_time: MeasurementTime(10),
            arrival_time: ArrivalTime(11),
            frame_id: "camera_front_optical_frame".into(),
            jpeg: vec![],
        };
        let path = BevPathFrame {
            measurement_time: MeasurementTime(10),
            arrival_time: ArrivalTime(11),
            frame_id: "base_link".into(),
            points: vec![[0.0, 1.0], [0.0, 2.0]],
        };
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
        let calibrations = CameraCalibrationSet::from_json(include_str!(
            "../../../config/camera_calibration.json"
        ))
        .unwrap();
        let prepared = prepare_camera_frame(
            &frame,
            DecodedImage {
                width: 320,
                height: 240,
                rgba: vec![0; 320 * 240 * 4],
            },
            Some(&path),
            &transforms,
            &calibrations,
        );

        assert_eq!(
            prepared.overlay_status,
            OverlayStatus::Ready { visible_points: 2 }
        );
        let center = (119 * 320 + 159) * 4;
        assert_ne!(&prepared.image.rgba[center..center + 4], &[0, 0, 0, 0]);
    }
}
