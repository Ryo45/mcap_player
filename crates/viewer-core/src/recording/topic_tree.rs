//! Metadata-only exploration. A tree entry never selects a playback route.
use crate::{NumericField, RosMessageSchema, SourceCatalog, StreamId};

#[derive(Clone, Debug)]
pub struct TopicTreeEntry {
    pub stream_id: StreamId,
    pub topic: String,
    pub schema_name: String,
    pub message_encoding: String,
    pub message_count: Option<u64>,
    pub schema_definition: Option<std::sync::Arc<crate::SchemaDefinition>>,
    numeric_fields: std::sync::OnceLock<Result<Vec<NumericField>, SchemaExplorationError>>,
}
#[derive(Clone, Debug, Default)]
pub struct TopicTree {
    pub entries: Vec<TopicTreeEntry>,
}
impl TopicTreeEntry {
    pub fn from_stream(stream: &crate::StreamDescriptor) -> Self {
        Self {
            stream_id: stream.id,
            topic: stream.topic.clone(),
            schema_name: stream.schema.clone(),
            message_encoding: stream.message_encoding.clone(),
            message_count: stream.timing.message_count,
            schema_definition: stream.schema_definition.clone(),
            numeric_fields: std::sync::OnceLock::new(),
        }
    }

    /// Explore only the requested channel. Cache success and failure for this source entry.
    pub fn numeric_fields(&self) -> Result<&[NumericField], &SchemaExplorationError> {
        self.numeric_fields
            .get_or_init(|| {
                let definition = self
                    .schema_definition
                    .as_deref()
                    .ok_or(SchemaExplorationError::MissingDefinition)?;
                RosMessageSchema::parse(&self.schema_name, definition)
                    .and_then(|schema| schema.numeric_fields())
                    .map_err(SchemaExplorationError::Decode)
            })
            .as_deref()
    }
}
impl TopicTree {
    /// Source facts only. Numeric exploration is lazy and does not select playback routes.
    pub fn from_catalog(catalog: &SourceCatalog) -> Self {
        let mut entries = catalog
            .streams
            .iter()
            .map(TopicTreeEntry::from_stream)
            .collect::<Vec<_>>();
        entries.sort_by(|a, b| (&a.topic, a.stream_id).cmp(&(&b.topic, b.stream_id)));
        Self { entries }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SchemaExplorationError {
    MissingDefinition,
    Decode(crate::DecodeError),
}
impl std::fmt::Display for SchemaExplorationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingDefinition => write!(f, "source has no schema definition"),
            Self::Decode(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for SchemaExplorationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Decode(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SchemaDefinition, StreamDescriptor, StreamTimingSummary};
    #[test]
    fn numeric_exploration_is_lazy_and_reuses_the_successful_result() {
        let definition = std::sync::Arc::new(SchemaDefinition {
            encoding: "ros2msg".into(),
            data: bytes::Bytes::from_static(b"float64 speed\nuint32 status\n"),
        });
        let catalog = SourceCatalog {
            streams: (0..2)
                .map(|i| StreamDescriptor {
                    id: StreamId(i),
                    topic: format!("/status_{i}"),
                    schema: "demo/msg/Status".into(),
                    message_encoding: "cdr".into(),
                    schema_definition: Some(definition.clone()),
                    timing: StreamTimingSummary::default(),
                })
                .collect(),
            ..Default::default()
        };
        let tree = TopicTree::from_catalog(&catalog);
        assert!(
            tree.entries
                .iter()
                .all(|e| e.numeric_fields.get().is_none())
        );
        let fields = tree.entries[0].numeric_fields().unwrap();
        assert_eq!(fields.len(), 2);
        assert!(std::ptr::eq(
            fields,
            tree.entries[0].numeric_fields().unwrap()
        ));
        assert!(tree.entries[1].numeric_fields.get().is_none());
    }
    #[test]
    fn metadata_keeps_channel_facts_and_shared_schema_without_numeric_analysis() {
        let definition = std::sync::Arc::new(SchemaDefinition {
            encoding: "ros2msg".into(),
            data: bytes::Bytes::from_static(b"invalid schema !!!"),
        });
        let catalog = SourceCatalog {
            streams: vec![StreamDescriptor {
                id: StreamId(7),
                topic: "/custom".into(),
                schema: "custom/msg/Status".into(),
                message_encoding: "cdr".into(),
                schema_definition: Some(definition.clone()),
                timing: StreamTimingSummary {
                    message_count: Some(123),
                },
            }],
            ..Default::default()
        };
        let tree = TopicTree::from_catalog(&catalog);
        let entry = &tree.entries[0];
        assert_eq!(entry.message_count, Some(123));
        assert_eq!(entry.message_encoding, "cdr");
        assert!(std::sync::Arc::ptr_eq(
            entry.schema_definition.as_ref().unwrap(),
            &definition
        ));
        assert!(
            entry.numeric_fields.get().is_none(),
            "catalog construction must not parse ROS definitions"
        );
        let error = entry.numeric_fields().unwrap_err();
        assert!(
            std::ptr::eq(error, entry.numeric_fields().unwrap_err()),
            "errors are cached too"
        );
    }
}
