use super::Graphics;
use crate::presentation::CameraBasePresentationUpdate;
use std::{fmt, time::Instant};
use viewer_core::{ArrivalTime, CameraId, CameraPreviewFrame, CameraState};
use viewer_renderer::NativeJpegError;

#[derive(Debug)]
pub(crate) struct CameraUploadError {
    camera_id: CameraId,
    source: NativeJpegError,
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
            let image = self
                .jpeg_decoder
                .decode(frame.bytes(), (frame.width(), frame.height()))
                .map_err(|source| CameraUploadError {
                    camera_id: frame.camera_id(),
                    source,
                })?
                .image;
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
        let mut error = None;
        while let Some(result) = self.camera_decode.take_ready() {
            let frame = result.frame;
            let camera_id = frame.camera_id;
            let Some(&target) = self.camera_decode_sizes.get(&camera_id) else {
                continue;
            };
            if self
                .camera_base_images
                .arrival(camera_id)
                .is_some_and(|arrival| arrival > frame.arrival_time)
            {
                continue;
            }
            let decoded = match result.image {
                Ok(decoded) => decoded,
                Err(source) => {
                    error = Some(CameraUploadError { camera_id, source });
                    continue;
                }
            };
            // A resize can supersede a decode without changing message arrival time.
            if !decoded.geometry.matches_target(target) {
                continue;
            }
            if decoded.image.width > self.device.limits().max_texture_dimension_2d
                || decoded.image.height > self.device.limits().max_texture_dimension_2d
            {
                error = Some(CameraUploadError {
                    camera_id,
                    source: NativeJpegError::Limit("GPU texture dimensions"),
                });
                continue;
            }
            let image = decoded.image;
            let decode_elapsed = result.decode_time;
            let upload_started = Instant::now();
            let slot = self.camera_slots.entry(camera_id).or_default();
            let recreated = slot.update(&self.device, &self.queue, &image);
            if recreated {
                let view = slot.view().expect("updated slot has a view");
                let texture_id = self.camera_texture_ids.entry(camera_id).or_insert_with(|| {
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
            self.camera_geometry.insert(camera_id, decoded.geometry);
            self.camera_base_images.mark_updated(&frame);
            self.presented_camera_frames.insert(camera_id, frame);
            updates.push(CameraBasePresentationUpdate {
                camera_id,
                jpeg_decode_time: decode_elapsed,
                upload_time: upload_started.elapsed(),
            });
        }
        for (camera_id, frame) in camera.frames() {
            let Some(&target) = self.camera_decode_sizes.get(camera_id) else {
                continue;
            };
            if !self.camera_base_images.needs_update(frame)
                && self
                    .camera_geometry
                    .get(camera_id)
                    .is_some_and(|g| g.matches_target(target))
            {
                continue;
            }
            if let Err(source) = self.camera_decode.submit(frame, target) {
                error = Some(CameraUploadError {
                    camera_id: *camera_id,
                    source,
                });
            }
        }
        error.map_or(Ok(()), Err)
    }

    pub(crate) fn hide_camera(&mut self) {
        self.camera_base_images.clear();
        self.camera_geometry.clear();
        self.presented_camera_frames.clear();
        self.camera_decode.clear();
    }

    pub(crate) fn clear_preview(&mut self) {
        self.preview_camera_keys.clear();
    }

    pub(crate) fn presented_camera_frames(
        &self,
    ) -> impl Iterator<Item = (&viewer_core::CameraFrame, (u32, u32))> {
        self.presented_camera_frames
            .iter()
            .filter_map(|(id, frame)| Some((frame, self.camera_geometry.get(id)?.source_size)))
    }

    pub(super) fn camera_texture(
        &self,
        camera_id: CameraId,
    ) -> Option<(egui::TextureId, ArrivalTime, (u32, u32))> {
        Some((
            *self.camera_texture_ids.get(&camera_id)?,
            self.camera_base_images.arrival(camera_id)?,
            self.camera_geometry.get(&camera_id)?.source_size,
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
