//! Settings for the next source, independent of DOM/I/O and of active playback.
use viewer_core::WorkspaceBindings;
const BUNDLED: &str = include_str!("../../../config/workspace_bindings.json");
pub(crate) fn bundled() -> WorkspaceBindings {
    WorkspaceBindings::from_json(BUNDLED).expect("bundled workspace bindings")
}
pub(crate) struct BindingsSelection {
    generation: u64,
    bindings: Result<WorkspaceBindings, String>,
    pub label: String,
}
impl Default for BindingsSelection {
    fn default() -> Self {
        Self {
            generation: 0,
            bindings: Ok(bundled()),
            label: "Bundled default".into(),
        }
    }
}
impl BindingsSelection {
    pub fn begin(&mut self, name: String) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.label = name;
        self.bindings = Err("Reading workspace bindings…".into());
        self.generation
    }
    pub fn finish(&mut self, generation: u64, result: Result<WorkspaceBindings, String>) {
        if self.generation == generation {
            self.bindings = result;
        }
    }
    pub fn reset(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.bindings = Ok(bundled());
        self.label = "Bundled default".into();
    }
    pub fn resolve(&self, point_cloud: &str) -> Result<WorkspaceBindings, String> {
        self.bindings
            .clone()?
            .with_point_cloud_override((!point_cloud.is_empty()).then_some(point_cloud))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_errors_block_open_until_explicit_recovery_and_ignore_stale_reads() {
        let mut settings = BindingsSelection::default();
        assert_eq!(settings.resolve("").unwrap(), bundled());
        let first = settings.begin("first.json".into());
        assert!(settings.resolve("").is_err());
        let second = settings.begin("bad.json".into());
        settings.finish(first, Ok(bundled()));
        assert!(settings.resolve("").is_err());
        settings.finish(second, WorkspaceBindings::from_json("{broken"));
        assert!(settings.resolve("/override").is_err());
        assert_eq!(settings.label, "bad.json");
        let third = settings.begin("robot.json".into());
        let mut robot = bundled();
        robot.point_cloud_topic = "/robot/cloud".into();
        robot.path_topic = "/robot/path".into();
        settings.finish(third, Ok(robot.clone()));
        assert_eq!(settings.resolve("").unwrap(), robot);
        let overridden = settings.resolve("/manual").unwrap();
        assert_eq!(overridden.point_cloud_topic, "/manual");
        assert_eq!(overridden.path_topic, robot.path_topic);
        assert!(settings.resolve(" ").is_err());
        let pending = settings.begin("late.json".into());
        settings.reset();
        settings.finish(pending, Err("late error".into()));
        assert_eq!(settings.resolve("").unwrap(), bundled());
    }
}
