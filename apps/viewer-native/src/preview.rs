use crate::interaction::PreviewDragState;
use std::{collections::BTreeMap, fs, path::Path};
use viewer_core::{
    ArrivalTime, PreviewBudget, PreviewRequest, PreviewSnapshot, SignalId, SourceFingerprint,
};
use viewer_preview_mcap::{PreviewArtifact, read_preview_mcap};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) enum PreviewStatus {
    #[default]
    Unavailable,
    Ready,
    Warning(String),
}

#[derive(Default)]
pub(crate) struct PreviewCoordinator {
    artifact: Option<PreviewArtifact>,
    latest_snapshot: Option<PreviewSnapshot>,
    pub(crate) drag: PreviewDragState,
    status: PreviewStatus,
    camera_ids: BTreeMap<viewer_core::CameraId, viewer_core::CameraId>,
    camera_binding_revision: u64,
}

impl PreviewCoordinator {
    pub(crate) fn clear(&mut self) {
        self.artifact = None;
        self.latest_snapshot = None;
        self.drag.clear();
        self.status = PreviewStatus::Unavailable;
        self.camera_ids.clear();
        self.camera_binding_revision = self.camera_binding_revision.wrapping_add(1);
    }

    pub(crate) fn load_for_source(&mut self, source_path: &Path, expected: &SourceFingerprint) {
        self.clear();
        let directory = if source_path.is_dir() {
            source_path
        } else {
            source_path.parent().unwrap_or_else(|| Path::new("."))
        };
        let preview_path = directory.join("preview.mcap");
        if !preview_path.exists() {
            self.status = PreviewStatus::Warning(format!(
                "Preview unavailable: {} was not found",
                preview_path.display()
            ));
            return;
        }
        let result = fs::read(&preview_path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| read_preview_mcap(&bytes).map_err(|error| error.to_string()))
            .and_then(|artifact| {
                artifact
                    .validate_source(expected)
                    .map_err(|error| error.to_string())?;
                Ok(artifact)
            });
        match result {
            Ok(artifact) => {
                self.artifact = Some(artifact);
                self.status = PreviewStatus::Ready;
                self.update(None);
            }
            Err(error) => {
                self.status = PreviewStatus::Warning(format!("Preview disabled: {error}"));
            }
        }
    }

    pub(crate) fn bind_cameras(&mut self, plan: &viewer_core::SessionPlan) {
        let mut camera_ids = BTreeMap::new();
        if let Some(artifact) = &self.artifact {
            for (source_id, topic) in artifact.build_info.camera_topics() {
                if let Some(route) = plan
                    .camera_routes()
                    .iter()
                    .find(|route| route.stream.topic == *topic)
                {
                    camera_ids.insert(*source_id, route.camera_id);
                }
            }
        }
        if camera_ids != self.camera_ids {
            self.camera_ids = camera_ids;
            self.camera_binding_revision = self.camera_binding_revision.wrapping_add(1);
        }
        self.update(None);
    }

    pub(crate) fn camera_binding_revision(&self) -> u64 {
        self.camera_binding_revision
    }

    pub(crate) fn update(&mut self, target_time: Option<ArrivalTime>) {
        let Some(artifact) = &self.artifact else {
            self.latest_snapshot = None;
            return;
        };
        let Some(range) = artifact.available_range else {
            self.latest_snapshot = None;
            return;
        };
        let request = PreviewRequest {
            range,
            target_time,
            camera_ids: self.camera_ids.keys().copied().collect(),
            signal_ids: artifact.signal_overviews.keys().copied().collect(),
            budget: PreviewBudget {
                max_camera_frames: artifact.camera_frames.len(),
                max_signal_buckets_per_signal: 4_000,
                max_trajectory_points: 2_000,
            },
        };
        match artifact.query(&request) {
            Ok(snapshot) => {
                self.latest_snapshot = Some(snapshot.remap_camera_ids(&self.camera_ids))
            }
            Err(error) => {
                self.latest_snapshot = None;
                self.status = PreviewStatus::Warning(format!("Preview query failed: {error}"));
            }
        }
    }

    pub(crate) fn snapshot(&self) -> Option<&PreviewSnapshot> {
        self.latest_snapshot.as_ref()
    }

    pub(crate) fn speed_overview(&self) -> Option<&viewer_core::SignalOverview> {
        self.latest_snapshot
            .as_ref()?
            .signal_overviews()
            .iter()
            .find(|overview| overview.signal_id() == SignalId::Speed)
    }

    pub(crate) fn warning(&self) -> Option<&str> {
        match &self.status {
            PreviewStatus::Warning(message) => Some(message),
            PreviewStatus::Unavailable | PreviewStatus::Ready => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use viewer_core::{
        CameraId, CameraPreviewFrame, PlaybackRequirements, PreviewBuildInfo, PreviewImageEncoding,
        SessionPlan, SourceCatalog, StreamDescriptor, StreamId, StreamTimingSummary, TimeRange,
        WorkspaceBindings,
    };

    #[test]
    fn preview_follows_source_topics_across_subset_selection_id_changes_and_return() {
        let info = PreviewBuildInfo::new(
            "test",
            "0",
            SourceFingerprint::new("sha256", "fixture").unwrap(),
            BTreeMap::from([
                (CameraId(0), "/front".into()),
                (CameraId(1), "/rear".into()),
            ]),
        )
        .unwrap();
        let frame = |id, byte| {
            CameraPreviewFrame::new(
                CameraId(id),
                None,
                ArrivalTime(100),
                "optical".into(),
                PreviewImageEncoding::Jpeg,
                1,
                1,
                vec![byte],
            )
            .unwrap()
        };
        let mut coordinator = PreviewCoordinator {
            artifact: Some(PreviewArtifact {
                build_info: info,
                available_range: Some(TimeRange::new(ArrivalTime(100), ArrivalTime(200)).unwrap()),
                camera_frames: BTreeMap::from([
                    (CameraId(0), vec![frame(0, 11)]),
                    (CameraId(1), vec![frame(1, 22)]),
                ]),
                signal_overviews: BTreeMap::new(),
                trajectory: vec![],
            }),
            ..Default::default()
        };
        let descriptor = |id, topic: &str| StreamDescriptor {
            id: StreamId(id),
            topic: topic.into(),
            schema: "sensor_msgs/msg/CompressedImage".into(),
            schema_definition: None,
            message_encoding: "cdr".into(),
            timing: StreamTimingSummary::default(),
        };
        let catalog = SourceCatalog {
            streams: vec![
                descriptor(1, "/front"),
                descriptor(2, "/rear"),
                descriptor(3, "/unbuilt"),
            ],
            ..Default::default()
        };
        let bindings = WorkspaceBindings {
            path_topic: "/path".into(),
            odometry_topic: "/odom".into(),
            point_cloud_topic: "/scan".into(),
            dynamic_tf_topic: "/tf".into(),
            static_tf_topic: "/tf_static".into(),
        };
        for (topic, expected) in [
            ("/front", Some(11)),
            ("/rear", Some(22)),
            ("/unbuilt", None),
            ("/front", Some(11)),
        ] {
            let mut requirements = PlaybackRequirements::empty();
            requirements.require_camera_topic(topic);
            let plan = SessionPlan::build(&catalog, topic, &requirements, &bindings).unwrap();
            let revision = coordinator.camera_binding_revision();
            coordinator.bind_cameras(&plan);
            assert_ne!(coordinator.camera_binding_revision(), revision);
            coordinator.update(Some(ArrivalTime(150)));
            let frames = coordinator.snapshot().unwrap().camera_frames();
            if let Some(byte) = expected {
                assert_eq!(frames.len(), 1);
                assert_eq!(frames[0].camera_id(), CameraId(0));
                assert_eq!(frames[0].bytes(), [byte]);
            } else {
                assert!(frames.is_empty());
            }
        }
    }
}
