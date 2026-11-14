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
    pub numeric_fields: Vec<NumericField>,
    /// Unsupported or absent schema is visible; the topic remains inspectable.
    pub schema_error: Option<SchemaExplorationError>,
}
#[derive(Clone, Debug, Default)]
pub struct TopicTree {
    pub entries: Vec<TopicTreeEntry>,
}
impl TopicTree {
    /// Source facts only: no ROS schema traversal or numeric-field extraction.
    pub fn metadata(catalog: &SourceCatalog) -> Self {
        let mut entries = catalog
            .streams
            .iter()
            .map(|stream| TopicTreeEntry {
                stream_id: stream.id,
                topic: stream.topic.clone(),
                schema_name: stream.schema.clone(),
                message_encoding: stream.message_encoding.clone(),
                message_count: stream.timing.message_count,
                schema_definition: stream.schema_definition.clone(),
                numeric_fields: vec![],
                schema_error: None,
            })
            .collect::<Vec<_>>();
        entries.sort_by(|a, b| (&a.topic, a.stream_id).cmp(&(&b.topic, b.stream_id)));
        Self { entries }
    }

    /// Numeric exploration for hosts that expose numeric-field plots.
    pub fn from_catalog(catalog: &SourceCatalog) -> Self {
        let mut tree = Self::metadata(catalog);
        for entry in &mut tree.entries {
            let fields = entry
                .schema_definition
                .as_deref()
                .ok_or(SchemaExplorationError::MissingDefinition)
                .and_then(|definition| {
                    RosMessageSchema::parse(&entry.schema_name, definition)
                        .and_then(|schema| schema.numeric_fields())
                        .map_err(SchemaExplorationError::Decode)
                });
            match fields {
                Ok(fields) => entry.numeric_fields = fields,
                Err(error) => entry.schema_error = Some(error),
            }
        }
        tree
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
        let tree = TopicTree::metadata(&catalog);
        let entry = &tree.entries[0];
        assert_eq!(entry.message_count, Some(123));
        assert_eq!(entry.message_encoding, "cdr");
        assert!(std::sync::Arc::ptr_eq(
            entry.schema_definition.as_ref().unwrap(),
            &definition
        ));
        assert!(entry.numeric_fields.is_empty());
        assert!(
            entry.schema_error.is_none(),
            "metadata construction must not parse invalid ROS definitions"
        );
        assert!(
            TopicTree::from_catalog(&catalog).entries[0]
                .schema_error
                .is_some()
        );
    }
}
