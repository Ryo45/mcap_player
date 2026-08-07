use crate::{ArrivalTime, RawMessage, StreamDescriptor, StreamId};
use bytes::Bytes;
use mcap::{MessageStream, Summary};
use std::fmt;

const LINEAR_FALLBACK_LIMIT: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, Default)]
pub struct StreamCatalog {
    pub streams: Vec<StreamDescriptor>,
}

impl StreamCatalog {
    pub fn by_topic(&self, topic: &str) -> Option<&StreamDescriptor> {
        self.streams.iter().find(|stream| stream.topic == topic)
    }
}

#[derive(Debug)]
pub enum McapOpenError {
    Mcap(mcap::McapError),
    SummaryRequired(usize),
    TimestampOverflow,
}

impl fmt::Display for McapOpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mcap(error) => write!(f, "MCAP error: {error}"),
            Self::SummaryRequired(size) => write!(
                f,
                "MCAP file without a summary is too large for linear fallback ({size} bytes)"
            ),
            Self::TimestampOverflow => write!(f, "MCAP timestamp exceeds signed nanosecond range"),
        }
    }
}

impl std::error::Error for McapOpenError {}
impl From<mcap::McapError> for McapOpenError {
    fn from(value: mcap::McapError) -> Self {
        Self::Mcap(value)
    }
}

#[derive(Clone)]
struct CachedMessage {
    arrival: ArrivalTime,
    stream_id: StreamId,
    payload: Bytes,
}

pub struct McapSource<B: AsRef<[u8]>> {
    backing: B,
    summary: Option<Summary>,
    catalog: StreamCatalog,
    range: (ArrivalTime, ArrivalTime),
    cache: Vec<CachedMessage>,
    chunk: Option<usize>,
    position: usize,
}

impl<B: AsRef<[u8]>> McapSource<B> {
    pub fn new(backing: B) -> Result<Self, McapOpenError> {
        let bytes = backing.as_ref();
        let summary = Summary::read(bytes)?;
        if summary.is_none() && bytes.len() > LINEAR_FALLBACK_LIMIT {
            return Err(McapOpenError::SummaryRequired(bytes.len()));
        }
        let mut catalog = StreamCatalog::default();
        if let Some(value) = &summary {
            for channel in value.channels.values() {
                catalog.streams.push(StreamDescriptor {
                    id: StreamId(u32::from(channel.id)),
                    topic: channel.topic.clone(),
                    schema: channel
                        .schema
                        .as_ref()
                        .map(|schema| schema.name.clone())
                        .unwrap_or_default(),
                    message_encoding: channel.message_encoding.clone(),
                });
            }
            catalog.streams.sort_by_key(|stream| stream.id.0);
        }
        let range = if let Some(value) = &summary {
            let start = value
                .chunk_indexes
                .iter()
                .map(|chunk| chunk.message_start_time)
                .min()
                .unwrap_or(0);
            let end = value
                .chunk_indexes
                .iter()
                .map(|chunk| chunk.message_end_time)
                .max()
                .unwrap_or(start);
            (to_arrival(start)?, to_arrival(end)?)
        } else {
            (ArrivalTime(0), ArrivalTime(0))
        };
        let mut source = Self {
            backing,
            summary,
            catalog,
            range,
            cache: vec![],
            chunk: None,
            position: 0,
        };
        if !source.has_chunks() {
            source.load_linear()?;
            if let (Some(first), Some(last)) = (source.cache.first(), source.cache.last()) {
                source.range = (first.arrival, last.arrival);
            }
        }
        Ok(source)
    }

    pub fn catalog(&self) -> &StreamCatalog {
        &self.catalog
    }
    pub fn time_range(&self) -> (ArrivalTime, ArrivalTime) {
        self.range
    }

    pub(crate) fn backing_bytes(&self) -> &[u8] {
        self.backing.as_ref()
    }

    pub fn seek(&mut self, cursor: ArrivalTime) -> Result<(), McapOpenError> {
        self.cache.clear();
        self.position = 0;
        if self.has_chunks() {
            let summary = self.summary.as_ref().expect("chunk summary checked");
            let partition = summary
                .chunk_indexes
                .partition_point(|index| to_arrival_lossy(index.message_end_time) < cursor);
            let selected = partition.min(summary.chunk_indexes.len() - 1);
            self.load_chunk(selected)?;
            self.position = self
                .cache
                .partition_point(|message| message.arrival < cursor);
        } else {
            self.load_linear()?;
            self.position = self
                .cache
                .partition_point(|message| message.arrival < cursor);
        }
        Ok(())
    }

    pub fn read_until(&mut self, cursor: ArrivalTime) -> Result<Vec<RawMessage>, McapOpenError> {
        if self.has_chunks() && self.chunk.is_none() {
            self.load_chunk(0)?;
        }
        let mut output = vec![];
        loop {
            while let Some(message) = self.cache.get(self.position) {
                if message.arrival > cursor {
                    return Ok(output);
                }
                output.push(RawMessage {
                    stream_id: message.stream_id,
                    arrival_time: message.arrival,
                    payload: message.payload.clone(),
                });
                self.position += 1;
            }
            let Some(next) = self.chunk.map(|value| value + 1) else {
                return Ok(output);
            };
            let count = self
                .summary
                .as_ref()
                .map_or(0, |summary| summary.chunk_indexes.len());
            if next >= count {
                return Ok(output);
            }
            self.load_chunk(next)?;
        }
    }

    fn load_chunk(&mut self, index: usize) -> Result<(), McapOpenError> {
        let summary = self
            .summary
            .as_ref()
            .expect("chunk loading requires a summary");
        let chunk = summary
            .chunk_indexes
            .get(index)
            .ok_or(mcap::McapError::BadIndex)?;
        let mut cache = vec![];
        for message in summary.stream_chunk(self.backing.as_ref(), chunk)? {
            let message = message?;
            cache.push(CachedMessage {
                arrival: to_arrival(message.log_time)?,
                stream_id: StreamId(u32::from(message.channel.id)),
                payload: Bytes::from(message.data.into_owned()),
            });
        }
        cache.sort_by_key(|message| message.arrival);
        self.cache = cache;
        self.chunk = Some(index);
        self.position = 0;
        Ok(())
    }

    fn load_linear(&mut self) -> Result<(), McapOpenError> {
        if !self.cache.is_empty() {
            return Ok(());
        }
        let mut cache = vec![];
        for message in MessageStream::new(self.backing.as_ref())? {
            let message = message?;
            if !self
                .catalog
                .streams
                .iter()
                .any(|stream| stream.id == StreamId(u32::from(message.channel.id)))
            {
                self.catalog.streams.push(StreamDescriptor {
                    id: StreamId(u32::from(message.channel.id)),
                    topic: message.channel.topic.clone(),
                    schema: message
                        .channel
                        .schema
                        .as_ref()
                        .map(|schema| schema.name.clone())
                        .unwrap_or_default(),
                    message_encoding: message.channel.message_encoding.clone(),
                });
            }
            cache.push(CachedMessage {
                arrival: to_arrival(message.log_time)?,
                stream_id: StreamId(u32::from(message.channel.id)),
                payload: Bytes::from(message.data.into_owned()),
            });
        }
        cache.sort_by_key(|message| message.arrival);
        self.cache = cache;
        Ok(())
    }

    fn has_chunks(&self) -> bool {
        self.summary
            .as_ref()
            .is_some_and(|summary| !summary.chunk_indexes.is_empty())
    }
}

fn to_arrival(value: u64) -> Result<ArrivalTime, McapOpenError> {
    i64::try_from(value)
        .map(ArrivalTime)
        .map_err(|_| McapOpenError::TimestampOverflow)
}
fn to_arrival_lossy(value: u64) -> ArrivalTime {
    ArrivalTime(i64::try_from(value).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcap::{WriteOptions, Writer, records::MessageHeader};
    use std::{collections::BTreeMap, io::Cursor};

    #[test]
    fn reads_summary_bearing_mcap_without_chunks_linearly() {
        let mut bytes = Cursor::new(Vec::new());
        {
            let options = WriteOptions::new().use_chunks(false);
            let mut writer = Writer::with_options(&mut bytes, options).unwrap();
            let schema = writer
                .add_schema("example/msg/Value", "ros2msg", b"uint8 value\n")
                .unwrap();
            let channel = writer
                .add_channel(schema, "/value", "cdr", &BTreeMap::new())
                .unwrap();
            writer
                .write_to_known_channel(
                    &MessageHeader {
                        channel_id: channel,
                        sequence: 0,
                        log_time: 20,
                        publish_time: 10,
                    },
                    &[1, 2, 3],
                )
                .unwrap();
            writer.finish().unwrap();
        }

        let mut source = McapSource::new(bytes.into_inner()).unwrap();
        assert_eq!(source.time_range(), (ArrivalTime(20), ArrivalTime(20)));
        let messages = source.read_until(ArrivalTime(20)).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].payload, vec![1, 2, 3]);
    }
}
