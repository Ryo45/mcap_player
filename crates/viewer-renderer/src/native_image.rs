//! Native camera pixel decoding, with SIMD/IDCT scaling for JPEG.
use crate::{DecodedImage, ImageDecodeError, decode_jpeg};
use std::fmt;
use turbojpeg::{Colorspace, Decompressor, PixelFormat, ScalingFactor};

const MAX_JPEG_BYTES: usize = 32 * 1024 * 1024;
const MAX_SOURCE_PIXELS: usize = 32 * 1024 * 1024;

#[derive(Debug)]
pub enum NativeImageError {
    Codec(turbojpeg::Error),
    Image(ImageDecodeError),
    Limit(&'static str),
    Allocation(std::collections::TryReserveError),
}
impl fmt::Display for NativeImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Codec(e) => write!(f, "native JPEG: {e}"),
            Self::Image(e) => e.fmt(f),
            Self::Limit(s) => write!(f, "native image exceeds {s} limit"),
            Self::Allocation(e) => write!(f, "native image allocation: {e}"),
        }
    }
}
impl std::error::Error for NativeImageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Codec(e) => Some(e),
            Self::Image(e) => Some(e),
            Self::Allocation(e) => Some(e),
            Self::Limit(_) => None,
        }
    }
}
impl From<turbojpeg::Error> for NativeImageError {
    fn from(e: turbojpeg::Error) -> Self {
        Self::Codec(e)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeImageGeometry {
    pub source_size: (u32, u32),
    pub pixel_size: (u32, u32),
    /// Crop padded final IDCT pixels; preserves source aspect ratio for odd JPEG sizes.
    pub uv_max: [f32; 2],
    scaling: ImageScaling,
}
#[derive(Clone, Copy, Debug, PartialEq)]
enum ImageScaling {
    Fixed,
    Idct,
    Resize,
}

impl NativeImageGeometry {
    pub fn matches_target(&self, target: (u32, u32)) -> bool {
        if self.scaling == ImageScaling::Resize {
            return self.pixel_size == crate::camera_target_size(self.source_size, target);
        }
        let factor = scale(self.source_size, target, self.scaling == ImageScaling::Idct);
        self.pixel_size
            == (
                factor.scale(self.source_size.0 as usize) as u32,
                factor.scale(self.source_size.1 as usize) as u32,
            )
    }
}
#[derive(Debug)]
pub struct NativeImage {
    pub image: DecodedImage,
    pub geometry: NativeImageGeometry,
}

pub struct NativeCameraDecoder {
    decoder: Decompressor,
}
impl NativeCameraDecoder {
    pub fn new() -> Result<Self, NativeImageError> {
        let mut decoder = Decompressor::new()?;
        decoder.set_scan_limit(100)?;
        Ok(Self { decoder })
    }
    pub fn decode_frame(
        &mut self,
        frame: &viewer_core::CameraFrame,
        target: (u32, u32),
    ) -> Result<NativeImage, NativeImageError> {
        if frame.format == viewer_core::CameraImageFormat::Jpeg {
            return self.decode(&frame.data, target);
        }
        let source_size = crate::camera_dimensions(frame).map_err(NativeImageError::Image)?;
        let image = crate::decode_camera_image(frame, target).map_err(NativeImageError::Image)?;
        Ok(NativeImage {
            geometry: NativeImageGeometry {
                source_size,
                pixel_size: (image.width, image.height),
                uv_max: [1.; 2],
                scaling: ImageScaling::Resize,
            },
            image,
        })
    }
    pub fn decode(
        &mut self,
        bytes: &[u8],
        target: (u32, u32),
    ) -> Result<NativeImage, NativeImageError> {
        if bytes.len() > MAX_JPEG_BYTES {
            return Err(NativeImageError::Limit("32 MiB input"));
        }
        let header = self.decoder.read_header(bytes)?;
        if header.width == 0
            || header.height == 0
            || header
                .width
                .checked_mul(header.height)
                .is_none_or(|n| n > MAX_SOURCE_PIXELS)
        {
            return Err(NativeImageError::Limit("32 megapixel source"));
        }
        let source_size = (header.width as u32, header.height as u32);
        // TurboJPEG does not convert CMYK/YCCK into RGBA. Keep valid legacy CMYK input usable.
        if matches!(header.colorspace, Colorspace::CMYK | Colorspace::YCCK) {
            let image = decode_jpeg(bytes).map_err(NativeImageError::Image)?;
            return Ok(NativeImage {
                geometry: NativeImageGeometry {
                    source_size,
                    pixel_size: (image.width, image.height),
                    uv_max: [1.; 2],
                    scaling: ImageScaling::Fixed,
                },
                image,
            });
        }
        let scalable = !header.is_lossless;
        let factor = scale(source_size, target, scalable);
        self.decoder.set_scaling_factor(factor)?;
        let width = factor.scale(header.width);
        let height = factor.scale(header.height);
        let length = width
            .checked_mul(height)
            .and_then(|n| n.checked_mul(4))
            .ok_or(NativeImageError::Limit("RGBA dimensions"))?;
        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(length)
            .map_err(NativeImageError::Allocation)?;
        pixels.resize(length, 0);
        self.decoder.decompress(
            bytes,
            turbojpeg::Image {
                pixels: pixels.as_mut_slice(),
                width,
                height,
                pitch: width * 4,
                format: PixelFormat::RGBA,
            },
        )?;
        let ratio = factor.num() as f32 / factor.denom() as f32;
        Ok(NativeImage {
            image: DecodedImage {
                width: width as u32,
                height: height as u32,
                rgba: pixels,
            },
            geometry: NativeImageGeometry {
                source_size,
                pixel_size: (width as u32, height as u32),
                uv_max: [
                    source_size.0 as f32 * ratio / width as f32,
                    source_size.1 as f32 * ratio / height as f32,
                ],
                scaling: if scalable {
                    ImageScaling::Idct
                } else {
                    ImageScaling::Fixed
                },
            },
        })
    }
}
fn scale(source: (u32, u32), target: (u32, u32), scalable: bool) -> ScalingFactor {
    if !scalable {
        return ScalingFactor::ONE;
    }
    let ratio = (target.0.max(1) as f64 / source.0 as f64)
        .min(target.1.max(1) as f64 / source.1 as f64)
        .min(1.0);
    for denominator in [8, 4, 2, 1] {
        if 1.0 / denominator as f64 >= ratio {
            return ScalingFactor::new(1, denominator);
        }
    }
    ScalingFactor::ONE
}

#[cfg(test)]
mod tests {
    use super::*;
    fn jpeg(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 90)
            .encode_image(&image::RgbImage::from_pixel(
                width,
                height,
                image::Rgb([30, 60, 90]),
            ))
            .unwrap();
        bytes
    }
    #[test]
    fn idct_scaling_keeps_source_geometry_and_alpha() {
        let mut decoder = NativeCameraDecoder::new().unwrap();
        let decoded = decoder.decode(&jpeg(641, 479), (100, 100)).unwrap();
        assert_eq!(decoded.geometry.source_size, (641, 479));
        assert_eq!(decoded.geometry.pixel_size, (161, 120));
        assert_eq!(decoded.image.rgba.len(), 161 * 120 * 4);
        assert!(decoded.image.rgba.chunks_exact(4).all(|p| p[3] == 255));
        assert!(decoded.geometry.uv_max[0] < 1.0 && decoded.geometry.uv_max[1] < 1.0);
        assert!(decoded.geometry.matches_target((100, 100)));
        assert!(!decoded.geometry.matches_target((640, 480)));
        // Scale is reset on decoder reuse (thumbnail -> full focus).
        let full = decoder.decode(&jpeg(641, 479), (641, 479)).unwrap();
        assert_eq!(full.geometry.pixel_size, (641, 479));
        assert_eq!(full.geometry.uv_max, [1.; 2]);
    }
    #[test]
    fn malformed_and_oversized_input_are_errors() {
        let mut decoder = NativeCameraDecoder::new().unwrap();
        assert!(decoder.decode(&[0, 1, 2], (320, 240)).is_err());
        assert!(decoder.decode(&jpeg(10, 10)[..20], (320, 240)).is_err());
        assert!(matches!(
            decoder.decode(&vec![0; MAX_JPEG_BYTES + 1], (320, 240)),
            Err(NativeImageError::Limit(_))
        ));
    }
}
