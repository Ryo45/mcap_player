//! Startup-only file I/O; parsing/validation remains source/platform neutral.
use anyhow::{Context, Result};
use std::{fs::File, io::Read, path::Path};
use viewer_core::WorkspaceBindings;
pub(crate) fn load(path: Option<&Path>, point_cloud: Option<&str>) -> Result<WorkspaceBindings> {
    let bindings = if let Some(path) = path {
        let mut json = String::new();
        File::open(path)
            .with_context(|| format!("open workspace bindings {}", path.display()))?
            .take((WorkspaceBindings::MAX_JSON_BYTES + 1) as u64)
            .read_to_string(&mut json)
            .with_context(|| format!("read workspace bindings {}", path.display()))?;
        WorkspaceBindings::from_json(&json)
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("workspace bindings {}", path.display()))?
    } else {
        crate::workspace::bundled_workspace_bindings()
    };
    bindings
        .with_point_cloud_override(point_cloud)
        .map_err(anyhow::Error::msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn external_json_and_explicit_cli_override_are_validated_before_use() {
        assert_eq!(
            load(None, None).unwrap(),
            crate::workspace::bundled_workspace_bindings()
        );
        let path = std::env::temp_dir().join(format!("mcap-bindings-{}.json", std::process::id()));
        let mut expected = crate::workspace::bundled_workspace_bindings();
        expected.point_cloud_topic = "/robot/cloud".into();
        expected.path_topic = "/robot/path".into();
        std::fs::write(&path, serde_json::to_vec(&expected).unwrap()).unwrap();
        assert_eq!(load(Some(&path), None).unwrap(), expected);
        let effective = load(Some(&path), Some("/cli")).unwrap();
        assert_eq!(effective.point_cloud_topic, "/cli");
        assert_eq!(effective.path_topic, expected.path_topic);
        std::fs::write(&path, r#"{"pointCloudTopic":"/cloud","unknown":true}"#).unwrap();
        assert!(load(Some(&path), Some("/cli")).is_err());
        std::fs::write(&path, [0xff]).unwrap();
        assert!(load(Some(&path), None).is_err());
        std::fs::write(&path, "x".repeat(WorkspaceBindings::MAX_JSON_BYTES + 1)).unwrap();
        assert!(load(Some(&path), None).is_err());
        std::fs::remove_file(&path).unwrap();
        assert!(load(Some(&path), None).is_err());
    }
}
