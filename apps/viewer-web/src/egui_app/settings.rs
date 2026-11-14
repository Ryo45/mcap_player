//! Pending source bindings and independent physical facts. No session mutation or file I/O.
use crate::workspace_bindings::BindingsSelection;
use viewer_core::{
    CameraCalibrationSet, PathProjectionPolicy, PlatformGeometry, WorkspaceBindings,
};

pub(super) struct OpenSettings {
    pub bindings: BindingsSelection,
    pub point_cloud: String,
    pub requirements: viewer_core::PlaybackRequirements,
}
impl Default for OpenSettings {
    fn default() -> Self {
        Self {
            bindings: BindingsSelection::default(),
            point_cloud: String::new(),
            requirements: viewer_core::PlaybackRequirements::empty(),
        }
    }
}
impl OpenSettings {
    pub fn resolve(&self) -> Result<WorkspaceBindings, String> {
        self.bindings.resolve(&self.point_cloud)
    }
}
pub(super) struct PhysicalSettings {
    pub platform: Option<PlatformGeometry>,
    pub enabled: bool,
    pub calibrations: CameraCalibrationSet,
    pub projection: PathProjectionPolicy,
    pub fixed_frame: String,
    pub ground_frame: String,
    pub error: String,
    pub calibration_label: String,
    pub platform_label: String,
}
impl PhysicalSettings {
    pub fn bundled() -> Result<Self, String> {
        Ok(Self {
            platform: Some(
                serde_json::from_str(include_str!(
                    "../../../../config/platform_geometry/turtlebot3_waffle_pi.json"
                ))
                .map_err(|e| e.to_string())?,
            ),
            enabled: true,
            calibrations: CameraCalibrationSet::from_json(include_str!(
                "../../../../config/camera_calibration.json"
            ))
            .map_err(|e| e.to_string())?,
            projection: PathProjectionPolicy::default(),
            fixed_frame: "odom".into(),
            ground_frame: String::new(),
            error: String::new(),
            calibration_label: "Bundled demo calibration (not measured)".into(),
            platform_label: "Bundled Waffle Pi physical bounds".into(),
        })
    }
    pub fn apply_frames(&mut self) -> bool {
        let fixed = self.fixed_frame.trim();
        let ground = self.ground_frame.trim();
        if fixed.is_empty() || fixed.len() > 256 || ground.len() > 256 {
            self.error = "World frame must be named; frame names must be at most 256 bytes".into();
            return false;
        }
        self.projection = PathProjectionPolicy {
            fixed_frame: fixed.into(),
            ground_frame: (!ground.is_empty()).then(|| ground.into()),
        };
        self.error.clear();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_frame_edit_preserves_the_active_projection_policy() {
        let mut settings = PhysicalSettings::bundled().unwrap();
        settings.ground_frame = "base_footprint".into();
        assert!(settings.apply_frames());
        settings.fixed_frame.clear();
        assert!(!settings.apply_frames());
        assert_eq!(settings.projection.fixed_frame, "odom");
        assert_eq!(
            settings.projection.ground_frame.as_deref(),
            Some("base_footprint")
        );
        assert!(!settings.error.is_empty());
    }
}
