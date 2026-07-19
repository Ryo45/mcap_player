//! Shared JPEG decoding and persistent camera GPU textures.

use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug)]
pub struct ImageDecodeError(image::ImageError);

impl fmt::Display for ImageDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JPEG decode failed: {}", self.0)
    }
}

impl std::error::Error for ImageDecodeError {}

pub fn decode_jpeg(bytes: &[u8]) -> Result<DecodedImage, ImageDecodeError> {
    let decoded = image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg)
        .map_err(ImageDecodeError)?;
    let rgba = decoded.into_rgba8();
    Ok(DecodedImage {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TextureMetrics {
    pub creations: u64,
    pub writes: u64,
}

pub struct CameraTextureSlot {
    texture: Option<wgpu::Texture>,
    view: Option<wgpu::TextureView>,
    size: Option<(u32, u32)>,
    format: wgpu::TextureFormat,
    metrics: TextureMetrics,
}

impl Default for CameraTextureSlot {
    fn default() -> Self {
        Self::new(wgpu::TextureFormat::Rgba8Unorm)
    }
}

impl CameraTextureSlot {
    pub fn new(format: wgpu::TextureFormat) -> Self {
        Self {
            texture: None,
            view: None,
            size: None,
            format,
            metrics: TextureMetrics::default(),
        }
    }

    pub fn needs_recreate(&self, width: u32, height: u32, format: wgpu::TextureFormat) -> bool {
        self.texture.is_none() || self.size != Some((width, height)) || self.format != format
    }

    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        image: &DecodedImage,
    ) -> bool {
        if image.width == 0
            || image.height == 0
            || image.rgba.len() != image.width as usize * image.height as usize * 4
        {
            return false;
        }
        let recreated =
            self.needs_recreate(image.width, image.height, wgpu::TextureFormat::Rgba8Unorm);
        if recreated {
            self.format = wgpu::TextureFormat::Rgba8Unorm;
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("camera texture slot"),
                size: wgpu::Extent3d {
                    width: image.width,
                    height: image.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            self.view = Some(texture.create_view(&wgpu::TextureViewDescriptor::default()));
            self.texture = Some(texture);
            self.size = Some((image.width, image.height));
            self.metrics.creations += 1;
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: self.texture.as_ref().expect("texture was created"),
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &image.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * image.width),
                rows_per_image: Some(image.height),
            },
            wgpu::Extent3d {
                width: image.width,
                height: image.height,
                depth_or_array_layers: 1,
            },
        );
        self.metrics.writes += 1;
        recreated
    }

    pub fn view(&self) -> Option<&wgpu::TextureView> {
        self.view.as_ref()
    }
    pub fn size(&self) -> Option<(u32, u32)> {
        self.size
    }
    pub fn metrics(&self) -> TextureMetrics {
        self.metrics
    }
    pub fn clear(&mut self) {
        self.texture = None;
        self.view = None;
        self.size = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recreation_decision_only_depends_on_image_storage() {
        let slot = CameraTextureSlot::default();
        assert!(slot.needs_recreate(320, 240, wgpu::TextureFormat::Rgba8UnormSrgb));
        assert!(slot.needs_recreate(320, 240, wgpu::TextureFormat::Rgba8Unorm));
    }
}
