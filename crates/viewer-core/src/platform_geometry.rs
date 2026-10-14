//! Source-neutral physical bounds in a named ROS frame. All distances are metres.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "GeometryWire")]
pub struct PlatformGeometry {
    reference_frame: String,
    min: [f64; 3],
    max: [f64; 3],
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GeometryWire {
    reference_frame: String,
    min: [f64; 3],
    max: [f64; 3],
}
#[derive(Clone, Debug, PartialEq)]
pub struct PlatformGeometryError;
impl std::fmt::Display for PlatformGeometryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "platform geometry requires a named frame and finite, strictly ordered metre bounds"
        )
    }
}
impl std::error::Error for PlatformGeometryError {}
impl TryFrom<GeometryWire> for PlatformGeometry {
    type Error = PlatformGeometryError;
    fn try_from(w: GeometryWire) -> Result<Self, Self::Error> {
        Self::new(w.reference_frame, w.min, w.max)
    }
}
impl PlatformGeometry {
    pub fn new(
        reference_frame: String,
        min: [f64; 3],
        max: [f64; 3],
    ) -> Result<Self, PlatformGeometryError> {
        let reference_frame = reference_frame.trim().trim_start_matches('/').to_owned();
        if reference_frame.is_empty()
            || reference_frame.len() > 256
            || !(0..3).all(|i| {
                min[i].is_finite()
                    && max[i].is_finite()
                    && min[i] < max[i]
                    && (max[i] - min[i]).is_finite()
            })
        {
            return Err(PlatformGeometryError);
        }
        Ok(Self {
            reference_frame,
            min,
            max,
        })
    }
    pub fn reference_frame(&self) -> &str {
        &self.reference_frame
    }
    pub fn min(&self) -> [f64; 3] {
        self.min
    }
    pub fn max(&self) -> [f64; 3] {
        self.max
    }
    pub fn extent(&self) -> [f64; 3] {
        std::array::from_fn(|i| self.max[i] - self.min[i])
    }
    pub fn corners(&self) -> [[f64; 3]; 8] {
        let [x0, y0, z0] = self.min;
        let [x1, y1, z1] = self.max;
        [
            [x0, y0, z0],
            [x1, y0, z0],
            [x1, y1, z0],
            [x0, y1, z0],
            [x0, y0, z1],
            [x1, y0, z1],
            [x1, y1, z1],
            [x0, y1, z1],
        ]
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn asymmetric_bounds_remain_relative_to_reference_frame() {
        let g =
            PlatformGeometry::new("base_link".into(), [-1.0, -0.9, -0.2], [3.0, 0.9, 1.4]).unwrap();
        assert_eq!(g.extent(), [4.0, 1.8, 1.5999999999999999]);
        assert_eq!(g.corners()[0], [-1.0, -0.9, -0.2]);
        for (min, max) in [
            ([0.; 3], [0.; 3]),
            ([f64::NAN; 3], [1.; 3]),
            ([1.; 3], [0.; 3]),
        ] {
            assert!(PlatformGeometry::new("base_link".into(), min, max).is_err());
        }
        assert!(PlatformGeometry::new("".into(), [0.; 3], [1.; 3]).is_err());
    }
}
