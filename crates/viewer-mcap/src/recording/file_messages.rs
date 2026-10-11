//! Bounded sequential reads for offline conversion and derived artifacts (including unchunked data).
use super::backing::{Backing, FileRanges, MAX_SOURCE_RECORD_BYTES};
use crate::McapOpenError;
use mcap::{
    Channel, Message, Schema,
    records::Record,
    sans_io::{LinearReadEvent, LinearReader, LinearReaderOptions},
};
use std::{borrow::Cow, collections::HashMap, sync::Arc};

pub struct FileMessageStream {
    backing: Backing,
    reader: LinearReader,
    position: u64,
    schemas: HashMap<u16, Arc<Schema<'static>>>,
    channels: HashMap<u16, Arc<Channel<'static>>>,
    done: bool,
}
impl FileMessageStream {
    pub fn open(path: &std::path::Path) -> Result<Self, McapOpenError> {
        Ok(Self {
            backing: Backing::File(Arc::new(FileRanges::open(path)?)),
            reader: LinearReader::new_with_options(
                LinearReaderOptions::default()
                    .with_record_length_limit(MAX_SOURCE_RECORD_BYTES)
                    .with_prevalidate_chunk_crcs(true),
            ),
            position: 0,
            schemas: HashMap::new(),
            channels: HashMap::new(),
            done: false,
        })
    }
    /// Reads source metadata through the same file handle without requiring Chunk Index records.
    pub fn read_metadata(
        &self,
    ) -> Result<(mcap::Summary, crate::McapSummaryIdentity), McapOpenError> {
        let summary = self.backing.summary()?;
        let stats = summary
            .stats
            .as_ref()
            .ok_or(McapOpenError::SummaryRequired)?;
        let identity = crate::McapSummaryIdentity {
            file_size: self.backing.len(),
            summary_crc: self.backing.summary_crc()?,
            message_start_time: stats.message_start_time,
            message_end_time: stats.message_end_time,
            message_count: stats.message_count,
            schema_count: stats.schema_count,
            channel_count: stats.channel_count,
            chunk_count: summary.chunk_indexes.len(),
        };
        Ok((summary, identity))
    }
    fn next_message(&mut self) -> Result<Option<Message<'static>>, McapOpenError> {
        while let Some(event) = self.reader.next_event() {
            match event? {
                LinearReadEvent::ReadRequest(hint) => {
                    let remaining = self.backing.len().saturating_sub(self.position);
                    let length = (hint.clamp(1, 65536) as u64).min(remaining) as usize;
                    if length == 0 {
                        return Err(McapOpenError::Io(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "truncated MCAP stream",
                        )));
                    }
                    let data = self.backing.read(self.position, length)?;
                    self.reader.insert(length).copy_from_slice(&data);
                    self.reader.notify_read(length);
                    self.position += length as u64;
                }
                LinearReadEvent::Record { data, opcode } => match mcap::parse_record(opcode, data)?
                {
                    Record::Schema { header, data } => {
                        let schema = Arc::new(Schema {
                            id: header.id,
                            name: header.name,
                            encoding: header.encoding,
                            data: Cow::Owned(data.into_owned()),
                        });
                        if self
                            .schemas
                            .get(&schema.id)
                            .is_some_and(|old| old != &schema)
                        {
                            return Err(
                                mcap::McapError::ConflictingSchemas(schema.name.clone()).into()
                            );
                        }
                        self.schemas.insert(schema.id, schema);
                    }
                    Record::Channel(channel) => {
                        let schema = if channel.schema_id == 0 {
                            None
                        } else {
                            Some(
                                self.schemas
                                    .get(&channel.schema_id)
                                    .ok_or_else(|| {
                                        mcap::McapError::UnknownSchema(
                                            channel.topic.clone(),
                                            channel.schema_id,
                                        )
                                    })?
                                    .clone(),
                            )
                        };
                        let channel = Arc::new(Channel {
                            id: channel.id,
                            topic: channel.topic,
                            message_encoding: channel.message_encoding,
                            metadata: channel.metadata,
                            schema,
                        });
                        if self
                            .channels
                            .get(&channel.id)
                            .is_some_and(|old| old != &channel)
                        {
                            return Err(mcap::McapError::ConflictingChannels(
                                channel.topic.clone(),
                            )
                            .into());
                        }
                        self.channels.insert(channel.id, channel);
                    }
                    Record::Message { header, data } => {
                        return Ok(Some(Message {
                            channel: self
                                .channels
                                .get(&header.channel_id)
                                .ok_or(mcap::McapError::BadIndex)?
                                .clone(),
                            sequence: header.sequence,
                            log_time: header.log_time,
                            publish_time: header.publish_time,
                            data: Cow::Owned(data.into_owned()),
                        }));
                    }
                    Record::DataEnd(_) => return Ok(None),
                    _ => {}
                },
            }
        }
        Ok(None)
    }
}
impl Iterator for FileMessageStream {
    type Item = Result<Message<'static>, McapOpenError>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        match self.next_message() {
            Ok(Some(message)) => Some(Ok(message)),
            Ok(None) => {
                self.done = true;
                None
            }
            Err(error) => {
                self.done = true;
                Some(Err(error))
            }
        }
    }
}
