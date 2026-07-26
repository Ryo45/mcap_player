use super::Graphics;
use crate::presentation::CameraPresentationUpdate;
use std::{fmt, time::Instant};
use viewer_core::{BevPathFrame, CameraId, CameraState, TransformState};
use viewer_renderer::{ImageDecodeError, decode_camera_frame, prepare_camera_frame};

#[derive(Debug)]
pub(crate) struct CameraUploadError {
    pub(crate) camera_id: CameraId,
    source: ImageDecodeError,
}

impl fmt::Display for CameraUploadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "camera {} upload preparation failed: {}",
            self.camera_id.0, self.source
        )
    }
}

impl std::error::Error for CameraUploadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

impl Graphics {
    pub(crate) fn upload_latest(
        &mut self,
        camera: &CameraState,
        camera_path: Option<&BevPathFrame>,
        transforms: &TransformState,
        updates: &mut Vec<CameraPresentationUpdate>,
    ) -> Result<(), CameraUploadError> {
        for (camera_id, frame) in camera.frames() {
            if self.uploaded_arrivals.get(camera_id) == Some(&frame.arrival_time.0) {
                continue;
            }
            let decode_started = Instant::now();
            let image = decode_camera_frame(frame).map_err(|source| CameraUploadError {
                camera_id: *camera_id,
                source,
            })?;
            let decode_elapsed = decode_started.elapsed();
            let upload_started = Instant::now();
            let prepared =
                prepare_camera_frame(frame, image, camera_path, transforms, &self.calibrations);
            let slot = self.camera_slots.entry(*camera_id).or_default();
            let recreated = slot.update(&self.device, &self.queue, &prepared.image);
            if recreated {
                let view = slot.view().expect("updated slot has a view");
                let texture_id = self
                    .camera_texture_ids
                    .entry(*camera_id)
                    .or_insert_with(|| {
                        self.egui_renderer.register_native_texture(
                            &self.device,
                            view,
                            wgpu::FilterMode::Linear,
                        )
                    });
                self.egui_renderer.update_egui_texture_from_wgpu_texture(
                    &self.device,
                    view,
                    wgpu::FilterMode::Linear,
                    *texture_id,
                );
            }
            self.uploaded_arrivals
                .insert(*camera_id, prepared.arrival_time.0);
            updates.push(CameraPresentationUpdate {
                camera_id: *camera_id,
                overlay_status: prepared.overlay_status,
                jpeg_decode_time: decode_elapsed,
                upload_time: upload_started.elapsed(),
            });
        }
        Ok(())
    }

    pub(crate) fn hide_camera(&mut self) {
        self.uploaded_arrivals.clear();
    }

    pub(super) fn camera_texture(
        &self,
        camera_id: CameraId,
    ) -> Option<(egui::TextureId, (u32, u32))> {
        Some((
            *self.camera_texture_ids.get(&camera_id)?,
            self.camera_slots.get(&camera_id)?.size()?,
        ))
    }
}
