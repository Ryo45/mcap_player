use std::{error::Error, fmt};
use viewer_core::{
    ArrivalTime, RecordingTimeRange, SessionPlan, SourceCatalog, StreamDescriptor, StreamId,
    StreamTimingSummary,
};

#[derive(Clone, Debug)]
pub(crate) struct LocalCatalog {
    pub core: SourceCatalog,
    pub start: ArrivalTime,
    pub end: ArrivalTime,
    pub end_exclusive: ArrivalTime,
    pub plan: SessionPlan,
    pub selected_topics: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LocalCatalogError(String);

impl LocalCatalogError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for LocalCatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for LocalCatalogError {}

impl LocalCatalog {
    pub(crate) fn from_summary(
        summary: &mcap::Summary,
        primary_camera_topic: &str,
    ) -> Result<Self, LocalCatalogError> {
        if summary.chunk_indexes.is_empty() {
            return Err(LocalCatalogError::new(
                "browser lazy playback requires MCAP Chunk Index records",
            ));
        }
        let start = summary
            .chunk_indexes
            .iter()
            .map(|chunk| chunk.message_start_time)
            .min()
            .ok_or_else(|| LocalCatalogError::new("MCAP contains no indexed time range"))?;
        let end = summary
            .chunk_indexes
            .iter()
            .map(|chunk| chunk.message_end_time)
            .max()
            .ok_or_else(|| LocalCatalogError::new("MCAP contains no indexed time range"))?;
        let end_exclusive = end
            .checked_add(1)
            .ok_or_else(|| LocalCatalogError::new("MCAP end time cannot be made exclusive"))?;

        let start = to_arrival(start)?;
        let end = to_arrival(end)?;
        let end_exclusive = to_arrival(end_exclusive)?;
        if start >= end_exclusive {
            return Err(LocalCatalogError::new("MCAP indexed time range is empty"));
        }

        let mut streams = summary
            .channels
            .values()
            .map(|channel| StreamDescriptor {
                id: StreamId(u32::from(channel.id)),
                topic: channel.topic.clone(),
                schema: channel
                    .schema
                    .as_ref()
                    .map(|schema| schema.name.clone())
                    .unwrap_or_default(),
                message_encoding: channel.message_encoding.clone(),
                timing: StreamTimingSummary {
                    message_count: summary
                        .stats
                        .as_ref()
                        .and_then(|stats| stats.channel_message_counts.get(&channel.id).copied()),
                },
            })
            .collect::<Vec<_>>();
        streams.sort_by_key(|stream| stream.id.0);
        let core = SourceCatalog {
            time_range: Some(RecordingTimeRange {
                start,
                end_exclusive,
            }),
            streams,
        };
        let plan = SessionPlan::build(&core, primary_camera_topic)
            .map_err(|error| LocalCatalogError::new(error.to_string()))?;
        let selected_topics = plan.selected_topics();

        Ok(Self {
            core,
            start,
            end,
            end_exclusive,
            plan,
            selected_topics,
        })
    }
}

fn to_arrival(value: u64) -> Result<ArrivalTime, LocalCatalogError> {
    i64::try_from(value)
        .map(ArrivalTime)
        .map_err(|_| LocalCatalogError::new("MCAP timestamp exceeds signed nanoseconds"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use viewer_core::ODOM_TOPIC;

    #[test]
    fn fixture_catalog_keeps_all_cameras_and_standard_streams() {
        let bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/camera-jpeg/camera_7_5s.mcap"),
        )
        .unwrap();
        let summary = mcap::Summary::read(&bytes).unwrap().unwrap();
        let catalog =
            LocalCatalog::from_summary(&summary, "/camera/front/image/compressed").unwrap();

        assert_eq!(
            catalog
                .core
                .streams
                .iter()
                .filter(|stream| stream.schema == "sensor_msgs/msg/CompressedImage")
                .count(),
            7
        );
        assert!(catalog.core.by_topic(ODOM_TOPIC).is_some());
        assert!(
            catalog
                .core
                .streams
                .iter()
                .all(|stream| stream.timing.message_count.is_some())
        );
        assert_eq!(
            catalog.core.time_range,
            Some(RecordingTimeRange {
                start: catalog.start,
                end_exclusive: catalog.end_exclusive,
            })
        );
        assert_eq!(catalog.end_exclusive.0, catalog.end.0 + 1);
        assert_eq!(
            catalog.selected_topics,
            catalog.plan.selected_topics(),
            "loader selection comes only from the shared SessionPlan"
        );
    }
}
