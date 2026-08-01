//! Platform-neutral camera preparation and persistent native GPU textures.

mod camera_base;
mod camera_overlay;
mod camera_texture;
mod image;

pub use camera_base::CameraBaseImageTracker;
pub use camera_overlay::{CameraOverlaySnapshot, CameraOverlayState};
pub use camera_texture::{CameraTextureSlot, TextureMetrics};
pub use image::{DecodedImage, ImageDecodeError, decode_jpeg};

use viewer_core::CameraFrame;

pub fn decode_camera_frame(frame: &CameraFrame) -> Result<DecodedImage, ImageDecodeError> {
    decode_jpeg(&frame.jpeg)
}
