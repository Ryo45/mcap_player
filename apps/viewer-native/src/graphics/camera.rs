use super::Graphics;
use crate::presentation::CameraBasePresentationUpdate;
use std::{fmt, time::Instant};
use viewer_core::{ArrivalTime, CameraId, CameraPreviewFrame, CameraState};
use viewer_renderer::{ImageDecodeError, decode_camera_frame, decode_jpeg};

#[derive(Debug)]
pub(crate) struct CameraUploadError {
    camera_id: CameraId,
    source: ImageDecodeError,
}

impl fmt::Display for CameraUploadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "camera {} base image upload failed: {}",
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
    pub(crate) fn upload_preview(
        &mut self,
        frames: &[CameraPreviewFrame],
    ) -> Result<(), CameraUploadError> {
        for frame in frames {
            let key = (frame.arrival_time(), frame.width(), frame.height());
            if self.preview_camera_keys.get(&frame.camera_id()) == Some(&key) {
                continue;
            }
            let image = decode_jpeg(frame.bytes()).map_err(|source| CameraUploadError {
                camera_id: frame.camera_id(),
                source,
            })?;
            let slot = self
                .preview_camera_slots
                .entry(frame.camera_id())
                .or_default();
            let recreated = slot.update(&self.device, &self.queue, &image);
            if recreated {
                let view = slot.view().expect("updated preview slot has a view");
                let texture_id = self
                    .preview_camera_texture_ids
                    .entry(frame.camera_id())
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
            self.preview_camera_keys.insert(frame.camera_id(), key);
        }
        Ok(())
    }

    pub(crate) fn upload_latest(
        &mut self,
        camera: &CameraState,
        updates: &mut Vec<CameraBasePresentationUpdate>,
    ) -> Result<(), CameraUploadError> {
        for (camera_id, frame) in camera.frames() {
            if !self.camera_base_images.needs_update(frame) {
                continue;
            }
            let decode_started = Instant::now();
            let image = decode_camera_frame(frame).map_err(|source| CameraUploadError {
                camera_id: *camera_id,
                source,
            })?;
            let decode_elapsed = decode_started.elapsed();
            let upload_started = Instant::now();
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
            self.camera_base_images.mark_updated(frame);
            updates.push(CameraBasePresentationUpdate {
                camera_id: *camera_id,
                jpeg_decode_time: decode_elapsed,
                upload_time: upload_started.elapsed(),
            });
        }
        Ok(())
    }

    pub(crate) fn hide_camera(&mut self) {
        self.camera_base_images.clear();
    }

    pub(crate) fn clear_preview(&mut self) {
        self.preview_camera_keys.clear();
    }

    pub(crate) fn camera_base_images(
        &self,
    ) -> impl Iterator<Item = (CameraId, ArrivalTime, (u32, u32))> + '_ {
        self.camera_slots.iter().filter_map(|(camera_id, slot)| {
            Some((
                *camera_id,
                self.camera_base_images.arrival(*camera_id)?,
                slot.size()?,
            ))
        })
    }

    pub(super) fn camera_texture(
        &self,
        camera_id: CameraId,
    ) -> Option<(egui::TextureId, ArrivalTime, (u32, u32))> {
        Some((
            *self.camera_texture_ids.get(&camera_id)?,
            self.camera_base_images.arrival(camera_id)?,
            self.camera_slots.get(&camera_id)?.size()?,
        ))
    }

    pub(super) fn preview_camera_texture(
        &self,
        camera_id: CameraId,
    ) -> Option<(egui::TextureId, ArrivalTime, (u32, u32))> {
        let (arrival, _, _) = *self.preview_camera_keys.get(&camera_id)?;
        Some((
            *self.preview_camera_texture_ids.get(&camera_id)?,
            arrival,
            self.preview_camera_slots.get(&camera_id)?.size()?,
        ))
    }
}
