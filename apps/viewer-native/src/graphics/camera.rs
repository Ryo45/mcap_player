use super::Graphics;
use crate::session::PlaybackSession;
use std::{fmt, sync::Arc, time::Instant};
use viewer_core::{CameraId, DomainState, OverlayStatus};
use viewer_renderer::{ImageDecodeError, decode_jpeg, draw_plan_overlay};

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
    pub(crate) fn upload_latest(&mut self, state: &DomainState) -> Result<(), CameraUploadError> {
        for (camera_id, frame) in state.camera.frames() {
            if self.uploaded_arrivals.get(camera_id) == Some(&frame.arrival_time.0) {
                continue;
            }
            let decode_started = Instant::now();
            let mut image = decode_jpeg(&frame.jpeg).map_err(|source| CameraUploadError {
                camera_id: *camera_id,
                source,
            })?;
            let decode_elapsed = decode_started.elapsed();
            let upload_started = Instant::now();
            if let Some(path) = state.bev.latest() {
                match self.calibrations.project_plan(
                    frame,
                    path,
                    &state.transforms,
                    (image.width, image.height),
                ) {
                    Ok(projected) => {
                        draw_plan_overlay(&mut image, &projected.points);
                        self.overlay_status.insert(
                            *camera_id,
                            OverlayStatus::Ready {
                                visible_points: projected.visible_points,
                            },
                        );
                    }
                    Err(error) => {
                        self.overlay_status
                            .insert(*camera_id, OverlayStatus::Error(error.to_string()));
                    }
                }
            } else {
                self.overlay_status
                    .insert(*camera_id, OverlayStatus::Waiting);
            }
            let slot = self.camera_slots.entry(*camera_id).or_default();
            let recreated = slot.update(&self.device, &self.queue, &image);
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
                .insert(*camera_id, frame.arrival_time.0);
            self.presentation_metrics.record_camera(
                *camera_id,
                decode_elapsed,
                upload_started.elapsed(),
            );
        }
        Ok(())
    }

    pub(crate) fn hide_camera(&mut self) {
        self.uploaded_arrivals.clear();
    }

    pub(crate) fn reset_camera_catalog(&mut self) {
        self.hide_camera();
        self.camera_topics = Arc::new(Vec::new());
        self.focused_camera = None;
        self.overlay_status.clear();
        self.presentation_metrics.reset();
    }

    pub(super) fn sync_camera_catalog(&mut self, session: &PlaybackSession) {
        if self.camera_topics.as_slice() != session.camera_topics() {
            self.camera_topics = Arc::new(session.camera_topics().to_vec());
        }
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
