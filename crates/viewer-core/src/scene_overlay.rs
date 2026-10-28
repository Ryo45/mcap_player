//! Validated, source-frame metric lines. No Wasm or GPU ownership.

use std::{error::Error, fmt};

pub const MAX_OVERLAY_LINES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneLine {
    /// Stable within this extension's complete replacement snapshot.
    pub id: u32,
    pub start: [f32; 3],
    pub end: [f32; 3],
    /// Opaque linear RGB. Initial rendering uses one-pixel depth-tested lines.
    pub color: [f32; 3],
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SceneOverlay {
    lines: Vec<SceneLine>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SceneOverlayError;

impl fmt::Display for SceneOverlayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(
            "overlay exceeds limits or contains invalid coordinates, colors or duplicate IDs",
        )
    }
}
impl Error for SceneOverlayError {}

impl SceneOverlay {
    pub fn new(lines: Vec<SceneLine>) -> Result<Self, SceneOverlayError> {
        let mut ids = std::collections::BTreeSet::new();
        if lines.len() > MAX_OVERLAY_LINES
            || lines.iter().any(|line| {
                !ids.insert(line.id)
                    || line
                        .start
                        .iter()
                        .chain(&line.end)
                        .any(|v| !v.is_finite() || v.abs() > 1e6)
                    || line
                        .color
                        .iter()
                        .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
            })
        {
            return Err(SceneOverlayError);
        }
        Ok(Self { lines })
    }

    pub fn lines(&self) -> &[SceneLine] {
        &self.lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_all_external_geometry() {
        let line = SceneLine {
            id: 1,
            start: [0.; 3],
            end: [1.; 3],
            color: [1.; 3],
        };
        assert!(SceneOverlay::new(vec![line]).is_ok());
        assert!(SceneOverlay::new(vec![line, line]).is_err());
        assert!(
            SceneOverlay::new(vec![SceneLine {
                start: [f32::NAN; 3],
                ..line
            }])
            .is_err()
        );
        assert!(
            SceneOverlay::new(vec![SceneLine {
                color: [2.; 3],
                ..line
            }])
            .is_err()
        );
        assert!(SceneOverlay::new(vec![line; MAX_OVERLAY_LINES + 1]).is_err());
    }
}
