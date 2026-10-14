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

/// Read JPEG source dimensions without materializing its pixel buffer.
pub fn jpeg_dimensions(bytes: &[u8]) -> Result<(u32, u32), ImageDecodeError> {
    fn invalid() -> ImageDecodeError {
        ImageDecodeError(image::ImageError::IoError(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid JPEG dimension header",
        )))
    }
    if !bytes.starts_with(&[0xff, 0xd8]) {
        return Err(invalid());
    }
    let mut offset = 2usize;
    loop {
        if bytes.get(offset) != Some(&0xff) {
            return Err(invalid());
        }
        while bytes.get(offset) == Some(&0xff) {
            offset += 1;
        }
        let marker = *bytes.get(offset).ok_or_else(invalid)?;
        offset += 1;
        if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        if matches!(marker, 0x00 | 0xd8 | 0xd9 | 0xda) {
            return Err(invalid());
        }
        let length = bytes.get(offset..offset + 2).ok_or_else(invalid)?;
        let length = usize::from(u16::from_be_bytes([length[0], length[1]]));
        if length < 2 {
            return Err(invalid());
        }
        let end = offset.checked_add(length).ok_or_else(invalid)?;
        let segment = bytes.get(offset + 2..end).ok_or_else(invalid)?;
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            if segment.len() < 6
                || segment[5] == 0
                || segment.len() != 6 + usize::from(segment[5]) * 3
            {
                return Err(invalid());
            }
            let height = u16::from_be_bytes([segment[1], segment[2]]);
            let width = u16::from_be_bytes([segment[3], segment[4]]);
            if width == 0 || height == 0 {
                return Err(invalid());
            }
            return Ok((u32::from(width), u32::from(height)));
        }
        offset = end;
    }
}

#[cfg(test)]
mod dimension_tests {
    use super::*;
    #[test]
    fn dimensions_do_not_require_entropy_data_and_skip_metadata() {
        let mut encoded = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut encoded)
            .encode_image(&image::RgbImage::new(641, 479))
            .unwrap();
        assert_eq!(jpeg_dimensions(&encoded).unwrap(), (641, 479));
        let scan = encoded.windows(2).position(|b| b == [0xff, 0xda]).unwrap();
        assert_eq!(jpeg_dimensions(&encoded[..scan]).unwrap(), (641, 479));
        let mut with_metadata = vec![0xff, 0xd8, 0xff, 0xe1, 0, 6, 0, 0, 0, 0];
        with_metadata.extend_from_slice(&encoded[2..]);
        assert_eq!(jpeg_dimensions(&with_metadata).unwrap(), (641, 479));
        for length in 0..scan {
            let _ = jpeg_dimensions(&encoded[..length]);
        }
    }
    #[test]
    fn malformed_segment_lengths_cannot_loop_or_read_out_of_bounds() {
        for bytes in [
            &[0xff, 0xd8, 0xff, 0xe1, 0, 0][..],
            &[0xff, 0xd8, 0xff, 0xe1, 0xff, 0xff],
            &[0xff, 0xd8, 0xff, 0xff],
            &[0xff, 0xd8, 0xff, 0xda],
            &[0xff, 0xd8, 0xff, 0xc0, 0, 2],
        ] {
            assert!(jpeg_dimensions(bytes).is_err());
        }
    }
}
