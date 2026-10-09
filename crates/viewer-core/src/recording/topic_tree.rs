//! Metadata-only exploration. A tree entry never selects a playback route.
use crate::{NumericField, RosMessageSchema, SourceCatalog, StreamId};

#[derive(Clone, Debug)]
pub struct TopicTreeEntry {
    pub stream_id: StreamId,
    pub topic: String,
    pub schema_name: String,
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
    pub fn from_catalog(catalog: &SourceCatalog) -> Self {
        let mut entries = catalog
            .streams
            .iter()
            .map(|stream| {
                let fields = stream
                    .schema_definition
                    .as_deref()
                    .ok_or(SchemaExplorationError::MissingDefinition)
                    .and_then(|definition| {
                        RosMessageSchema::parse(&stream.schema, definition)
                            .and_then(|schema| schema.numeric_fields())
                            .map_err(SchemaExplorationError::Decode)
                    });
                let (numeric_fields, schema_error) = match fields {
                    Ok(fields) => (fields, None),
                    Err(error) => (vec![], Some(error)),
                };
                TopicTreeEntry {
                    stream_id: stream.id,
                    topic: stream.topic.clone(),
                    schema_name: stream.schema.clone(),
                    schema_definition: stream.schema_definition.clone(),
                    numeric_fields,
                    schema_error,
                }
            })
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
