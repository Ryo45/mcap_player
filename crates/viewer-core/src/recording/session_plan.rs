//! Fixed workspace requirements resolved against recording facts.

use crate::{
    CameraController, CameraId, OdometryController, PathController, PointCloudController,
    RestoreInput, RestoreSemantics, SourceCatalog, StreamDescriptor, StreamId, TransformController,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fmt};

const RAW_IMAGE_SCHEMA: &str = "sensor_msgs/msg/Image";
const COMPRESSED_IMAGE_SCHEMA: &str = "sensor_msgs/msg/CompressedImage";
const DETECTION_SCHEMA: &str = "vision_msgs/msg/Detection3DArray";
const PATH_SCHEMA: &str = "nav_msgs/msg/Path";
const ODOMETRY_SCHEMA: &str = "nav_msgs/msg/Odometry";
const LASER_SCAN_SCHEMA: &str = "sensor_msgs/msg/LaserScan";
const POINT_CLOUD2_SCHEMA: &str = "sensor_msgs/msg/PointCloud2";
const TF_MESSAGE_SCHEMA: &str = "tf2_msgs/msg/TFMessage";

/// Image streams supported by the camera CDR decoders, independent of topic naming.
pub fn camera_streams(catalog: &SourceCatalog) -> impl Iterator<Item = &StreamDescriptor> {
    catalog.streams.iter().filter(|stream| {
        accepts(stream, COMPRESSED_IMAGE_SCHEMA) || accepts(stream, RAW_IMAGE_SCHEMA)
    })
}

/// Continuous inputs required by the currently open workspace.
///
/// This is deliberately a small, closed set rather than a generic capability registry. Plot,
/// Inspector, and Preview requests use their existing independent data paths.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlaybackRequirements {
    all_cameras: bool,
    camera_topics: BTreeSet<String>,
    path: RequirementLevel,
    path_topics: BTreeSet<String>,
    detection_topics: BTreeSet<String>,
    odometry: RequirementLevel,
    point_cloud: RequirementLevel,
    dynamic_transforms: RequirementLevel,
    static_transforms: RequirementLevel,
    transforms_required: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
enum RequirementLevel {
    #[default]
    Disabled,
    Optional,
    Required,
}

impl PlaybackRequirements {
    pub fn empty() -> Self {
        Self {
            all_cameras: false,
            camera_topics: BTreeSet::new(),
            path: RequirementLevel::Disabled,
            path_topics: BTreeSet::new(),
            detection_topics: BTreeSet::new(),
            odometry: RequirementLevel::Disabled,
            point_cloud: RequirementLevel::Disabled,
            dynamic_transforms: RequirementLevel::Disabled,
            static_transforms: RequirementLevel::Disabled,
            transforms_required: false,
        }
    }

    pub fn require_all_cameras(&mut self) {
        self.all_cameras = true;
    }

    pub fn require_camera_topic(&mut self, topic: impl Into<String>) {
        self.camera_topics.insert(topic.into());
    }

    /// Camera topics named explicitly by fixed workspace panels.
    ///
    /// Source adapters may use these facts to build a closed catalog. `require_all_cameras` is
    /// intentionally not exposed as a topic list because discovering source cameras remains an
    /// adapter responsibility.
    pub fn explicit_camera_topics(&self) -> impl Iterator<Item = &str> {
        self.camera_topics.iter().map(String::as_str)
    }

    pub fn require_path(&mut self) {
        self.path = RequirementLevel::Required;
    }

    /// Additional fixed Path consumers, independent of the workspace's displayed Path.
    /// Multiple consumers of one topic share a route and materialization.
    pub fn require_path_topic(&mut self, topic: impl Into<String>) {
        self.path_topics.insert(topic.into());
    }

    pub fn require_detection_topic(&mut self, topic: impl Into<String>) {
        self.detection_topics.insert(topic.into());
    }

    pub fn require_odometry(&mut self) {
        self.odometry = RequirementLevel::Required;
    }

    pub fn require_point_cloud(&mut self) {
        self.point_cloud = RequirementLevel::Required;
    }

    pub fn require_transforms(&mut self) {
        self.dynamic_transforms = self.dynamic_transforms.max(RequirementLevel::Optional);
        self.static_transforms = self.static_transforms.max(RequirementLevel::Optional);
        self.transforms_required = true;
    }

    pub fn optional_path(&mut self) {
        self.path = self.path.max(RequirementLevel::Optional);
    }

    pub fn optional_odometry(&mut self) {
        self.odometry = self.odometry.max(RequirementLevel::Optional);
    }

    pub fn optional_point_cloud(&mut self) {
        self.point_cloud = self.point_cloud.max(RequirementLevel::Optional);
    }

    pub fn optional_transforms(&mut self) {
        self.dynamic_transforms = self.dynamic_transforms.max(RequirementLevel::Optional);
        self.static_transforms = self.static_transforms.max(RequirementLevel::Optional);
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
    additional_path_streams: Vec<StreamDescriptor>,
    detection_streams: Vec<StreamDescriptor>,
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
        let available_cameras = camera_streams(catalog).cloned().collect::<Vec<_>>();

        let mut selected_cameras = available_cameras
            .into_iter()
            .filter(|stream| {
                requirements.all_cameras || requirements.camera_topics.contains(&stream.topic)
            })
            .collect::<Vec<_>>();
        for topic in &requirements.camera_topics {
            if !selected_cameras.iter().any(|stream| &stream.topic == topic) {
                return Err(SessionPlanError::MissingRequiredStream {
                    feature: "Camera",
                    topic: topic.clone(),
                    expected_schemas: &[COMPRESSED_IMAGE_SCHEMA, RAW_IMAGE_SCHEMA],
                });
            }
        }
        let priority_is_required =
            requirements.all_cameras || requirements.camera_topics.contains(priority_camera_topic);
        if priority_is_required
            && !selected_cameras
                .iter()
                .any(|stream| stream.topic == priority_camera_topic)
        {
            return Err(SessionPlanError::MissingPriorityCamera {
                topic: priority_camera_topic.into(),
            });
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
                let camera_id = u16::try_from(index)
                    .map(CameraId)
                    .map_err(|_| SessionPlanError::TooManyCameraStreams)?;
                Ok(CameraRoute { stream, camera_id })
            })
            .collect::<Result<Vec<_>, SessionPlanError>>()?;

        let dynamic_tf_stream = resolve_requirement(
            catalog,
            &bindings.dynamic_tf_topic,
            &[TF_MESSAGE_SCHEMA],
            "dynamic TF",
            requirements.dynamic_transforms,
        )?;
        let static_tf_stream = resolve_requirement(
            catalog,
            &bindings.static_tf_topic,
            &[TF_MESSAGE_SCHEMA],
            "static TF",
            requirements.static_transforms,
        )?;
        if requirements.transforms_required
            && dynamic_tf_stream.is_none()
            && static_tf_stream.is_none()
        {
            return Err(SessionPlanError::MissingTransforms {
                dynamic_topic: bindings.dynamic_tf_topic.clone(),
                static_topic: bindings.static_tf_topic.clone(),
            });
        }

        if requirements.path_topics.len() > 32 {
            return Err(SessionPlanError::TooManyPathStreams);
        }
        let path_stream = resolve_requirement(
            catalog,
            &bindings.path_topic,
            &[PATH_SCHEMA],
            "Path",
            requirements.path,
        )?;
        let mut additional_path_streams = Vec::new();
        for topic in &requirements.path_topics {
            let stream = resolve_requirement(
                catalog,
                topic,
                &[PATH_SCHEMA],
                "Path input",
                RequirementLevel::Required,
            )?
            .expect("required stream resolved");
            if path_stream.as_ref().is_none_or(|path| path.id != stream.id)
                && !additional_path_streams
                    .iter()
                    .any(|path: &StreamDescriptor| path.id == stream.id)
            {
                additional_path_streams.push(stream);
            }
        }
        if requirements.detection_topics.len() > 32 {
            return Err(SessionPlanError::TooManyDetectionStreams);
        }
        let detection_streams = requirements
            .detection_topics
            .iter()
            .map(|topic| {
                resolve_requirement(
                    catalog,
                    topic,
                    &[DETECTION_SCHEMA],
                    "Detection3DArray",
                    RequirementLevel::Required,
                )
                .map(|stream| stream.expect("required stream resolved"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            camera_routes: selected_cameras,
            path_stream,
            additional_path_streams,
            detection_streams,
            odometry_stream: resolve_requirement(
                catalog,
                &bindings.odometry_topic,
                &[ODOMETRY_SCHEMA],
                "Odometry",
                requirements.odometry,
            )?,
            point_cloud_stream: resolve_requirement(
                catalog,
                &bindings.point_cloud_topic,
                &[LASER_SCAN_SCHEMA, POINT_CLOUD2_SCHEMA],
                "PointCloud",
                requirements.point_cloud,
            )?,
            dynamic_tf_stream,
            static_tf_stream,
            primary_camera,
        })
    }

    /// Validates the operations required when this plan is used for recording playback.
    ///
    /// Route resolution is also used by push/live sources. Recording adapters call this method
    /// explicitly at open time so source kind is not inferred from an incidental catalog field.
    pub fn validate_recording_capabilities(
        &self,
        capabilities: crate::SourceCapabilities,
    ) -> Result<(), SessionPlanError> {
        if !capabilities.catalog || !capabilities.forward_playback || !capabilities.exact_seek {
            return Err(SessionPlanError::MissingPlaybackCapabilities {
                provided: capabilities,
            });
        }
        for input in self.restore_inputs() {
            let supported = match input.semantics {
                RestoreSemantics::LatestBefore => capabilities.exact_seek,
                RestoreSemantics::History { .. } => capabilities.history_restore,
                RestoreSemantics::Persistent => capabilities.persistent_restore,
            };
            if !supported {
                return Err(SessionPlanError::UnsupportedRestore {
                    stream: input.stream_id,
                    semantics: input.semantics,
                });
            }
        }
        Ok(())
    }

    pub fn camera_routes(&self) -> &[CameraRoute] {
        &self.camera_routes
    }

    pub fn path_stream(&self) -> Option<&StreamDescriptor> {
        self.path_stream.as_ref()
    }

    pub fn additional_path_streams(&self) -> &[StreamDescriptor] {
        &self.additional_path_streams
    }

    pub fn path_stream_for_topic(&self, topic: &str) -> Option<&StreamDescriptor> {
        self.path_stream
            .iter()
            .chain(&self.additional_path_streams)
            .find(|stream| stream.topic == topic)
    }

    pub fn detection_streams(&self) -> &[StreamDescriptor] {
        &self.detection_streams
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
            .chain(
                self.path_stream
                    .iter()
                    .chain(&self.additional_path_streams)
                    .map(|stream| RestoreInput {
                        stream_id: stream.id,
                        semantics: PathController::restore_semantics(),
                    }),
            )
            .chain(self.detection_streams.iter().map(|stream| RestoreInput {
                stream_id: stream.id,
                semantics: RestoreSemantics::LatestBefore,
            }))
            .chain(self.odometry_stream.iter().map(|stream| RestoreInput {
                stream_id: stream.id,
                semantics: OdometryController::restore_semantics(),
            }))
            .chain(self.point_cloud_stream.iter().map(|stream| RestoreInput {
                stream_id: stream.id,
                semantics: PointCloudController::restore_semantics(),
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
        .chain(&self.additional_path_streams)
        .chain(&self.detection_streams)
    }
}

fn select_stream(
    catalog: &SourceCatalog,
    topic: &str,
    expected_schemas: &[&str],
) -> Option<StreamDescriptor> {
    catalog
        .streams
        .iter()
        .find(|stream| {
            stream.topic == topic
                && expected_schemas
                    .iter()
                    .any(|schema| accepts(stream, schema))
        })
        .cloned()
}

fn resolve_requirement(
    catalog: &SourceCatalog,
    topic: &str,
    expected_schemas: &'static [&'static str],
    feature: &'static str,
    level: RequirementLevel,
) -> Result<Option<StreamDescriptor>, SessionPlanError> {
    let selected = select_stream(catalog, topic, expected_schemas);
    if level == RequirementLevel::Required && selected.is_none() {
        return Err(SessionPlanError::MissingRequiredStream {
            feature,
            topic: topic.into(),
            expected_schemas,
        });
    }
    Ok((level != RequirementLevel::Disabled)
        .then_some(selected)
        .flatten())
}

fn accepts(stream: &StreamDescriptor, expected_schema: &str) -> bool {
    stream.message_encoding == "cdr" && stream.schema == expected_schema
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionPlanError {
    MissingRequiredStream {
        feature: &'static str,
        topic: String,
        expected_schemas: &'static [&'static str],
    },
    MissingPriorityCamera {
        topic: String,
    },
    MissingTransforms {
        dynamic_topic: String,
        static_topic: String,
    },
    TooManyCameraStreams,
    TooManyPathStreams,
    TooManyDetectionStreams,
    MissingPlaybackCapabilities {
        provided: crate::SourceCapabilities,
    },
    UnsupportedRestore {
        stream: StreamId,
        semantics: RestoreSemantics,
    },
}
impl fmt::Display for SessionPlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRequiredStream {
                feature,
                topic,
                expected_schemas,
            } => write!(
                f,
                "required {feature} topic {topic} is not an expected CDR stream ({})",
                expected_schemas.join(" or ")
            ),
            Self::MissingPriorityCamera { topic } => write!(
                f,
                "priority Camera topic {topic} is not a selected CDR Image or CompressedImage stream"
            ),
            Self::MissingTransforms {
                dynamic_topic,
                static_topic,
            } => write!(
                f,
                "required Transform input is unavailable on {dynamic_topic} or {static_topic}"
            ),
            Self::TooManyDetectionStreams => {
                write!(f, "at most 32 Detection3DArray inputs are supported")
            }
            Self::TooManyPathStreams => {
                write!(f, "at most 32 additional Path inputs are supported")
            }
            Self::TooManyCameraStreams => {
                write!(f, "source contains too many selected Camera streams")
            }
            Self::MissingPlaybackCapabilities { .. } => write!(
                f,
                "recording source does not provide Catalog, ForwardPlayback and ExactSeek"
            ),
            Self::UnsupportedRestore { stream, semantics } => write!(
                f,
                "recording source cannot satisfy {semantics:?} restore for stream {}",
                stream.0
            ),
        }
    }
}
impl std::error::Error for SessionPlanError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ArrivalTime, RecordingTimeRange, StreamId, StreamTimingSummary};

    fn descriptor(id: u32, topic: &str, schema: &str) -> StreamDescriptor {
        StreamDescriptor {
            schema_definition: None,
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
            capabilities: Default::default(),
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
    fn image_discovery_uses_schema_and_encoding_instead_of_topic_names() {
        let mut catalog = catalog();
        catalog.streams = vec![
            descriptor(1, "/project/raw", RAW_IMAGE_SCHEMA),
            descriptor(2, "/unusual/compressed", COMPRESSED_IMAGE_SCHEMA),
            descriptor(3, "/camera/front/image/compressed", PATH_SCHEMA),
            descriptor(4, "/not-cdr", RAW_IMAGE_SCHEMA),
        ];
        catalog.streams[3].message_encoding = "json".into();
        assert_eq!(
            camera_streams(&catalog)
                .map(|stream| stream.topic.as_str())
                .collect::<Vec<_>>(),
            ["/project/raw", "/unusual/compressed"]
        );
    }

    #[test]
    fn optional_scene_inputs_select_available_streams_without_requiring_them() {
        let mut requirements = PlaybackRequirements::empty();
        requirements.require_camera_topic("/camera/front/image/compressed");
        requirements.optional_path();
        requirements.optional_odometry();
        requirements.optional_point_cloud();
        requirements.optional_transforms();
        let full = catalog();
        let plan = SessionPlan::build(
            &full,
            "/camera/front/image/compressed",
            &requirements,
            &bindings(),
        )
        .unwrap();
        assert_eq!(plan.selected_stream_ids().len(), 6);
        assert!(plan.point_cloud_stream().is_some());
        let mut camera_only = full;
        camera_only
            .streams
            .retain(|stream| stream.schema == COMPRESSED_IMAGE_SCHEMA);
        let plan = SessionPlan::build(
            &camera_only,
            "/camera/front/image/compressed",
            &requirements,
            &bindings(),
        )
        .unwrap();
        assert_eq!(plan.selected_stream_ids(), vec![StreamId(7)]);
        assert!(plan.point_cloud_stream().is_none());
        requirements.require_point_cloud();
        requirements.optional_point_cloud();
        assert!(matches!(
            SessionPlan::build(
                &camera_only,
                "/camera/front/image/compressed",
                &requirements,
                &bindings()
            ),
            Err(SessionPlanError::MissingRequiredStream { .. })
        ));
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
    fn required_and_optional_feature_inputs_have_distinct_missing_semantics() {
        let mut missing = catalog();
        missing
            .streams
            .retain(|stream| stream.topic != "/planning/path");

        let mut required = PlaybackRequirements::empty();
        required.require_path();
        let error = SessionPlan::build(&missing, "/unused", &required, &bindings()).unwrap_err();
        assert!(error.to_string().contains("required Path topic"));

        let mut optional = PlaybackRequirements::empty();
        optional.optional_path();
        let plan = SessionPlan::build(&missing, "/unused", &optional, &bindings()).unwrap();
        assert!(plan.path_stream().is_none());
    }

    #[test]
    fn recording_capabilities_are_validated_only_for_recording_open() {
        let mut source = catalog();
        source.time_range = RecordingTimeRange::new(ArrivalTime(0), ArrivalTime(100));
        let mut requirements = PlaybackRequirements::empty();
        requirements.require_camera_topic("/camera/front/image/compressed");

        let plan = SessionPlan::build(
            &source,
            "/camera/front/image/compressed",
            &requirements,
            &bindings(),
        )
        .unwrap();
        let error = plan
            .validate_recording_capabilities(source.capabilities)
            .unwrap_err();
        assert!(error.to_string().contains("ExactSeek"));

        source.capabilities = crate::SourceCapabilities::INDEXED_RECORDING;
        plan.validate_recording_capabilities(source.capabilities)
            .unwrap();
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
