use std::{error::Error, fmt};
use viewer_core::{
    ArrivalTime, RecordingTimeRange, SessionPlan, SourceCapabilities, SourceCatalog,
    StreamDescriptor, StreamId, StreamTimingSummary,
};

#[derive(Clone, Debug)]
pub(crate) struct LocalCatalog {
    pub core: SourceCatalog,
    pub plan: SessionPlan,
    pub selected_topics: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum LocalCatalogError {
    Plan(viewer_core::SessionPlanError),
    Index(viewer_mcap::IndexedPlanError),
    MissingChunkIndex,
    MissingTimeRange,
    EndTimeOverflow,
    EmptyTimeRange,
    TimestampOverflow(u64),
}
impl fmt::Display for LocalCatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingChunkIndex => {
                write!(f, "{}", viewer_mcap::McapSourceError::ChunkIndexRequired)
            }
            Self::MissingTimeRange => write!(f, "MCAP contains no indexed time range"),
            Self::EndTimeOverflow => write!(f, "MCAP end time cannot be made exclusive"),
            Self::EmptyTimeRange => write!(f, "MCAP indexed time range is empty"),
            Self::TimestampOverflow(value) => {
                write!(f, "MCAP timestamp {value} exceeds signed nanoseconds")
            }
            Self::Plan(error) => error.fmt(f),
            Self::Index(error) => error.fmt(f),
        }
    }
}
impl Error for LocalCatalogError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Plan(error) => Some(error),
            Self::Index(error) => Some(error),
            _ => None,
        }
    }
}
impl From<viewer_core::SessionPlanError> for LocalCatalogError {
    fn from(error: viewer_core::SessionPlanError) -> Self {
        Self::Plan(error)
    }
}
impl From<viewer_mcap::IndexedPlanError> for LocalCatalogError {
    fn from(error: viewer_mcap::IndexedPlanError) -> Self {
        Self::Index(error)
    }
}

impl LocalCatalog {
    pub(crate) fn from_summary(
        summary: &mcap::Summary,
        primary_camera_topic: &str,
    ) -> Result<Self, LocalCatalogError> {
        if summary.chunk_indexes.is_empty() {
            return Err(LocalCatalogError::MissingChunkIndex);
        }
        let start = summary
            .chunk_indexes
            .iter()
            .map(|chunk| chunk.message_start_time)
            .min()
            .ok_or(LocalCatalogError::MissingTimeRange)?;
        let end = summary
            .chunk_indexes
            .iter()
            .map(|chunk| chunk.message_end_time)
            .max()
            .ok_or(LocalCatalogError::MissingTimeRange)?;
        let end_exclusive = end
            .checked_add(1)
            .ok_or(LocalCatalogError::EndTimeOverflow)?;

        let start = to_arrival(start)?;
        let end_exclusive = to_arrival(end_exclusive)?;
        if start >= end_exclusive {
            return Err(LocalCatalogError::EmptyTimeRange);
        }

        let schema_definitions = summary
            .schemas
            .iter()
            .map(|(id, schema)| (*id, viewer_mcap::schema_definition_from_mcap(schema)))
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut streams = summary
            .channels
            .values()
            .map(|channel| StreamDescriptor {
                schema_definition: channel
                    .schema
                    .as_ref()
                    .and_then(|schema| schema_definitions.get(&schema.id).cloned()),
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
            capabilities: SourceCapabilities::INDEXED_RECORDING,
        };
        let plan = crate::playback::web_session_plan(&core, primary_camera_topic)
            .map_err(LocalCatalogError::from)?;
        plan.validate_recording_capabilities(core.capabilities)
            .map_err(LocalCatalogError::from)?;
        let index_facts = summary
            .chunk_indexes
            .iter()
            .map(|chunk| viewer_mcap::IndexedChunkFact {
                start: ArrivalTime(i64::try_from(chunk.message_start_time).unwrap_or(i64::MAX)),
                end_inclusive: ArrivalTime(
                    i64::try_from(chunk.message_end_time).unwrap_or(i64::MAX),
                ),
                indexed_streams: chunk
                    .message_index_offsets
                    .keys()
                    .map(|channel| StreamId(u32::from(*channel)))
                    .collect(),
            })
            .collect::<Vec<_>>();
        for stream in plan.selected_stream_ids() {
            let descriptor = core
                .by_id(stream)
                .expect("SessionPlan stream is in catalog");
            viewer_mcap::ensure_indexed(&index_facts, stream, descriptor.timing.message_count)
                .map_err(LocalCatalogError::from)?;
        }
        let selected_topics = plan.selected_topics();

        Ok(Self {
            core,
            plan,
            selected_topics,
        })
    }
}

fn to_arrival(value: u64) -> Result<ArrivalTime, LocalCatalogError> {
    i64::try_from(value)
        .map(ArrivalTime)
        .map_err(|_| LocalCatalogError::TimestampOverflow(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_catalog_keeps_all_cameras_and_standard_streams() {
        let bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/camera-jpeg/camera_7_5s.mcap"),
        )
        .unwrap();
        let summary = mcap::Summary::read(&bytes).unwrap().unwrap();
        let expected_start = summary
            .chunk_indexes
            .iter()
            .map(|chunk| chunk.message_start_time)
            .min()
            .unwrap();
        let expected_end_exclusive = summary
            .chunk_indexes
            .iter()
            .map(|chunk| chunk.message_end_time)
            .max()
            .unwrap()
            .checked_add(1)
            .unwrap();
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
        assert!(catalog.core.by_topic("/odom").is_some());
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
                start: ArrivalTime(i64::try_from(expected_start).unwrap()),
                end_exclusive: ArrivalTime(i64::try_from(expected_end_exclusive).unwrap()),
            })
        );
        assert_eq!(
            catalog.selected_topics,
            catalog.plan.selected_topics(),
            "loader selection comes only from the shared SessionPlan"
        );
    }
}

#[cfg(test)]
mod schema_parity_tests {
    use super::*;
    #[test]
    fn local_and_native_preserve_original_schema_bytes_without_playback_reads() {
        let bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/camera-jpeg/camera_7_5s.mcap"),
        )
        .unwrap();
        let summary = mcap::Summary::read(&bytes).unwrap().unwrap();
        let native = viewer_mcap::McapSource::copy_from_slice((bytes).as_ref()).unwrap();
        let local = LocalCatalog::from_summary(&summary, "/camera/front/image/compressed").unwrap();
        assert_eq!(native.catalog().streams, local.core.streams);
        for stream in &local.core.streams {
            let schema = summary.channels[&(stream.id.0 as u16)]
                .schema
                .as_ref()
                .unwrap();
            let definition = stream.schema_definition.as_ref().unwrap();
            assert_eq!(definition.encoding, schema.encoding);
            assert_eq!(definition.data.as_ref(), schema.data.as_ref());
        }
    }
}
