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
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "JPEG decode failed: {}", self.0)
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
