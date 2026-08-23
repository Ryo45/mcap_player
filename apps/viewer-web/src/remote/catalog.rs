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

    let streams = supported
        .into_iter()
        .map(to_core_descriptor)
        .collect::<Vec<_>>();
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

fn to_core_descriptor(stream: &StreamDescriptor) -> CoreStreamDescriptor {
    CoreStreamDescriptor {
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
            id,
            topic: topic.into(),
            schema_name: schema.into(),
            schema_encoding: "ros2msg".into(),
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
        )
    }

    #[test]
    fn adapts_supported_streams_and_selects_camera_and_odometry() {
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
        assert_eq!(adapted.core.streams.len(), 4);
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
