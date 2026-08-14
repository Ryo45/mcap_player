/// Identifies one stream within a source catalog and the Viewer session built from it.
///
/// This is a source-local runtime token, not a persistent or global identity. Equal numeric
/// values from different recordings or Local/Remote sources need not describe the same stream.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct StreamId(pub u32);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamDescriptor {
    pub id: StreamId,
    pub topic: String,
    pub schema: String,
    pub message_encoding: String,
}

#[derive(Clone, Debug, Default)]
pub struct StreamCatalog {
    pub streams: Vec<StreamDescriptor>,
}

impl StreamCatalog {
    pub fn by_topic(&self, topic: &str) -> Option<&StreamDescriptor> {
        self.streams.iter().find(|stream| stream.topic == topic)
    }
}
