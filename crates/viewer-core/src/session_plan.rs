use crate::{
    CameraController, CameraId, OdometryController, PathController, RestoreInput, SceneController,
    SourceCatalog, StreamDescriptor, StreamId, TransformController,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fmt};

const COMPRESSED_IMAGE_SCHEMA: &str = "sensor_msgs/msg/CompressedImage";
const PATH_SCHEMA: &str = "nav_msgs/msg/Path";
const ODOMETRY_SCHEMA: &str = "nav_msgs/msg/Odometry";
const LASER_SCAN_SCHEMA: &str = "sensor_msgs/msg/LaserScan";
const TF_MESSAGE_SCHEMA: &str = "tf2_msgs/msg/TFMessage";

/// Continuous inputs required by the currently open workspace.
///
/// This is deliberately a small, closed set rather than a generic capability registry. Plot,
/// Inspector, and Preview requests use their existing independent data paths.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlaybackRequirements {
    all_cameras: bool,
    camera_topics: BTreeSet<String>,
    path: bool,
    odometry: bool,
    point_cloud: bool,
    dynamic_transforms: bool,
    static_transforms: bool,
}

impl PlaybackRequirements {
    pub fn empty() -> Self {
        Self {
            all_cameras: false,
            camera_topics: BTreeSet::new(),
            path: false,
            odometry: false,
            point_cloud: false,
            dynamic_transforms: false,
            static_transforms: false,
        }
    }

    pub fn require_all_cameras(&mut self) {
        self.all_cameras = true;
    }

    pub fn require_camera_topic(&mut self, topic: impl Into<String>) {
        self.camera_topics.insert(topic.into());
    }

    pub fn require_path(&mut self) {
        self.path = true;
    }

    pub fn require_odometry(&mut self) {
        self.odometry = true;
    }

    pub fn require_point_cloud(&mut self) {
        self.point_cloud = true;
    }

    pub fn require_transforms(&mut self) {
        self.dynamic_transforms = true;
        self.static_transforms = true;
    }
}

/// Workspace-owned bindings from semantic continuous inputs to source topics.
///
/// These are configuration facts, not recording facts and not viewer-core policy. Cameras remain
/// panel-selected because separate Camera panels may request different topics.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceBindings {
    pub path_topic: String,
    pub odometry_topic: String,
    pub point_cloud_topic: String,
    pub dynamic_tf_topic: String,
    pub static_tf_topic: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CameraRoute {
    pub stream: StreamDescriptor,
    pub camera_id: CameraId,
}

/// Fixed source selection and explicit feature routes resolved when a session opens.
///
/// It contains no cursor, controller state, decode queue, or query result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionPlan {
    camera_routes: Vec<CameraRoute>,
    path_stream: Option<StreamDescriptor>,
    odometry_stream: Option<StreamDescriptor>,
    point_cloud_stream: Option<StreamDescriptor>,
    dynamic_tf_stream: Option<StreamDescriptor>,
    static_tf_stream: Option<StreamDescriptor>,
    primary_camera: Option<CameraId>,
}

impl SessionPlan {
    pub fn build(
        catalog: &SourceCatalog,
        priority_camera_topic: &str,
        requirements: &PlaybackRequirements,
        bindings: &WorkspaceBindings,
    ) -> Result<Self, SessionPlanError> {
        let available_cameras = catalog
            .streams
            .iter()
            .filter(|stream| accepts(stream, COMPRESSED_IMAGE_SCHEMA))
            .cloned()
            .collect::<Vec<_>>();

        let mut selected_cameras = available_cameras
            .into_iter()
            .filter(|stream| {
                requirements.all_cameras || requirements.camera_topics.contains(&stream.topic)
            })
            .collect::<Vec<_>>();
        let priority_is_required =
            requirements.all_cameras || requirements.camera_topics.contains(priority_camera_topic);
        if priority_is_required
            && !selected_cameras
                .iter()
                .any(|stream| stream.topic == priority_camera_topic)
        {
            return Err(SessionPlanError(format!(
                "priority Camera topic {priority_camera_topic} is not a selected CDR CompressedImage stream"
            )));
        }
        if let Some(priority_index) = selected_cameras
            .iter()
            .position(|stream| stream.topic == priority_camera_topic)
        {
            selected_cameras.swap(0, priority_index);
        }
        let primary_camera = selected_cameras
            .first()
            .filter(|stream| stream.topic == priority_camera_topic)
            .map(|_| CameraId(0));
        let selected_cameras = selected_cameras
            .into_iter()
            .enumerate()
            .map(|(index, stream)| {
                let camera_id = u16::try_from(index).map(CameraId).map_err(|_| {
                    SessionPlanError("source contains too many selected Camera streams".into())
                })?;
                Ok(CameraRoute { stream, camera_id })
            })
            .collect::<Result<Vec<_>, SessionPlanError>>()?;

        Ok(Self {
            camera_routes: selected_cameras,
            path_stream: requirements
                .path
                .then(|| select_stream(catalog, &bindings.path_topic, PATH_SCHEMA))
                .flatten(),
            odometry_stream: requirements
                .odometry
                .then(|| select_stream(catalog, &bindings.odometry_topic, ODOMETRY_SCHEMA))
                .flatten(),
            point_cloud_stream: requirements
                .point_cloud
                .then(|| select_stream(catalog, &bindings.point_cloud_topic, LASER_SCAN_SCHEMA))
                .flatten(),
            dynamic_tf_stream: requirements
                .dynamic_transforms
                .then(|| select_stream(catalog, &bindings.dynamic_tf_topic, TF_MESSAGE_SCHEMA))
                .flatten(),
            static_tf_stream: requirements
                .static_transforms
                .then(|| select_stream(catalog, &bindings.static_tf_topic, TF_MESSAGE_SCHEMA))
                .flatten(),
            primary_camera,
        })
    }

    pub fn camera_routes(&self) -> &[CameraRoute] {
        &self.camera_routes
    }

    pub fn path_stream(&self) -> Option<&StreamDescriptor> {
        self.path_stream.as_ref()
    }

    pub fn odometry_stream(&self) -> Option<&StreamDescriptor> {
        self.odometry_stream.as_ref()
    }

    pub fn point_cloud_stream(&self) -> Option<&StreamDescriptor> {
        self.point_cloud_stream.as_ref()
    }

    pub fn dynamic_tf_stream(&self) -> Option<&StreamDescriptor> {
        self.dynamic_tf_stream.as_ref()
    }

    pub fn static_tf_stream(&self) -> Option<&StreamDescriptor> {
        self.static_tf_stream.as_ref()
    }

    pub fn primary_camera(&self) -> Option<CameraId> {
        self.primary_camera
    }

    pub fn camera_topics(&self) -> Vec<(CameraId, String)> {
        self.camera_routes
            .iter()
            .map(|route| (route.camera_id, route.stream.topic.clone()))
            .collect()
    }

    pub fn primary_camera_topic(&self) -> Option<&str> {
        let primary = self.primary_camera?;
        self.camera_routes
            .iter()
            .find(|route| route.camera_id == primary)
            .map(|route| route.stream.topic.as_str())
    }

    pub fn selected_stream_ids(&self) -> Vec<StreamId> {
        let mut ids = self
            .camera_routes
            .iter()
            .map(|route| route.stream.id)
            .chain(self.feature_streams().map(|stream| stream.id))
            .collect::<Vec<_>>();
        ids.sort_by_key(|id| id.0);
        ids.dedup();
        ids
    }

    pub fn selected_topics(&self) -> Vec<String> {
        let mut topics = self
            .camera_routes
            .iter()
            .map(|route| route.stream.topic.clone())
            .chain(self.feature_streams().map(|stream| stream.topic.clone()))
            .collect::<Vec<_>>();
        topics.sort();
        topics.dedup();
        topics
    }

    pub fn restore_inputs(&self) -> Vec<RestoreInput> {
        self.camera_routes
            .iter()
            .map(|route| RestoreInput {
                stream_id: route.stream.id,
                semantics: CameraController::restore_semantics(),
            })
            .chain(self.path_stream.iter().map(|stream| RestoreInput {
                stream_id: stream.id,
                semantics: PathController::restore_semantics(),
            }))
            .chain(self.odometry_stream.iter().map(|stream| RestoreInput {
                stream_id: stream.id,
                semantics: OdometryController::restore_semantics(),
            }))
            .chain(self.point_cloud_stream.iter().map(|stream| RestoreInput {
                stream_id: stream.id,
                semantics: SceneController::restore_semantics(),
            }))
            .chain(self.dynamic_tf_stream.iter().map(|stream| RestoreInput {
                stream_id: stream.id,
                semantics: TransformController::dynamic_restore_semantics(),
            }))
            .chain(self.static_tf_stream.iter().map(|stream| RestoreInput {
                stream_id: stream.id,
                semantics: TransformController::static_restore_semantics(),
            }))
            .collect()
    }

    fn feature_streams(&self) -> impl Iterator<Item = &StreamDescriptor> {
        [
            self.path_stream.as_ref(),
            self.odometry_stream.as_ref(),
            self.point_cloud_stream.as_ref(),
            self.dynamic_tf_stream.as_ref(),
            self.static_tf_stream.as_ref(),
        ]
        .into_iter()
        .flatten()
    }
}

fn select_stream(
    catalog: &SourceCatalog,
    topic: &str,
    expected_schema: &str,
) -> Option<StreamDescriptor> {
    catalog
        .streams
        .iter()
        .find(|stream| stream.topic == topic && accepts(stream, expected_schema))
        .cloned()
}

fn accepts(stream: &StreamDescriptor, expected_schema: &str) -> bool {
    stream.message_encoding == "cdr" && stream.schema == expected_schema
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionPlanError(String);

impl fmt::Display for SessionPlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for SessionPlanError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{StreamId, StreamTimingSummary};

    fn descriptor(id: u32, topic: &str, schema: &str) -> StreamDescriptor {
        StreamDescriptor {
            id: StreamId(id),
            topic: topic.into(),
            schema: schema.into(),
            message_encoding: "cdr".into(),
            timing: StreamTimingSummary::default(),
        }
    }

    fn bindings() -> WorkspaceBindings {
        WorkspaceBindings {
            path_topic: "/planning/path".into(),
            odometry_topic: "/odom".into(),
            point_cloud_topic: "/scan".into(),
            dynamic_tf_topic: "/tf".into(),
            static_tf_topic: "/tf_static".into(),
        }
    }

    fn standard_requirements() -> PlaybackRequirements {
        let mut requirements = PlaybackRequirements::empty();
        requirements.require_all_cameras();
        requirements.require_path();
        requirements.require_odometry();
        requirements.require_point_cloud();
        requirements.require_transforms();
        requirements
    }

    fn catalog() -> SourceCatalog {
        SourceCatalog {
            time_range: None,
            streams: vec![
                descriptor(9, "/camera/rear/image/compressed", COMPRESSED_IMAGE_SCHEMA),
                descriptor(20, "/planning/path", PATH_SCHEMA),
                descriptor(7, "/camera/front/image/compressed", COMPRESSED_IMAGE_SCHEMA),
                descriptor(21, "/odom", ODOMETRY_SCHEMA),
                descriptor(22, "/scan", LASER_SCAN_SCHEMA),
                descriptor(23, "/tf", TF_MESSAGE_SCHEMA),
                descriptor(24, "/tf_static", TF_MESSAGE_SCHEMA),
                descriptor(30, "/unrelated", "example_msgs/msg/Other"),
            ],
        }
    }

    #[test]
    fn explicit_standard_requirements_preserve_camera_order_and_selection() {
        let plan = SessionPlan::build(
            &catalog(),
            "/camera/front/image/compressed",
            &standard_requirements(),
            &bindings(),
        )
        .unwrap();

        assert_eq!(plan.primary_camera(), Some(CameraId(0)));
        assert_eq!(
            plan.camera_topics(),
            vec![
                (CameraId(0), "/camera/front/image/compressed".into()),
                (CameraId(1), "/camera/rear/image/compressed".into()),
            ]
        );
        assert_eq!(plan.path_stream().unwrap().id, StreamId(20));
        assert_eq!(plan.odometry_stream().unwrap().id, StreamId(21));
        assert_eq!(plan.point_cloud_stream().unwrap().id, StreamId(22));
        assert_eq!(plan.dynamic_tf_stream().unwrap().id, StreamId(23));
        assert_eq!(plan.static_tf_stream().unwrap().id, StreamId(24));
    }

    #[test]
    fn only_workspace_required_streams_are_selected() {
        let mut requirements = PlaybackRequirements::empty();
        requirements.require_camera_topic("/camera/front/image/compressed");
        requirements.require_path();
        let plan = SessionPlan::build(
            &catalog(),
            "/camera/front/image/compressed",
            &requirements,
            &bindings(),
        )
        .unwrap();

        assert_eq!(plan.selected_stream_ids(), vec![StreamId(7), StreamId(20)]);
        assert!(plan.odometry_stream().is_none());
        assert!(plan.point_cloud_stream().is_none());
        assert!(plan.dynamic_tf_stream().is_none());
    }

    #[test]
    fn duplicate_topic_selects_only_the_expected_schema() {
        let mut catalog = catalog();
        catalog
            .streams
            .insert(0, descriptor(2, "/odom", "example_msgs/msg/Foo"));
        let plan = SessionPlan::build(
            &catalog,
            "/camera/front/image/compressed",
            &standard_requirements(),
            &bindings(),
        )
        .unwrap();
        assert_eq!(plan.odometry_stream().unwrap().id, StreamId(21));
    }

    #[test]
    fn priority_camera_does_not_expand_physical_selection() {
        let mut source = catalog();
        source.streams.push(descriptor(
            31,
            "/camera/left/image/compressed",
            COMPRESSED_IMAGE_SCHEMA,
        ));
        let mut requirements = PlaybackRequirements::empty();
        requirements.require_camera_topic("/camera/left/image/compressed");
        let plan = SessionPlan::build(
            &source,
            "/camera/front/image/compressed",
            &requirements,
            &bindings(),
        )
        .unwrap();
        assert_eq!(
            plan.selected_stream_ids(),
            vec![StreamId(31)],
            "scheduler priority must not add an unrequested Camera stream"
        );
        assert_eq!(plan.primary_camera(), None);
    }

    #[test]
    fn a_required_priority_camera_must_be_cdr_compressed_image() {
        let mut source = catalog();
        source.streams.push(descriptor(
            31,
            "/camera/wrong",
            "example_msgs/msg/CameraLike",
        ));
        let mut requirements = PlaybackRequirements::empty();
        requirements.require_camera_topic("/camera/wrong");
        assert!(SessionPlan::build(&source, "/camera/wrong", &requirements, &bindings(),).is_err());
    }
}
