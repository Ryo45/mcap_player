//! Platform-neutral camera preparation and persistent native GPU textures.

mod camera_base;
#[cfg(all(feature = "native-jpeg", not(target_arch = "wasm32")))]
mod camera_decode;
mod camera_overlay;
mod camera_texture;
mod image;
#[cfg(all(feature = "native-jpeg", not(target_arch = "wasm32")))]
mod native_jpeg;
#[cfg(all(feature = "native-jpeg", not(target_arch = "wasm32")))]
pub use camera_decode::{CameraDecodeQueue, CameraDecodeResult, CameraDecodeStartError};
#[cfg(all(feature = "native-jpeg", not(target_arch = "wasm32")))]
pub use native_jpeg::{NativeImage, NativeImageGeometry, NativeJpegDecoder, NativeJpegError};

pub use camera_base::CameraBaseImageTracker;
pub use camera_overlay::{CameraOverlaySnapshot, CameraOverlayState};
pub use camera_texture::{CameraTextureSlot, TextureMetrics};
pub use image::{DecodedImage, ImageDecodeError, decode_jpeg, jpeg_dimensions};

use viewer_core::CameraFrame;

pub fn decode_camera_frame(frame: &CameraFrame) -> Result<DecodedImage, ImageDecodeError> {
    decode_jpeg(&frame.jpeg)
}
