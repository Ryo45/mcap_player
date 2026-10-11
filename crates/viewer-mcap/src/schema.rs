use viewer_core::SchemaDefinition;

/// Preserve the original MCAP schema bytes in the source-neutral catalog.
pub fn schema_definition_from_mcap(schema: &mcap::Schema<'_>) -> std::sync::Arc<SchemaDefinition> {
    std::sync::Arc::new(SchemaDefinition {
        encoding: schema.encoding.clone(),
        data: bytes::Bytes::copy_from_slice(&schema.data),
    })
}
