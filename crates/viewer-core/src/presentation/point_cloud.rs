//! Source attributes become linear RGBA here, independently of the GPU backend.
use crate::PointCloudAttributes;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PointCloudColorMode {
    #[default]
    Auto,
    Solid,
    Rgb,
    Intensity,
}

impl PointCloudColorMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Solid => "Solid",
            Self::Rgb => "RGB / RGBA",
            Self::Intensity => "Intensity",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PointCloudColoring {
    #[serde(default)]
    pub mode: PointCloudColorMode,
    /// Fixed producer-value range, clamped to a blue/cyan/green/yellow/red ramp.
    /// Keeping the range fixed avoids per-frame rescaling and flicker.
    #[serde(default = "default_range")]
    pub intensity_range: [f32; 2],
}

const fn default_range() -> [f32; 2] {
    [0.0, 255.0]
}
// Linear equivalents of the existing sRGB cyan (0.20, 0.82, 1.0).
pub(crate) const SOLID_COLOR: [f32; 4] = [0.033_104_766, 0.638_283_2, 1.0, 0.88];

impl Default for PointCloudColoring {
    fn default() -> Self {
        Self {
            mode: PointCloudColorMode::Auto,
            intensity_range: default_range(),
        }
    }
}

impl PointCloudColoring {
    pub fn is_valid(self) -> bool {
        self.intensity_range.iter().all(|v| v.is_finite())
            && self.intensity_range[0] < self.intensity_range[1]
            && (self.intensity_range[1] - self.intensity_range[0]).is_finite()
    }

    pub(crate) fn colors(self, attributes: &PointCloudAttributes, count: usize) -> Vec<[f32; 4]> {
        let mode = match self.mode {
            PointCloudColorMode::Auto if attributes.rgba.is_some() => PointCloudColorMode::Rgb,
            PointCloudColorMode::Auto if attributes.intensity.is_some() => {
                PointCloudColorMode::Intensity
            }
            PointCloudColorMode::Auto => PointCloudColorMode::Solid,
            mode => mode,
        };
        (0..count)
            .map(|index| match mode {
                PointCloudColorMode::Rgb => attributes
                    .rgba
                    .as_ref()
                    .and_then(|v| v.get(index))
                    .map_or(SOLID_COLOR, |[r, g, b, a]| {
                        [
                            linear_byte(*r),
                            linear_byte(*g),
                            linear_byte(*b),
                            *a as f32 / 255.0,
                        ]
                    }),
                PointCloudColorMode::Intensity if self.is_valid() => attributes
                    .intensity
                    .as_ref()
                    .and_then(|v| v.get(index))
                    .filter(|v| v.is_finite())
                    .map_or(SOLID_COLOR, |value| {
                        let t = ((*value - self.intensity_range[0])
                            / (self.intensity_range[1] - self.intensity_range[0]))
                            .clamp(0.0, 1.0)
                            * 4.0;
                        let rgb = match t {
                            t if t < 1.0 => [0.0, t, 1.0],
                            t if t < 2.0 => [0.0, 1.0, 2.0 - t],
                            t if t < 3.0 => [t - 2.0, 1.0, 0.0],
                            t => [1.0, 4.0 - t, 0.0],
                        };
                        [linear(rgb[0]), linear(rgb[1]), linear(rgb[2]), 1.0]
                    }),
                _ => SOLID_COLOR,
            })
            .collect()
    }

    pub(crate) fn missing_attribute(
        self,
        attributes: &PointCloudAttributes,
    ) -> Option<&'static str> {
        match self.mode {
            PointCloudColorMode::Rgb if attributes.rgba.is_none() => {
                Some("RGB unavailable; using solid color")
            }
            PointCloudColorMode::Intensity if attributes.intensity.is_none() => {
                Some("Intensity unavailable; using solid color")
            }
            _ if !self.is_valid() => {
                Some("Invalid intensity range; using solid color for intensity")
            }
            _ => None,
        }
    }
}

fn linear_byte(value: u8) -> f32 {
    static VALUES: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    VALUES.get_or_init(|| std::array::from_fn(|i| linear(i as f32 / 255.0)))[usize::from(value)]
}

fn linear(srgb: f32) -> f32 {
    if srgb <= 0.04045 {
        srgb / 12.92
    } else {
        ((srgb + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn modes_preserve_rgb_alpha_and_use_a_fixed_clamped_intensity_range() {
        let attributes = PointCloudAttributes {
            rgba: Some(vec![[255, 0, 128, 64]]),
            intensity: Some(vec![0.5]),
        };
        let rgb = PointCloudColoring::default().colors(&attributes, 1)[0];
        assert_eq!([rgb[0], rgb[1], rgb[3]], [1.0, 0.0, 64.0 / 255.0]);
        assert!((rgb[2] - 0.21586).abs() < 0.0001);
        let coloring = PointCloudColoring {
            mode: PointCloudColorMode::Intensity,
            intensity_range: [0.0, 1.0],
        };
        let attributes = PointCloudAttributes {
            rgba: None,
            intensity: Some(vec![-1.0, 0.5, 2.0, f32::NAN]),
        };
        assert_eq!(
            coloring.colors(&attributes, 4),
            vec![
                [0.0, 0.0, 1.0, 1.0],
                [0.0, 1.0, 0.0, 1.0],
                [1.0, 0.0, 0.0, 1.0],
                SOLID_COLOR
            ]
        );
        assert!(
            !PointCloudColoring {
                intensity_range: [1.0, 1.0],
                ..coloring
            }
            .is_valid()
        );
        assert!(
            coloring
                .missing_attribute(&PointCloudAttributes::default())
                .is_some()
        );
    }
}
