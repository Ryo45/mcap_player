use std::{error::Error, fmt};
use viewer_core::{
    ArrivalTime, RecordingTimeRange, SessionPlan, SourceCapabilities, SourceCatalog,
    StreamDescriptor as CoreStreamDescriptor, StreamId, StreamTimingSummary,
};
use viewer_remote_protocol::{CatalogResponse, StreamDescriptor};

#[derive(Clone, Debug)]
pub(crate) struct RemoteCatalog {
    pub core: SourceCatalog,
    pub recording_id: String,
    pub revision: String,
    pub plan: SessionPlan,
    pub selected_streams: Vec<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RemoteCatalogError(String);

impl fmt::Display for RemoteCatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for RemoteCatalogError {}

pub(crate) fn adapt_catalog(remote: &CatalogResponse) -> Result<RemoteCatalog, RemoteCatalogError> {
    if remote.time_basis != "mcap-log-time"
        || remote.range_semantics != "start-inclusive-end-exclusive"
    {
        return Err(RemoteCatalogError(
            "unsupported remote time or range semantics".into(),
        ));
    }
    let start = to_arrival(remote.time_range.start_ns.get())?;
    let end_exclusive = to_arrival(remote.time_range.end_ns_exclusive.get())?;
    if start >= end_exclusive {
        return Err(RemoteCatalogError("remote time range is empty".into()));
    }
    let supported = remote
        .streams
        .iter()
        .filter(|stream| stream.message_encoding == "cdr")
        .collect::<Vec<_>>();
    let camera = supported
        .iter()
        .copied()
        .find(|stream| stream.schema_name == "sensor_msgs/msg/CompressedImage")
        .ok_or_else(|| {
            RemoteCatalogError("remote catalog has no supported camera stream".into())
        })?;
    let primary_camera_topic = camera.topic.clone();

    let mut definitions = std::collections::BTreeMap::new();
    for schema in &remote.schemas {
        let definition = std::sync::Arc::new(viewer_core::SchemaDefinition {
            encoding: schema.encoding.clone(),
            data: bytes::Bytes::from_owner(schema.data.clone()),
        });
        if definitions.insert(schema.id, definition).is_some() {
            return Err(RemoteCatalogError("duplicate remote schema ID".into()));
        }
    }
    let streams = remote
        .streams
        .iter()
        .map(|stream| {
            let definition = match stream.schema_id {
                Some(id) => Some(
                    definitions
                        .get(&id)
                        .ok_or_else(|| RemoteCatalogError(format!("unknown schema ID {id}")))?
                        .clone(),
                ),
                None => None,
            };
            Ok(to_core_descriptor(stream, definition))
        })
        .collect::<Result<Vec<_>, RemoteCatalogError>>()?;
    let core = SourceCatalog {
        time_range: Some(RecordingTimeRange {
            start,
            end_exclusive,
        }),
        streams,
        capabilities: SourceCapabilities::INDEXED_RECORDING,
    };
    let plan = SessionPlan::build(
        &core,
        &primary_camera_topic,
        &crate::playback::web_playback_requirements(),
        &crate::playback::web_workspace_bindings(),
    )
    .map_err(|error| RemoteCatalogError(error.to_string()))?;
    let mut selected = plan
        .selected_stream_ids()
        .into_iter()
        .map(|stream_id| stream_id.0)
        .collect::<Vec<_>>();
    selected.sort_unstable();
    selected.dedup();
    Ok(RemoteCatalog {
        core,
        recording_id: remote.recording_id.clone(),
        revision: remote.recording_revision.clone(),
        plan,
        selected_streams: selected,
    })
}

fn to_core_descriptor(
    stream: &StreamDescriptor,
    definition: Option<std::sync::Arc<viewer_core::SchemaDefinition>>,
) -> CoreStreamDescriptor {
    CoreStreamDescriptor {
        schema_definition: definition,
        id: StreamId(stream.id),
        topic: stream.topic.clone(),
        schema: stream.schema_name.clone(),
        message_encoding: stream.message_encoding.clone(),
        timing: StreamTimingSummary {
            message_count: stream.message_count.map(|count| count.get()),
        },
    }
}

fn to_arrival(value: u64) -> Result<ArrivalTime, RemoteCatalogError> {
    i64::try_from(value)
        .map(ArrivalTime)
        .map_err(|_| RemoteCatalogError("remote timestamp exceeds signed nanoseconds".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use viewer_remote_protocol::{CatalogResponse, RemoteTimeRange, StreamDescriptor, TimestampNs};

    fn stream(id: u32, topic: &str, schema: &str, message_encoding: &str) -> StreamDescriptor {
        StreamDescriptor {
            schema_id: None,
            id,
            topic: topic.into(),
            schema_name: schema.into(),
            message_encoding: message_encoding.into(),
            message_count: Some(viewer_remote_protocol::MessageCount::new(10)),
        }
    }

    fn catalog() -> CatalogResponse {
        CatalogResponse::new(
            "demo".into(),
            "revision".into(),
            RemoteTimeRange {
                start_ns: TimestampNs::new(100),
                end_ns_exclusive: TimestampNs::new(200),
            },
            vec![
                stream(7, "/camera", "sensor_msgs/msg/CompressedImage", "cdr"),
                stream(8, "/camera/rear", "sensor_msgs/msg/CompressedImage", "cdr"),
                stream(3, "/odom", "nav_msgs/msg/Odometry", "cdr"),
                stream(4, "/extra", "example/msg/Extra", "cdr"),
                stream(9, "/future", "example/msg/Future", "future"),
            ],
            Vec::new(),
        )
    }

    #[test]
    fn retains_all_metadata_and_selects_camera_and_odometry() {
        let adapted = adapt_catalog(&catalog()).unwrap();
        assert_eq!(
            adapted.core.time_range,
            Some(RecordingTimeRange {
                start: ArrivalTime(100),
                end_exclusive: ArrivalTime(200),
            })
        );
        assert_eq!(adapted.plan.primary_camera_topic(), Some("/camera"));
        assert_eq!(adapted.selected_streams, vec![3, 7, 8]);
        assert_eq!(adapted.core.streams.len(), 5);
        assert!(adapted.core.by_topic("/future").is_some());
        assert!(adapted.core.by_topic("/extra").is_some());
    }

    #[test]
    fn rejects_missing_camera_and_timestamp_overflow() {
        let mut value = catalog();
        value
            .streams
            .retain(|stream| stream.schema_name != "sensor_msgs/msg/CompressedImage");
        assert!(adapt_catalog(&value).is_err());

        let mut value = catalog();
        value.time_range.start_ns = TimestampNs::new(i64::MAX as u64 + 1);
        assert!(adapt_catalog(&value).is_err());
    }
}

#[cfg(test)]
mod schema_parity_tests {
    use super::*;
    #[test]
    fn remote_json_preserves_all_source_schemas_including_non_cdr_topics() {
        let bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/camera-jpeg/camera_7_5s.mcap"),
        )
        .unwrap();
        let native = viewer_core::McapSource::new(bytes).unwrap();
        let range = native.catalog().time_range.unwrap();
        let mut streams = native
            .catalog()
            .streams
            .iter()
            .map(|s| StreamDescriptor {
                id: s.id.0,
                topic: s.topic.clone(),
                schema_name: s.schema.clone(),
                schema_id: Some(s.id.0 as u16),
                message_encoding: s.message_encoding.clone(),
                message_count: s
                    .timing
                    .message_count
                    .map(viewer_remote_protocol::MessageCount::new),
            })
            .collect::<Vec<_>>();
        streams.push(StreamDescriptor {
            id: 999,
            topic: "/opaque".into(),
            schema_name: "Any".into(),
            schema_id: Some(999),
            message_encoding: "custom".into(),
            message_count: None,
        });
        let remote = CatalogResponse::new(
            "test".into(),
            "rev".into(),
            viewer_remote_protocol::RemoteTimeRange {
                start_ns: viewer_remote_protocol::TimestampNs::new(range.start.0 as u64),
                end_ns_exclusive: viewer_remote_protocol::TimestampNs::new(
                    range.end_exclusive.0 as u64,
                ),
            },
            streams,
            native
                .catalog()
                .streams
                .iter()
                .map(|s| viewer_remote_protocol::SchemaDescriptor {
                    id: s.id.0 as u16,
                    encoding: s.schema_definition.as_ref().unwrap().encoding.clone(),
                    data: std::sync::Arc::from(s.schema_definition.as_ref().unwrap().data.as_ref()),
                })
                .chain(std::iter::once(viewer_remote_protocol::SchemaDescriptor {
                    id: 999,
                    encoding: "binary".into(),
                    data: std::sync::Arc::from([0, 255]),
                }))
                .collect(),
        );
        let json = serde_json::to_string(&remote).unwrap();
        let remote = adapt_catalog(&serde_json::from_str(&json).unwrap()).unwrap();
        for stream in &native.catalog().streams {
            assert_eq!(remote.core.by_id(stream.id), Some(stream));
        }
        let opaque = remote.core.by_id(StreamId(999)).unwrap();
        assert_eq!(
            opaque.schema_definition.as_ref().unwrap().data.as_ref(),
            &[0, 255]
        );
        assert!(!remote.plan.selected_stream_ids().contains(&StreamId(999)));
        let selected = remote.plan.selected_stream_ids();
        let tree = viewer_core::TopicTree::from_catalog(&remote.core);
        assert!(tree.entries.iter().any(|e| e.stream_id == StreamId(999)));
        assert_eq!(remote.plan.selected_stream_ids(), selected);
    }
}

#[cfg(test)]
mod sharing_tests {
    use super::*;
    #[test]
    fn all_streams_share_one_schema_allocation_and_invalid_references_fail() {
        let data: std::sync::Arc<[u8]> = std::sync::Arc::from(vec![42; 65536]);
        let mut response = CatalogResponse::new(
            "test".into(),
            "rev".into(),
            viewer_remote_protocol::RemoteTimeRange {
                start_ns: viewer_remote_protocol::TimestampNs::new(0),
                end_ns_exclusive: viewer_remote_protocol::TimestampNs::new(10),
            },
            (1..=256)
                .map(|id| StreamDescriptor {
                    id,
                    topic: format!("/camera/{id}"),
                    schema_name: "sensor_msgs/msg/CompressedImage".into(),
                    schema_id: Some(1),
                    message_encoding: "cdr".into(),
                    message_count: Some(viewer_remote_protocol::MessageCount::new(0)),
                })
                .collect(),
            vec![viewer_remote_protocol::SchemaDescriptor {
                id: 1,
                encoding: "ros2msg".into(),
                data,
            }],
        );
        let copy = response.clone();
        assert!(std::sync::Arc::ptr_eq(
            &copy.schemas[0].data,
            &response.schemas[0].data
        ));
        let json = serde_json::to_string(&response).unwrap();
        assert_eq!(json.matches("\"data\":").count(), 1);
        assert!(json.len() < 65536 * 5 + 256 * 256);
        let adapted = adapt_catalog(&response).unwrap();
        let first = adapted.core.streams[0].schema_definition.as_ref().unwrap();
        assert_eq!(first.data.as_ptr(), response.schemas[0].data.as_ptr());
        for stream in &adapted.core.streams {
            assert!(std::sync::Arc::ptr_eq(
                first,
                stream.schema_definition.as_ref().unwrap()
            ));
        }
        response.streams[0].schema_id = Some(999);
        assert!(adapt_catalog(&response).is_err());
        response.streams[0].schema_id = Some(1);
        response.schemas.push(response.schemas[0].clone());
        assert!(adapt_catalog(&response).is_err());
    }
}
