use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
};
use viewer_core::{
    PlaybackRequirements, SessionPlan, SourceCapabilities, SourceCatalog, StreamDescriptor,
    StreamId, StreamTimingSummary, WorkspaceBindings,
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum LiveStreamRole {
    Camera,
    RawCamera,
    Path,
    Odometry,
    LaserScan,
    DynamicTf,
    StaticTf,
}

impl LiveStreamRole {
    pub(crate) fn schema(self) -> &'static str {
        match self {
            Self::Camera => "sensor_msgs/msg/CompressedImage",
            Self::RawCamera => "sensor_msgs/msg/Image",
            Self::Path => "nav_msgs/msg/Path",
            Self::Odometry => "nav_msgs/msg/Odometry",
            Self::LaserScan => "sensor_msgs/msg/LaserScan",
            Self::DynamicTf | Self::StaticTf => "tf2_msgs/msg/TFMessage",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LiveDeliveryClass {
    Latest,
    Ordered,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LiveStreamSpec {
    pub(crate) stream: StreamDescriptor,
    pub(crate) role: LiveStreamRole,
    pub(crate) delivery: LiveDeliveryClass,
}

impl LiveStreamSpec {
    pub(crate) fn new(stream: &StreamDescriptor, role: LiveStreamRole) -> Self {
        let delivery = match role {
            LiveStreamRole::Camera | LiveStreamRole::RawCamera => LiveDeliveryClass::Latest,
            LiveStreamRole::Path
            | LiveStreamRole::Odometry
            | LiveStreamRole::LaserScan
            | LiveStreamRole::DynamicTf
            | LiveStreamRole::StaticTf => LiveDeliveryClass::Ordered,
        };
        Self {
            stream: stream.clone(),
            role,
            delivery,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LiveCatalogError(String);

impl LiveCatalogError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for LiveCatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for LiveCatalogError {}

/// Builds the source facts that the concrete ROS adapter will subscribe to.
///
/// The primary Camera and explicitly declared Raw cameras supply topics for an `all cameras` panel. Fixed
/// Camera panels contribute their explicit topics. Feature inputs come from the closed workspace
/// bindings and have concrete schemas derived from their roles.
pub(crate) fn live_catalog(
    primary_camera_topic: &str,
    requirements: &PlaybackRequirements,
    bindings: &WorkspaceBindings,
    raw_camera_topics: &BTreeSet<String>,
) -> Result<SourceCatalog, LiveCatalogError> {
    let mut topics = BTreeMap::<String, LiveStreamRole>::new();
    let camera_role = |topic: &str| {
        if raw_camera_topics.contains(topic) {
            LiveStreamRole::RawCamera
        } else {
            LiveStreamRole::Camera
        }
    };
    insert_topic(
        &mut topics,
        primary_camera_topic,
        camera_role(primary_camera_topic),
    )?;
    for topic in raw_camera_topics {
        insert_topic(&mut topics, topic, LiveStreamRole::RawCamera)?;
    }
    for topic in requirements.explicit_camera_topics() {
        insert_topic(&mut topics, topic, camera_role(topic))?;
    }
    for (topic, role) in [
        (&bindings.path_topic, LiveStreamRole::Path),
        (&bindings.odometry_topic, LiveStreamRole::Odometry),
        (&bindings.point_cloud_topic, LiveStreamRole::LaserScan),
        (&bindings.dynamic_tf_topic, LiveStreamRole::DynamicTf),
        (&bindings.static_tf_topic, LiveStreamRole::StaticTf),
    ] {
        insert_topic(&mut topics, topic, role)?;
    }

    let streams = topics
        .into_iter()
        .enumerate()
        .map(|(index, (topic, role))| StreamDescriptor {
            schema_definition: None,
            id: StreamId(
                u32::try_from(index + 1).expect("closed Live input count fits a StreamId"),
            ),
            topic,
            schema: role.schema().into(),
            message_encoding: "cdr".into(),
            timing: StreamTimingSummary::default(),
        })
        .collect();
    Ok(SourceCatalog {
        time_range: None,
        streams,
        capabilities: SourceCapabilities::LIVE,
    })
}

fn insert_topic(
    topics: &mut BTreeMap<String, LiveStreamRole>,
    topic: &str,
    role: LiveStreamRole,
) -> Result<(), LiveCatalogError> {
    if topic.trim().is_empty() {
        return Err(LiveCatalogError::new(format!(
            "{:?} Live input has an empty topic",
            role
        )));
    }
    match topics.get(topic) {
        Some(existing) if *existing != role => Err(LiveCatalogError::new(format!(
            "Live topic {topic} is bound to both {existing:?} and {role:?}"
        ))),
        Some(_) => Ok(()),
        None => {
            topics.insert(topic.to_owned(), role);
            Ok(())
        }
    }
}

/// Resolves delivery and QoS roles only for streams selected by the SessionPlan.
pub(crate) fn selected_stream_specs(plan: &SessionPlan) -> Vec<LiveStreamSpec> {
    let mut specs = plan
        .camera_routes()
        .iter()
        .map(|route| {
            LiveStreamSpec::new(
                &route.stream,
                if route.stream.schema == "sensor_msgs/msg/Image" {
                    LiveStreamRole::RawCamera
                } else {
                    LiveStreamRole::Camera
                },
            )
        })
        .chain(
            plan.path_stream()
                .map(|stream| LiveStreamSpec::new(stream, LiveStreamRole::Path)),
        )
        .chain(
            plan.odometry_stream()
                .map(|stream| LiveStreamSpec::new(stream, LiveStreamRole::Odometry)),
        )
        .chain(
            plan.point_cloud_stream()
                .map(|stream| LiveStreamSpec::new(stream, LiveStreamRole::LaserScan)),
        )
        .chain(
            plan.dynamic_tf_stream()
                .map(|stream| LiveStreamSpec::new(stream, LiveStreamRole::DynamicTf)),
        )
        .chain(
            plan.static_tf_stream()
                .map(|stream| LiveStreamSpec::new(stream, LiveStreamRole::StaticTf)),
        )
        .collect::<Vec<_>>();
    specs.sort_by_key(|spec| spec.stream.id);
    debug_assert!(
        specs
            .windows(2)
            .all(|pair| pair[0].stream.id != pair[1].stream.id)
    );
    specs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bindings() -> WorkspaceBindings {
        WorkspaceBindings {
            path_topic: "/planning/path".into(),
            odometry_topic: "/odom".into(),
            point_cloud_topic: "/scan".into(),
            dynamic_tf_topic: "/tf".into(),
            static_tf_topic: "/tf_static".into(),
        }
    }

    #[test]
    fn catalog_is_deterministic_and_standard_plan_selects_every_live_role() {
        let mut requirements = PlaybackRequirements::empty();
        requirements.require_all_cameras();
        requirements.require_path();
        requirements.require_odometry();
        requirements.require_point_cloud();
        requirements.require_transforms();
        let catalog = live_catalog(
            "/camera/front/image/compressed",
            &requirements,
            &bindings(),
            &Default::default(),
        )
        .unwrap();
        let second = live_catalog(
            "/camera/front/image/compressed",
            &requirements,
            &bindings(),
            &Default::default(),
        )
        .unwrap();
        assert_eq!(catalog.streams, second.streams);

        let plan = SessionPlan::build(
            &catalog,
            "/camera/front/image/compressed",
            &requirements,
            &bindings(),
        )
        .unwrap();
        assert_eq!(plan.selected_stream_ids().len(), 6);
        assert_eq!(
            selected_stream_specs(&plan)
                .iter()
                .map(|spec| (spec.role, spec.delivery))
                .collect::<Vec<_>>(),
            vec![
                (LiveStreamRole::Camera, LiveDeliveryClass::Latest),
                (LiveStreamRole::Odometry, LiveDeliveryClass::Ordered),
                (LiveStreamRole::Path, LiveDeliveryClass::Ordered),
                (LiveStreamRole::LaserScan, LiveDeliveryClass::Ordered),
                (LiveStreamRole::DynamicTf, LiveDeliveryClass::Ordered),
                (LiveStreamRole::StaticTf, LiveDeliveryClass::Ordered),
            ]
        );
    }

    #[test]
    fn fixed_camera_topics_expand_live_selection_without_duplicates() {
        let mut requirements = PlaybackRequirements::empty();
        requirements.require_camera_topic("/camera/front_left/image/compressed");
        requirements.require_camera_topic("/camera/front/image/compressed");
        requirements.require_camera_topic("/camera/front_right/image/compressed");
        let catalog = live_catalog(
            "/camera/front/image/compressed",
            &requirements,
            &bindings(),
            &Default::default(),
        )
        .unwrap();
        let plan = SessionPlan::build(
            &catalog,
            "/camera/front/image/compressed",
            &requirements,
            &bindings(),
        )
        .unwrap();
        assert_eq!(plan.camera_routes().len(), 3);
    }

    #[test]
    fn raw_and_compressed_cameras_share_latest_delivery_and_fixed_panel_selection() {
        let raw = BTreeSet::from(["/raw".to_owned(), "/unused_raw".to_owned()]);
        let mut requirements = PlaybackRequirements::empty();
        requirements.require_camera_topic("/raw");
        requirements.require_camera_topic("/compressed");
        let catalog = live_catalog("/raw", &requirements, &bindings(), &raw).unwrap();
        let plan = SessionPlan::build(&catalog, "/raw", &requirements, &bindings()).unwrap();
        let specs = selected_stream_specs(&plan);
        assert_eq!(specs.len(), 2);
        assert!(
            specs
                .iter()
                .all(|s| s.delivery == LiveDeliveryClass::Latest)
        );
        assert_eq!(
            specs
                .iter()
                .find(|s| s.stream.topic == "/raw")
                .unwrap()
                .role,
            LiveStreamRole::RawCamera
        );
        assert_eq!(
            specs
                .iter()
                .find(|s| s.stream.topic == "/compressed")
                .unwrap()
                .role,
            LiveStreamRole::Camera
        );
        assert!(!specs.iter().any(|s| s.stream.topic == "/unused_raw"));
        requirements.require_all_cameras();
        assert_eq!(
            SessionPlan::build(&catalog, "/raw", &requirements, &bindings())
                .unwrap()
                .camera_routes()
                .len(),
            3
        );
        assert!(
            live_catalog(
                "/raw",
                &requirements,
                &bindings(),
                &BTreeSet::from(["/odom".into()])
            )
            .is_err()
        );
    }

    #[test]
    fn one_topic_cannot_claim_two_live_roles() {
        let mut duplicate = bindings();
        duplicate.odometry_topic = duplicate.path_topic.clone();
        assert!(
            live_catalog(
                "/camera/front/image/compressed",
                &PlaybackRequirements::empty(),
                &duplicate,
                &Default::default()
            )
            .unwrap_err()
            .to_string()
            .contains("both")
        );
    }
}
