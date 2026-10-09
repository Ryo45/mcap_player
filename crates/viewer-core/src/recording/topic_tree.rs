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
    pub schema_error: Option<String>,
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
                    .ok_or_else(|| "source has no schema definition".to_string())
                    .and_then(|definition| {
                        RosMessageSchema::parse(&stream.schema, definition)
                            .and_then(|schema| schema.numeric_fields())
                            .map_err(|e| e.to_string())
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
