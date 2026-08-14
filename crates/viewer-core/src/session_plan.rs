use crate::{CameraId, StreamBinding, StreamCatalog, StreamDescriptor, StreamId};
use std::fmt;

pub const PATH_TOPIC: &str = "/planning/path";
pub const ODOM_TOPIC: &str = "/odom";
pub const SCAN_TOPIC: &str = "/scan";
pub const TF_TOPIC: &str = "/tf";
pub const TF_STATIC_TOPIC: &str = "/tf_static";

const OPTIONAL_ROUTES: &[(&str, DomainTarget)] = &[
    (PATH_TOPIC, DomainTarget::Path),
    (ODOM_TOPIC, DomainTarget::Telemetry),
    (SCAN_TOPIC, DomainTarget::PointCloud),
    (TF_TOPIC, DomainTarget::Transforms { is_static: false }),
    (
        TF_STATIC_TOPIC,
        DomainTarget::Transforms { is_static: true },
    ),
];

/// The shared-domain meaning assigned to a concrete source stream for one session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DomainTarget {
    Camera(CameraId),
    Telemetry,
    Path,
    PointCloud,
    Transforms { is_static: bool },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainRoute {
    pub stream: StreamDescriptor,
    pub target: DomainTarget,
}

/// Runtime Viewer policy derived once from a source catalog when a session opens.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionPlan {
    domain_routes: Vec<DomainRoute>,
    primary_camera: Option<CameraId>,
}

impl SessionPlan {
    pub fn build(
        catalog: &StreamCatalog,
        primary_camera_topic: &str,
    ) -> Result<Self, SessionPlanError> {
        let mut cameras = catalog
            .streams
            .iter()
            .filter(|stream| stream.schema == "sensor_msgs/msg/CompressedImage")
            .cloned()
            .collect::<Vec<_>>();
        let Some(primary_index) = cameras
            .iter()
            .position(|stream| stream.topic == primary_camera_topic)
        else {
            return Err(SessionPlanError(format!(
                "topic {primary_camera_topic} is not present"
            )));
        };
        cameras.swap(0, primary_index);

        let mut domain_routes = cameras
            .into_iter()
            .enumerate()
            .map(|(index, stream)| DomainRoute {
                stream,
                target: DomainTarget::Camera(CameraId(index as u16)),
            })
            .collect::<Vec<_>>();
        domain_routes.extend(OPTIONAL_ROUTES.iter().filter_map(|(topic, target)| {
            catalog.by_topic(topic).cloned().map(|stream| DomainRoute {
                stream,
                target: *target,
            })
        }));

        Ok(Self {
            primary_camera: domain_routes.iter().find_map(|route| match route.target {
                DomainTarget::Camera(camera_id) => Some(camera_id),
                _ => None,
            }),
            domain_routes,
        })
    }

    pub fn domain_routes(&self) -> &[DomainRoute] {
        &self.domain_routes
    }

    pub fn primary_camera(&self) -> Option<CameraId> {
        self.primary_camera
    }

    pub fn camera_topics(&self) -> Vec<(CameraId, String)> {
        self.domain_routes
            .iter()
            .filter_map(|route| match route.target {
                DomainTarget::Camera(camera_id) => Some((camera_id, route.stream.topic.clone())),
                _ => None,
            })
            .collect()
    }

    /// Temporary adapter for the existing domain pipeline constructor.
    pub(crate) fn stream_bindings(&self) -> Vec<(StreamId, StreamBinding)> {
        self.domain_routes
            .iter()
            .map(|route| {
                let binding = match route.target {
                    DomainTarget::Camera(camera_id) => StreamBinding::Camera(camera_id),
                    DomainTarget::Telemetry => StreamBinding::Odometry,
                    DomainTarget::Path => StreamBinding::Path,
                    DomainTarget::PointCloud => StreamBinding::LaserScan,
                    DomainTarget::Transforms { is_static } => {
                        StreamBinding::Transforms { is_static }
                    }
                };
                (route.stream.id, binding)
            })
            .collect()
    }
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

    fn descriptor(id: u32, topic: &str, schema: &str) -> StreamDescriptor {
        StreamDescriptor {
            id: StreamId(id),
            topic: topic.into(),
            schema: schema.into(),
            message_encoding: "cdr".into(),
        }
    }

    #[test]
    fn builds_current_camera_and_shared_domain_policy_once() {
        let catalog = StreamCatalog {
            streams: vec![
                descriptor(
                    9,
                    "/camera/rear/image/compressed",
                    "sensor_msgs/msg/CompressedImage",
                ),
                descriptor(20, PATH_TOPIC, "nav_msgs/msg/Path"),
                descriptor(
                    7,
                    "/camera/front/image/compressed",
                    "sensor_msgs/msg/CompressedImage",
                ),
                descriptor(21, ODOM_TOPIC, "nav_msgs/msg/Odometry"),
                descriptor(22, SCAN_TOPIC, "sensor_msgs/msg/LaserScan"),
                descriptor(23, TF_TOPIC, "tf2_msgs/msg/TFMessage"),
                descriptor(24, TF_STATIC_TOPIC, "tf2_msgs/msg/TFMessage"),
                descriptor(30, "/unrelated", "example_msgs/msg/Other"),
            ],
        };

        let plan = SessionPlan::build(&catalog, "/camera/front/image/compressed").unwrap();

        assert_eq!(plan.primary_camera(), Some(CameraId(0)));
        assert_eq!(
            plan.camera_topics(),
            vec![
                (CameraId(0), "/camera/front/image/compressed".into()),
                (CameraId(1), "/camera/rear/image/compressed".into()),
            ]
        );
        assert_eq!(
            plan.domain_routes()
                .iter()
                .map(|route| (route.stream.id, route.target))
                .collect::<Vec<_>>(),
            vec![
                (StreamId(7), DomainTarget::Camera(CameraId(0))),
                (StreamId(9), DomainTarget::Camera(CameraId(1))),
                (StreamId(20), DomainTarget::Path),
                (StreamId(21), DomainTarget::Telemetry),
                (StreamId(22), DomainTarget::PointCloud),
                (StreamId(23), DomainTarget::Transforms { is_static: false },),
                (StreamId(24), DomainTarget::Transforms { is_static: true },),
            ]
        );
    }
}
