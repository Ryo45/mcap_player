use crate::{
    CameraController, CameraId, OdometryController, PathController, RestoreInput, SceneController,
    SourceCatalog, StreamDescriptor, StreamId, TransformController,
};
use std::{collections::BTreeSet, fmt};

pub const PATH_TOPIC: &str = "/planning/path";
pub const ODOM_TOPIC: &str = "/odom";
pub const SCAN_TOPIC: &str = "/scan";
pub const TF_TOPIC: &str = "/tf";
pub const TF_STATIC_TOPIC: &str = "/tf_static";

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

    pub fn requires_all_cameras(&self) -> bool {
        self.all_cameras
    }

    pub fn camera_topics(&self) -> &BTreeSet<String> {
        &self.camera_topics
    }
}

impl Default for PlaybackRequirements {
    fn default() -> Self {
        let mut requirements = Self::empty();
        requirements.require_all_cameras();
        requirements.require_path();
        requirements.require_odometry();
        requirements.require_point_cloud();
        requirements.require_transforms();
        requirements
    }
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
        primary_camera_topic: &str,
        requirements: &PlaybackRequirements,
    ) -> Result<Self, SessionPlanError> {
        let mut available_cameras = catalog
            .streams
            .iter()
            .filter(|stream| accepts(stream, COMPRESSED_IMAGE_SCHEMA))
            .cloned()
            .collect::<Vec<_>>();

        let cameras_requested = requirements.all_cameras || !requirements.camera_topics.is_empty();
        let primary_camera = if cameras_requested {
            let Some(primary_index) = available_cameras
                .iter()
                .position(|stream| stream.topic == primary_camera_topic)
            else {
                return Err(SessionPlanError(format!(
                    "primary Camera topic {primary_camera_topic} is not a CDR CompressedImage stream"
                )));
            };
            available_cameras.swap(0, primary_index);
            Some(CameraId(0))
        } else {
            None
        };

        let selected_cameras = available_cameras
            .into_iter()
            .filter(|stream| {
                requirements.all_cameras
                    || stream.topic == primary_camera_topic
                    || requirements.camera_topics.contains(&stream.topic)
            })
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
                .then(|| select_stream(catalog, PATH_TOPIC, PATH_SCHEMA))
                .flatten(),
            odometry_stream: requirements
                .odometry
                .then(|| select_stream(catalog, ODOM_TOPIC, ODOMETRY_SCHEMA))
                .flatten(),
            point_cloud_stream: requirements
                .point_cloud
                .then(|| select_stream(catalog, SCAN_TOPIC, LASER_SCAN_SCHEMA))
                .flatten(),
            dynamic_tf_stream: requirements
                .dynamic_transforms
                .then(|| select_stream(catalog, TF_TOPIC, TF_MESSAGE_SCHEMA))
                .flatten(),
            static_tf_stream: requirements
                .static_transforms
                .then(|| select_stream(catalog, TF_STATIC_TOPIC, TF_MESSAGE_SCHEMA))
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

    fn catalog() -> SourceCatalog {
        SourceCatalog {
            time_range: None,
            streams: vec![
                descriptor(9, "/camera/rear/image/compressed", COMPRESSED_IMAGE_SCHEMA),
                descriptor(20, PATH_TOPIC, PATH_SCHEMA),
                descriptor(7, "/camera/front/image/compressed", COMPRESSED_IMAGE_SCHEMA),
                descriptor(21, ODOM_TOPIC, ODOMETRY_SCHEMA),
                descriptor(22, SCAN_TOPIC, LASER_SCAN_SCHEMA),
                descriptor(23, TF_TOPIC, TF_MESSAGE_SCHEMA),
                descriptor(24, TF_STATIC_TOPIC, TF_MESSAGE_SCHEMA),
                descriptor(30, "/unrelated", "example_msgs/msg/Other"),
            ],
        }
    }

    #[test]
    fn default_requirements_preserve_camera_order_and_standard_selection() {
        let plan = SessionPlan::build(
            &catalog(),
            "/camera/front/image/compressed",
            &PlaybackRequirements::default(),
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
        let plan = SessionPlan::build(&catalog(), "/camera/front/image/compressed", &requirements)
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
            .insert(0, descriptor(2, ODOM_TOPIC, "example_msgs/msg/Foo"));
        let plan = SessionPlan::build(
            &catalog,
            "/camera/front/image/compressed",
            &PlaybackRequirements::default(),
        )
        .unwrap();
        assert_eq!(plan.odometry_stream().unwrap().id, StreamId(21));
    }

    #[test]
    fn primary_camera_must_be_a_cdr_compressed_image() {
        let mut catalog = catalog();
        catalog.streams.push(descriptor(
            31,
            "/camera/wrong",
            "example_msgs/msg/CameraLike",
        ));
        assert!(
            SessionPlan::build(&catalog, "/camera/wrong", &PlaybackRequirements::default(),)
                .is_err()
        );
    }
}
