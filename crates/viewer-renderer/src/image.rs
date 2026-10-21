use std::fmt;

/// Unmultiplied (straight) RGBA in source color order. Frontends prepare their
/// own blend/texture convention without modifying canonical Camera data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug)]
pub struct ImageDecodeError(pub(crate) image::ImageError);

impl fmt::Display for ImageDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "image decode failed: {}", self.0)
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

fn invalid_image(message: &str) -> ImageDecodeError {
    ImageDecodeError(image::ImageError::IoError(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message,
    )))
}

/// Validate source facts before scheduling pixel work (32 MiB input, 32 megapixels).
pub fn camera_dimensions(frame: &viewer_core::CameraFrame) -> Result<(u32, u32), ImageDecodeError> {
    use viewer_core::CameraImageFormat;
    if frame.data.len() > 32 * 1024 * 1024 {
        return Err(invalid_image("camera exceeds 32 MiB input limit"));
    }
    let size = match frame.format {
        CameraImageFormat::Jpeg => jpeg_dimensions(&frame.data)?,
        CameraImageFormat::Png => {
            // Inspect only IHDR on the UI thread. Codec/CRC validation stays in the worker;
            // constructing a PNG decoder here would also parse/inflate ancillary metadata.
            let header = frame
                .data
                .get(..33)
                .ok_or_else(|| invalid_image("truncated PNG header"))?;
            if &header[..8] != b"\x89PNG\r\n\x1a\n"
                || &header[8..16] != b"\0\0\0\rIHDR"
                || !matches!(header[24], 1 | 2 | 4 | 8)
                || !matches!(header[25], 0 | 2 | 3 | 4 | 6)
            {
                return Err(invalid_image("unsupported PNG header / depth"));
            }
            let width = u32::from_be_bytes(header[16..20].try_into().expect("four IHDR bytes"));
            let height = u32::from_be_bytes(header[20..24].try_into().expect("four IHDR bytes"));
            (width, height)
        }
        CameraImageFormat::Raw(layout) => {
            layout
                .validate(frame.data.len())
                .map_err(|e| invalid_image(&e.to_string()))?;
            (layout.width, layout.height)
        }
    };
    if size.0 == 0 || size.1 == 0 || u64::from(size.0) * u64::from(size.1) > 32 * 1024 * 1024 {
        return Err(invalid_image("camera exceeds 32 megapixel source limit"));
    }
    Ok(size)
}

/// Aspect-preserving display dimensions, never upscaling the source.
pub fn camera_target_size(source: (u32, u32), target: (u32, u32)) -> (u32, u32) {
    let ratio = (target.0.max(1) as f64 / source.0 as f64)
        .min(target.1.max(1) as f64 / source.1 as f64)
        .min(1.0);
    (
        (source.0 as f64 * ratio).round().max(1.0) as u32,
        (source.1 as f64 * ratio).round().max(1.0) as u32,
    )
}

/// Raw conversion samples directly into display-sized RGBA; no source-sized RGBA allocation.
pub fn decode_camera_image(
    frame: &viewer_core::CameraFrame,
    target: (u32, u32),
) -> Result<DecodedImage, ImageDecodeError> {
    use viewer_core::{CameraImageFormat, RawImageEncoding};
    let source = camera_dimensions(frame)?;
    let (width, height) = camera_target_size(source, target);
    if let CameraImageFormat::Raw(layout) = frame.format {
        let mut rgba = vec![0; width as usize * height as usize * 4];
        for y in 0..height {
            let sy = (u64::from(y) * u64::from(layout.height) / u64::from(height)) as usize;
            for x in 0..width {
                let sx = (u64::from(x) * u64::from(layout.width) / u64::from(width)) as usize;
                let offset = sy * layout.step as usize + sx * layout.encoding.channels() as usize;
                let input = &frame.data[offset..offset + layout.encoding.channels() as usize];
                let pixel = match layout.encoding {
                    RawImageEncoding::Rgb8 => [input[0], input[1], input[2], 255],
                    RawImageEncoding::Bgr8 => [input[2], input[1], input[0], 255],
                    RawImageEncoding::Rgba8 => [input[0], input[1], input[2], input[3]],
                    RawImageEncoding::Bgra8 => [input[2], input[1], input[0], input[3]],
                    RawImageEncoding::Mono8 => [input[0], input[0], input[0], 255],
                };
                let out = (y as usize * width as usize + x as usize) * 4;
                rgba[out..out + 4].copy_from_slice(&pixel);
            }
        }
        return Ok(DecodedImage {
            width,
            height,
            rgba,
        });
    }
    let format = if frame.format == CameraImageFormat::Png {
        image::ImageFormat::Png
    } else {
        image::ImageFormat::Jpeg
    };
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(&frame.data), format);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(256 * 1024 * 1024);
    limits.max_image_width = Some(source.0);
    limits.max_image_height = Some(source.1);
    reader.limits(limits);
    let decoded = reader.decode().map_err(ImageDecodeError)?;
    let rgba = if source == (width, height) {
        decoded.into_rgba8()
    } else {
        decoded
            .resize_exact(width, height, image::imageops::FilterType::Nearest)
            .into_rgba8()
    };
    Ok(DecodedImage {
        width,
        height,
        rgba: rgba.into_raw(),
    })
}

#[cfg(test)]
mod camera_tests {
    use super::*;
    use viewer_core::{
        ArrivalTime, CameraFrame, CameraId, CameraImageFormat, MeasurementTime, RawImageEncoding,
        RawImageLayout,
    };
    fn frame(format: CameraImageFormat, data: Vec<u8>) -> CameraFrame {
        CameraFrame {
            camera_id: CameraId(0),
            arrival_time: ArrivalTime(1),
            measurement_time: MeasurementTime(0),
            frame_id: "optical".into(),
            format,
            data: data.into(),
        }
    }
    #[test]
    fn raw_channels_padding_alpha_and_display_sampling() {
        for (encoding, pixel, expected) in [
            (
                RawImageEncoding::Rgb8,
                vec![10, 20, 30],
                vec![10, 20, 30, 255],
            ),
            (
                RawImageEncoding::Bgr8,
                vec![30, 20, 10],
                vec![10, 20, 30, 255],
            ),
            (
                RawImageEncoding::Rgba8,
                vec![10, 20, 30, 40],
                vec![10, 20, 30, 40],
            ),
            (
                RawImageEncoding::Bgra8,
                vec![30, 20, 10, 40],
                vec![10, 20, 30, 40],
            ),
            (RawImageEncoding::Mono8, vec![17], vec![17, 17, 17, 255]),
        ] {
            let mut data = pixel.repeat(4);
            data.extend_from_slice(&[99, 99]);
            let step = data.len() as u32;
            data.extend_from_slice(&vec![0; step as usize]);
            let frame = frame(
                CameraImageFormat::Raw(RawImageLayout {
                    width: 4,
                    height: 2,
                    step,
                    encoding,
                }),
                data,
            );
            let decoded = decode_camera_image(&frame, (2, 1)).unwrap();
            assert_eq!((decoded.width, decoded.height), (2, 1));
            assert_eq!(decoded.rgba, expected.repeat(2));
        }
    }
    #[test]
    fn png_preserves_color_alpha_and_bounds_and_rejects_depth() {
        use image::{ExtendedColorType, ImageEncoder, codecs::png::PngEncoder};
        let mut bytes = Vec::new();
        PngEncoder::new(&mut bytes)
            .write_image(
                &[10, 20, 30, 40, 50, 60, 70, 80],
                2,
                1,
                ExtendedColorType::Rgba8,
            )
            .unwrap();
        let frame = frame(CameraImageFormat::Png, bytes);
        assert_eq!(camera_dimensions(&frame).unwrap(), (2, 1));
        assert_eq!(
            decode_camera_image(&frame, (2, 1)).unwrap().rgba,
            vec![10, 20, 30, 40, 50, 60, 70, 80]
        );
        assert_eq!(decode_camera_image(&frame, (1, 1)).unwrap().rgba.len(), 4);
        for length in 0..frame.data.len() / 2 {
            let mut bad = frame.clone();
            bad.data = bad.data.slice(..length);
            assert!(decode_camera_image(&bad, (2, 1)).is_err());
        }
        let mut depth = Vec::new();
        PngEncoder::new(&mut depth)
            .write_image(&[0, 1], 1, 1, ExtendedColorType::L16)
            .unwrap();
        assert!(camera_dimensions(&self::frame(CameraImageFormat::Png, depth)).is_err());
    }
}
