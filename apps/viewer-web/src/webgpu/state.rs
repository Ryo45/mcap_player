pub(crate) const MAX_PHYSICAL_SIZE: u32 = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PhysicalSize {
    pub(crate) width: u32,
    pub(crate) height: u32,
}

pub(crate) fn physical_size(
    logical_width: i32,
    logical_height: i32,
    device_pixel_ratio: f64,
) -> Option<PhysicalSize> {
    if logical_width <= 0 || logical_height <= 0 || !device_pixel_ratio.is_finite() {
        return None;
    }
    let scale = device_pixel_ratio.clamp(0.25, 4.0);
    Some(PhysicalSize {
        width: (f64::from(logical_width) * scale)
            .round()
            .clamp(1.0, f64::from(MAX_PHYSICAL_SIZE)) as u32,
        height: (f64::from(logical_height) * scale)
            .round()
            .clamp(1.0, f64::from(MAX_PHYSICAL_SIZE)) as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_logical_size_with_dpr() {
        assert_eq!(
            physical_size(320, 180, 2.0),
            Some(PhysicalSize {
                width: 640,
                height: 360,
            })
        );
    }

    #[test]
    fn rejects_zero_size_and_clamps_large_surfaces() {
        assert_eq!(physical_size(0, 180, 2.0), None);
        assert_eq!(physical_size(320, 0, 2.0), None);
        assert_eq!(
            physical_size(10_000, 10_000, 4.0),
            Some(PhysicalSize {
                width: MAX_PHYSICAL_SIZE,
                height: MAX_PHYSICAL_SIZE,
            })
        );
    }
}
