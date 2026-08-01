//! Browser range-read spike parsing.
//!
//! This module deliberately is not a production MCAP reader or a reusable I/O abstraction. It
//! keeps browser I/O outside and only parses the three byte ranges used by the diagnostic spike.

use mcap::{
    MAGIC, parse_record,
    read::{ChunkReader, LinearReader},
    records::{self, Record, op},
};
use std::{borrow::Cow, fmt, io::SeekFrom};
use viewer_core::{
    ArrivalTime, CameraId, DomainState, PipelineSet, RawMessage, StreamBinding, StreamDescriptor,
    StreamId,
};

pub(crate) const RECORD_HEADER_LEN: usize = 1 + 8;
pub(crate) const FOOTER_BODY_LEN: usize = 8 + 8 + 4;
pub(crate) const FOOTER_TAIL_LEN: usize = RECORD_HEADER_LEN + FOOTER_BODY_LEN + MAGIC.len();
const JS_MAX_SAFE_INTEGER: u64 = (1_u64 << 53) - 1;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RequestGeneration {
    current: u64,
}

impl RequestGeneration {
    pub(crate) fn begin(&mut self) -> u64 {
        self.current = self.current.wrapping_add(1);
        self.current
    }

    pub(crate) fn is_current(self, generation: u64) -> bool {
        self.current == generation
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PipelineProbeResult {
    pub(crate) domain: &'static str,
    pub(crate) arrival_time: ArrivalTime,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ByteRange {
    pub(crate) offset: u64,
    pub(crate) length: usize,
}

impl ByteRange {
    pub(crate) fn end(self) -> u64 {
        self.offset + self.length as u64
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FooterInfo {
    pub(crate) footer_offset: u64,
    pub(crate) summary_start: u64,
    pub(crate) summary_offset_start: u64,
    pub(crate) summary_crc: u32,
    pub(crate) summary_range: Option<ByteRange>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SchemaInfo {
    pub(crate) id: u16,
    pub(crate) name: String,
    pub(crate) encoding: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ChannelInfo {
    pub(crate) id: u16,
    pub(crate) schema_id: u16,
    pub(crate) topic: String,
    pub(crate) message_encoding: String,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SummaryCatalog {
    pub(crate) schemas: Vec<SchemaInfo>,
    pub(crate) channels: Vec<ChannelInfo>,
    pub(crate) statistics: Option<records::Statistics>,
    pub(crate) chunk_indexes: Vec<records::ChunkIndex>,
    pub(crate) attachment_indexes: Vec<records::AttachmentIndex>,
    pub(crate) metadata_indexes: Vec<records::MetadataIndex>,
    pub(crate) summary_offsets: Vec<records::SummaryOffset>,
}

impl SummaryCatalog {
    pub(crate) fn has_message_indexes(&self) -> bool {
        self.chunk_indexes
            .iter()
            .any(|index| index.message_index_length > 0 || !index.message_index_offsets.is_empty())
    }

    pub(crate) fn time_range(&self) -> Option<(u64, u64)> {
        self.statistics
            .as_ref()
            .map(|stats| (stats.message_start_time, stats.message_end_time))
            .or_else(|| {
                let start = self
                    .chunk_indexes
                    .iter()
                    .map(|index| index.message_start_time)
                    .min()?;
                let end = self
                    .chunk_indexes
                    .iter()
                    .map(|index| index.message_end_time)
                    .max()?;
                Some((start, end))
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ChunkInspection {
    pub(crate) record_count: Option<usize>,
    pub(crate) message_count: Option<usize>,
    pub(crate) status: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SpikeParseError {
    RangeOverflow,
    RangeOutsideFile {
        offset: u64,
        length: usize,
        file_size: u64,
    },
    JavaScriptNumberLimit(u64),
    EmptyFile,
    TruncatedFooter {
        actual: usize,
    },
    BadTrailingMagic,
    BadFooterOpcode(u8),
    BadFooterLength(u64),
    InvalidSummaryRange(String),
    InvalidChunkRange(String),
    Mcap(String),
}

impl fmt::Display for SpikeParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RangeOverflow => write!(formatter, "range end overflow"),
            Self::RangeOutsideFile {
                offset,
                length,
                file_size,
            } => write!(
                formatter,
                "range {offset}..+{length} exceeds file size {file_size}"
            ),
            Self::JavaScriptNumberLimit(value) => {
                write!(
                    formatter,
                    "offset {value} exceeds JavaScript's safe integer range"
                )
            }
            Self::EmptyFile => write!(formatter, "MCAP file is empty"),
            Self::TruncatedFooter { actual } => write!(
                formatter,
                "footer tail is truncated: expected {FOOTER_TAIL_LEN} bytes, got {actual}"
            ),
            Self::BadTrailingMagic => write!(formatter, "invalid MCAP trailing magic"),
            Self::BadFooterOpcode(opcode) => {
                write!(formatter, "expected Footer opcode 0x02, got {opcode:#04x}")
            }
            Self::BadFooterLength(length) => write!(
                formatter,
                "expected {FOOTER_BODY_LEN}-byte Footer body, got {length}"
            ),
            Self::InvalidSummaryRange(message) => {
                write!(formatter, "invalid summary range: {message}")
            }
            Self::InvalidChunkRange(message) => write!(formatter, "invalid chunk range: {message}"),
            Self::Mcap(message) => write!(formatter, "MCAP parse failed: {message}"),
        }
    }
}

impl std::error::Error for SpikeParseError {}

pub(crate) fn validate_range(
    file_size: u64,
    offset: u64,
    length: usize,
) -> Result<ByteRange, SpikeParseError> {
    if file_size == 0 {
        return Err(SpikeParseError::EmptyFile);
    }
    if file_size > JS_MAX_SAFE_INTEGER {
        return Err(SpikeParseError::JavaScriptNumberLimit(file_size));
    }
    let length_u64 = u64::try_from(length).map_err(|_| SpikeParseError::RangeOverflow)?;
    let end = offset
        .checked_add(length_u64)
        .ok_or(SpikeParseError::RangeOverflow)?;
    if end > file_size {
        return Err(SpikeParseError::RangeOutsideFile {
            offset,
            length,
            file_size,
        });
    }
    if offset > JS_MAX_SAFE_INTEGER {
        return Err(SpikeParseError::JavaScriptNumberLimit(offset));
    }
    if end > JS_MAX_SAFE_INTEGER {
        return Err(SpikeParseError::JavaScriptNumberLimit(end));
    }
    Ok(ByteRange { offset, length })
}

pub(crate) fn resolve_seek(
    file_size: u64,
    current: u64,
    seek: SeekFrom,
) -> Result<u64, SpikeParseError> {
    let resolved = match seek {
        SeekFrom::Start(offset) => i128::from(offset),
        SeekFrom::Current(delta) => i128::from(current) + i128::from(delta),
        SeekFrom::End(delta) => i128::from(file_size) + i128::from(delta),
    };
    let resolved = u64::try_from(resolved).map_err(|_| SpikeParseError::RangeOutsideFile {
        offset: current,
        length: 0,
        file_size,
    })?;
    if resolved > file_size {
        return Err(SpikeParseError::RangeOutsideFile {
            offset: resolved,
            length: 0,
            file_size,
        });
    }
    Ok(resolved)
}

pub(crate) fn feed_pipeline(
    summary: &mcap::Summary,
    header: records::MessageHeader,
    data: &[u8],
    topic: &str,
) -> Result<PipelineProbeResult, String> {
    let channel = summary
        .channels
        .get(&header.channel_id)
        .ok_or_else(|| format!("message references unknown channel {}", header.channel_id))?;
    if channel.topic != topic {
        return Err(format!(
            "topic filter yielded {}, expected {topic}",
            channel.topic
        ));
    }
    let schema = channel
        .schema
        .as_ref()
        .map(|schema| schema.name.clone())
        .unwrap_or_default();
    let stream_id = StreamId(u32::from(channel.id));
    let descriptor = StreamDescriptor {
        id: stream_id,
        topic: channel.topic.clone(),
        schema,
        message_encoding: channel.message_encoding.clone(),
    };
    let (binding, domain) = if topic == viewer_core::ODOM_TOPIC {
        (StreamBinding::Odometry, "odometry")
    } else if descriptor.schema == "sensor_msgs/msg/CompressedImage" {
        (StreamBinding::Camera(CameraId(0)), "camera")
    } else {
        return Err(format!(
            "spike only routes odometry or compressed camera data, got {topic} ({})",
            descriptor.schema
        ));
    };
    let arrival = i64::try_from(header.log_time)
        .map(ArrivalTime)
        .map_err(|_| "message log time exceeds ArrivalTime".to_owned())?;
    let mut pipelines = PipelineSet::new(&[descriptor], &[(stream_id, binding)]);
    let mut updates = Vec::new();
    pipelines.decode(
        RawMessage {
            stream_id,
            arrival_time: arrival,
            payload: data.to_vec(),
        },
        &mut updates,
    );
    if pipelines.counters().decoded != 1 || updates.len() != 1 {
        return Err(format!(
            "pipeline did not decode exactly one message: decoded={} errors={} updates={}",
            pipelines.counters().decoded,
            pipelines.counters().errors,
            updates.len()
        ));
    }
    let mut state = DomainState::default();
    state.apply_all(updates);
    let updated = match binding {
        StreamBinding::Odometry => state
            .telemetry
            .latest()
            .is_some_and(|frame| frame.arrival_time == arrival),
        StreamBinding::Camera(camera_id) => state
            .camera
            .latest_for(camera_id)
            .is_some_and(|frame| frame.arrival_time == arrival),
        _ => false,
    };
    if !updated {
        return Err(format!("{domain} PipelineSet did not update DomainState"));
    }
    Ok(PipelineProbeResult {
        domain,
        arrival_time: arrival,
    })
}

pub(crate) fn footer_tail_range(file_size: u64) -> Result<ByteRange, SpikeParseError> {
    if file_size == 0 {
        return Err(SpikeParseError::EmptyFile);
    }
    let minimum_file_size = (FOOTER_TAIL_LEN + MAGIC.len()) as u64;
    if file_size < minimum_file_size {
        return Err(SpikeParseError::TruncatedFooter {
            actual: usize::try_from(file_size).unwrap_or(usize::MAX),
        });
    }
    let offset =
        file_size
            .checked_sub(FOOTER_TAIL_LEN as u64)
            .ok_or(SpikeParseError::TruncatedFooter {
                actual: usize::try_from(file_size).unwrap_or(usize::MAX),
            })?;
    validate_range(file_size, offset, FOOTER_TAIL_LEN)
}

pub(crate) fn parse_footer_tail(
    file_size: u64,
    tail: &[u8],
) -> Result<FooterInfo, SpikeParseError> {
    if tail.len() != FOOTER_TAIL_LEN {
        return Err(SpikeParseError::TruncatedFooter { actual: tail.len() });
    }
    if file_size < (FOOTER_TAIL_LEN + MAGIC.len()) as u64 {
        return Err(SpikeParseError::TruncatedFooter {
            actual: usize::try_from(file_size).unwrap_or(usize::MAX),
        });
    }
    if &tail[FOOTER_TAIL_LEN - MAGIC.len()..] != MAGIC {
        return Err(SpikeParseError::BadTrailingMagic);
    }
    let opcode = tail[0];
    if opcode != op::FOOTER {
        return Err(SpikeParseError::BadFooterOpcode(opcode));
    }
    let body_length = u64::from_le_bytes(
        tail[1..RECORD_HEADER_LEN]
            .try_into()
            .expect("record length slice"),
    );
    if body_length != FOOTER_BODY_LEN as u64 {
        return Err(SpikeParseError::BadFooterLength(body_length));
    }
    let footer = match parse_record(
        opcode,
        &tail[RECORD_HEADER_LEN..RECORD_HEADER_LEN + FOOTER_BODY_LEN],
    )
    .map_err(|error| SpikeParseError::Mcap(error.to_string()))?
    {
        Record::Footer(footer) => footer,
        _ => return Err(SpikeParseError::BadFooterOpcode(opcode)),
    };
    let footer_offset = file_size
        .checked_sub(FOOTER_TAIL_LEN as u64)
        .ok_or(SpikeParseError::TruncatedFooter { actual: tail.len() })?;
    let summary_range = if footer.summary_start == 0 {
        None
    } else {
        if footer.summary_start >= footer_offset {
            return Err(SpikeParseError::InvalidSummaryRange(format!(
                "summary start {} is not before footer {}",
                footer.summary_start, footer_offset
            )));
        }
        if footer.summary_offset_start != 0
            && !(footer.summary_start..footer_offset).contains(&footer.summary_offset_start)
        {
            return Err(SpikeParseError::InvalidSummaryRange(format!(
                "summary offset start {} is outside {}..{}",
                footer.summary_offset_start, footer.summary_start, footer_offset
            )));
        }
        let length = usize::try_from(footer_offset - footer.summary_start)
            .map_err(|_| SpikeParseError::RangeOverflow)?;
        Some(validate_range(file_size, footer.summary_start, length)?)
    };
    Ok(FooterInfo {
        footer_offset,
        summary_start: footer.summary_start,
        summary_offset_start: footer.summary_offset_start,
        summary_crc: footer.summary_crc,
        summary_range,
    })
}

pub(crate) fn parse_summary_range(bytes: &[u8]) -> Result<SummaryCatalog, SpikeParseError> {
    if bytes.is_empty() {
        return Err(SpikeParseError::InvalidSummaryRange(
            "summary bytes are empty".to_owned(),
        ));
    }
    let mut catalog = SummaryCatalog::default();
    for record in LinearReader::sans_magic(bytes) {
        match record.map_err(|error| SpikeParseError::Mcap(error.to_string()))? {
            Record::Schema { header, .. } => catalog.schemas.push(SchemaInfo {
                id: header.id,
                name: header.name,
                encoding: header.encoding,
            }),
            Record::Channel(channel) => catalog.channels.push(ChannelInfo {
                id: channel.id,
                schema_id: channel.schema_id,
                topic: channel.topic,
                message_encoding: channel.message_encoding,
            }),
            Record::Statistics(statistics) => catalog.statistics = Some(statistics),
            Record::ChunkIndex(index) => catalog.chunk_indexes.push(index),
            Record::AttachmentIndex(index) => catalog.attachment_indexes.push(index),
            Record::MetadataIndex(index) => catalog.metadata_indexes.push(index),
            Record::SummaryOffset(offset) => catalog.summary_offsets.push(offset),
            _ => {}
        }
    }
    if catalog.schemas.is_empty()
        && catalog.channels.is_empty()
        && catalog.statistics.is_none()
        && catalog.chunk_indexes.is_empty()
        && catalog.attachment_indexes.is_empty()
        && catalog.metadata_indexes.is_empty()
        && catalog.summary_offsets.is_empty()
    {
        return Err(SpikeParseError::InvalidSummaryRange(
            "range contained no summary records".to_owned(),
        ));
    }
    catalog.schemas.sort_by_key(|schema| schema.id);
    catalog.channels.sort_by_key(|channel| channel.id);
    Ok(catalog)
}

pub(crate) fn chunk_range(
    file_size: u64,
    index: &records::ChunkIndex,
) -> Result<ByteRange, SpikeParseError> {
    let length = usize::try_from(index.chunk_length).map_err(|_| SpikeParseError::RangeOverflow)?;
    validate_range(file_size, index.chunk_start_offset, length)
        .map_err(|error| SpikeParseError::InvalidChunkRange(error.to_string()))
}

pub(crate) fn inspect_chunk(bytes: &[u8]) -> Result<ChunkInspection, SpikeParseError> {
    if bytes.len() < RECORD_HEADER_LEN {
        return Err(SpikeParseError::InvalidChunkRange(
            "chunk record header is truncated".to_owned(),
        ));
    }
    if bytes[0] != op::CHUNK {
        return Err(SpikeParseError::InvalidChunkRange(format!(
            "expected Chunk opcode 0x06, got {:#04x}",
            bytes[0]
        )));
    }
    let body_length = u64::from_le_bytes(
        bytes[1..RECORD_HEADER_LEN]
            .try_into()
            .expect("record length slice"),
    );
    if body_length != (bytes.len() - RECORD_HEADER_LEN) as u64 {
        return Err(SpikeParseError::InvalidChunkRange(format!(
            "record body says {body_length} bytes, range contains {}",
            bytes.len() - RECORD_HEADER_LEN
        )));
    }
    let (header, data) = match parse_record(op::CHUNK, &bytes[RECORD_HEADER_LEN..])
        .map_err(|error| SpikeParseError::Mcap(error.to_string()))?
    {
        Record::Chunk { header, data } => (header, data),
        _ => unreachable!("Chunk opcode parsed as another record"),
    };
    if !header.compression.is_empty() {
        return Ok(ChunkInspection {
            record_count: None,
            message_count: None,
            status: format!(
                "range access succeeded; '{}' decompression is outside this spike boundary",
                header.compression
            ),
        });
    }
    let data: &[u8] = match &data {
        Cow::Borrowed(data) => data,
        Cow::Owned(data) => data,
    };
    let mut record_count = 0;
    let mut message_count = 0;
    for record in
        ChunkReader::new(header, data).map_err(|error| SpikeParseError::Mcap(error.to_string()))?
    {
        let record = record.map_err(|error| SpikeParseError::Mcap(error.to_string()))?;
        record_count += 1;
        if matches!(record, Record::Message { .. }) {
            message_count += 1;
        }
    }
    Ok(ChunkInspection {
        record_count: Some(record_count),
        message_count: Some(message_count),
        status: "uncompressed chunk parsed with mcap::ChunkReader".to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        borrow::Cow,
        collections::BTreeMap,
        fs::File,
        io::{Cursor, Read, Seek, SeekFrom},
        path::Path,
        sync::Arc,
    };
    use viewer_core::{CompressedImage, MeasurementTime, encode_compressed_image_cdr};

    const FIXTURE: &str = "../../tests/fixtures/camera-jpeg/camera_7_5s.mcap";

    fn footer_bytes(summary_start: u64, summary_offset_start: u64, crc: u32) -> Vec<u8> {
        let mut bytes = vec![op::FOOTER];
        bytes.extend_from_slice(&(FOOTER_BODY_LEN as u64).to_le_bytes());
        bytes.extend_from_slice(&summary_start.to_le_bytes());
        bytes.extend_from_slice(&summary_offset_start.to_le_bytes());
        bytes.extend_from_slice(&crc.to_le_bytes());
        bytes.extend_from_slice(MAGIC);
        bytes
    }

    fn read_exact_range(path: &Path, range: ByteRange) -> Vec<u8> {
        let mut file = File::open(path).unwrap();
        file.seek(SeekFrom::Start(range.offset)).unwrap();
        let mut bytes = vec![0; range.length];
        file.read_exact(&mut bytes).unwrap();
        assert_eq!(file.stream_position().unwrap(), range.end());
        bytes
    }

    #[test]
    fn parses_footer_and_rejects_magic_or_truncation() {
        let file_size = 1_000;
        let bytes = footer_bytes(700, 900, 0x1234_5678);
        let footer = parse_footer_tail(file_size, &bytes).unwrap();
        assert_eq!(footer.footer_offset, 963);
        assert_eq!(
            footer.summary_range,
            Some(ByteRange {
                offset: 700,
                length: 263
            })
        );
        assert_eq!(footer.summary_crc, 0x1234_5678);

        let mut bad_magic = bytes.clone();
        *bad_magic.last_mut().unwrap() = 0;
        assert_eq!(
            parse_footer_tail(file_size, &bad_magic).unwrap_err(),
            SpikeParseError::BadTrailingMagic
        );
        assert!(matches!(
            parse_footer_tail(file_size, &bytes[..bytes.len() - 1]),
            Err(SpikeParseError::TruncatedFooter { .. })
        ));
    }

    #[test]
    fn validates_ranges_without_reading_extra_bytes() {
        let data = b"0123456789";
        let range = validate_range(data.len() as u64, 3, 4).unwrap();
        assert_eq!(&data[range.offset as usize..range.end() as usize], b"3456");
        assert!(matches!(
            validate_range(10, u64::MAX, 2),
            Err(SpikeParseError::RangeOverflow)
        ));
        assert!(matches!(
            validate_range(10, 8, 3),
            Err(SpikeParseError::RangeOutsideFile { .. })
        ));
        assert!(matches!(
            validate_range(JS_MAX_SAFE_INTEGER + 1, 0, 0),
            Err(SpikeParseError::JavaScriptNumberLimit(_))
        ));
        assert_eq!(validate_range(10, 10, 0).unwrap().length, 0);
        assert_eq!(validate_range(0, 0, 0), Err(SpikeParseError::EmptyFile));
    }

    #[test]
    fn resolves_sans_io_seeks_and_rejects_out_of_file_positions() {
        assert_eq!(resolve_seek(1_000, 100, SeekFrom::Start(25)).unwrap(), 25);
        assert_eq!(
            resolve_seek(1_000, 100, SeekFrom::Current(-20)).unwrap(),
            80
        );
        assert_eq!(resolve_seek(1_000, 100, SeekFrom::End(-37)).unwrap(), 963);
        assert!(resolve_seek(1_000, 0, SeekFrom::End(1)).is_err());
        assert!(resolve_seek(1_000, 0, SeekFrom::Current(-1)).is_err());
    }

    #[test]
    fn replacing_generation_invalidates_an_older_seek() {
        let mut generations = RequestGeneration::default();
        let first = generations.begin();
        assert!(generations.is_current(first));
        let replacement = generations.begin();
        assert!(!generations.is_current(first));
        assert!(generations.is_current(replacement));
    }

    #[test]
    fn indexed_reader_filters_seeks_and_updates_camera_domain() {
        use mcap::sans_io::{
            IndexedReadEvent, IndexedReader, IndexedReaderOptions, SummaryReadEvent, SummaryReader,
            SummaryReaderOptions,
        };

        let camera_topic = "/camera/probe/image/compressed";
        let schema = Arc::new(mcap::Schema {
            id: 1,
            name: "sensor_msgs/msg/CompressedImage".to_owned(),
            encoding: "ros2msg".to_owned(),
            data: Cow::Borrowed(&[]),
        });
        let camera_channel = Arc::new(mcap::Channel {
            id: 1,
            topic: camera_topic.to_owned(),
            schema: Some(schema.clone()),
            message_encoding: "cdr".to_owned(),
            metadata: BTreeMap::new(),
        });
        let ignored_channel = Arc::new(mcap::Channel {
            id: 2,
            topic: "/ignored".to_owned(),
            schema: Some(schema),
            message_encoding: "cdr".to_owned(),
            metadata: BTreeMap::new(),
        });
        let payload = encode_compressed_image_cdr(&CompressedImage {
            measurement_time: MeasurementTime(30),
            frame_id: "probe-camera".to_owned(),
            format: "jpeg".to_owned(),
            jpeg: vec![0xff, 0xd8, 0xff, 0xd9],
        })
        .unwrap();
        let mut writer = mcap::WriteOptions::new()
            .compression(None)
            .chunk_size(None)
            .create(Cursor::new(Vec::new()))
            .unwrap();
        for (channel, time, data) in [
            (ignored_channel, 25, vec![0]),
            (camera_channel, 30, payload),
        ] {
            writer
                .write(&mcap::Message {
                    channel,
                    sequence: 0,
                    log_time: time,
                    publish_time: time,
                    data: Cow::Owned(data),
                })
                .unwrap();
            writer.flush().unwrap();
        }
        writer.finish().unwrap();
        let bytes = writer.into_inner().into_inner();

        let mut cursor = Cursor::new(bytes.as_slice());
        let mut summary_reader = SummaryReader::new_with_options(
            SummaryReaderOptions::default().with_file_size(bytes.len() as u64),
        );
        while let Some(event) = summary_reader.next_event() {
            match event.unwrap() {
                SummaryReadEvent::ReadRequest(length) => {
                    let read = cursor.read(summary_reader.insert(length)).unwrap();
                    summary_reader.notify_read(read);
                }
                SummaryReadEvent::SeekRequest(seek) => {
                    let position = cursor.seek(seek).unwrap();
                    summary_reader.notify_seeked(position);
                }
            }
        }
        let summary = summary_reader.finish().unwrap();
        let options = IndexedReaderOptions::new()
            .include_topics([camera_topic])
            .log_time_on_or_after(20);
        let mut indexed = IndexedReader::new_with_options(&summary, options).unwrap();
        let mut requested = Vec::new();
        let pipeline = loop {
            match indexed.next_event().unwrap().unwrap() {
                IndexedReadEvent::ReadChunkRequest { offset, length } => {
                    requested.push((offset, length));
                    indexed
                        .insert_chunk_record_data(
                            offset,
                            &bytes[offset as usize..offset as usize + length],
                        )
                        .unwrap();
                }
                IndexedReadEvent::Message { header, data } => {
                    assert_eq!(header.log_time, 30);
                    break feed_pipeline(&summary, header, data, camera_topic).unwrap();
                }
            }
        };
        assert_eq!(pipeline.domain, "camera");
        assert_eq!(pipeline.arrival_time, ArrivalTime(30));
        assert_eq!(requested.len(), 1, "topic index should prune ignored chunk");
        let requested_offset = requested[0].0;
        let chunk = summary
            .chunk_indexes
            .iter()
            .find(|chunk| {
                chunk
                    .compressed_data_offset()
                    .is_ok_and(|offset| offset == requested_offset)
            })
            .unwrap();
        assert!(chunk.message_index_offsets.contains_key(&1));
        assert_eq!(requested[0].1 as u64, chunk.compressed_size);
    }

    #[test]
    fn rejects_invalid_summary_ranges() {
        let bytes = footer_bytes(990, 0, 0);
        assert!(matches!(
            parse_footer_tail(1_000, &bytes),
            Err(SpikeParseError::InvalidSummaryRange(_))
        ));
        assert!(matches!(
            parse_summary_range(&[0; 16]),
            Err(SpikeParseError::Mcap(_)) | Err(SpikeParseError::InvalidSummaryRange(_))
        ));
    }

    #[test]
    fn reads_only_footer_and_summary_ranges_from_existing_fixture() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
        let file_size = path.metadata().unwrap().len();
        let footer_range = footer_tail_range(file_size).unwrap();
        let footer = parse_footer_tail(file_size, &read_exact_range(&path, footer_range)).unwrap();
        let summary_range = footer.summary_range.unwrap();
        let summary_bytes = read_exact_range(&path, summary_range);
        assert_eq!(summary_bytes.len(), summary_range.length);
        let catalog = parse_summary_range(&summary_bytes).unwrap();
        assert!(!catalog.schemas.is_empty());
        assert!(!catalog.channels.is_empty());
        assert!(!catalog.chunk_indexes.is_empty());
        assert!(catalog.time_range().is_some());
        let _has_message_indexes = catalog.has_message_indexes();
        let chunk = chunk_range(file_size, &catalog.chunk_indexes[0]).unwrap();
        assert!(chunk.end() <= footer.summary_start);
        let chunk_bytes = read_exact_range(&path, chunk);
        let inspection = inspect_chunk(&chunk_bytes).unwrap();
        assert!(!inspection.status.is_empty());
    }

    #[test]
    #[ignore = "manual range-read spike measurement for the large requested recording"]
    fn measure_requested_recording_without_full_read() {
        measure_recording("../../mcap/turtlebot3_7cam_fhd/turtlebot3_7cam_fhd_0.mcap");
    }

    #[test]
    #[ignore = "manual range-read spike measurement for the uncompressed recording"]
    fn measure_uncompressed_recording_without_full_read() {
        measure_recording("../../mcap/turtlebot3_7cam_fhd/turtlebot3_7cam_fhd_0_uncompressed.mcap");
    }

    #[test]
    #[ignore = "manual IndexedReader end-to-end measurement for the uncompressed recording"]
    fn measure_uncompressed_indexed_reader_to_domain_state() {
        use mcap::sans_io::{
            IndexedReadEvent, IndexedReader, IndexedReaderOptions, SummaryReadEvent, SummaryReader,
            SummaryReaderOptions,
        };

        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../mcap/turtlebot3_7cam_fhd/turtlebot3_7cam_fhd_0_uncompressed.mcap");
        let file_size = path.metadata().unwrap().len();
        let mut file = File::open(&path).unwrap();
        let mut summary_reader = SummaryReader::new_with_options(
            SummaryReaderOptions::default().with_file_size(file_size),
        );
        let mut position = 0_u64;
        let mut seek_count = 0_u32;
        let mut summary_reads = 0_u32;
        let mut summary_bytes = 0_u64;
        while let Some(event) = summary_reader.next_event() {
            match event.unwrap() {
                SummaryReadEvent::SeekRequest(seek) => {
                    position = resolve_seek(file_size, position, seek).unwrap();
                    file.seek(SeekFrom::Start(position)).unwrap();
                    seek_count += 1;
                    summary_reader.notify_seeked(position);
                }
                SummaryReadEvent::ReadRequest(need) => {
                    let remaining = usize::try_from(file_size - position).unwrap_or(usize::MAX);
                    let requested = if seek_count >= 2 {
                        need.max(256 * 1024).min(remaining)
                    } else {
                        need
                    };
                    file.read_exact(summary_reader.insert(requested)).unwrap();
                    summary_reader.notify_read(requested);
                    position += requested as u64;
                    summary_reads += 1;
                    summary_bytes += requested as u64;
                }
            }
        }
        let summary = summary_reader.finish().unwrap();
        let channel = summary
            .channels
            .values()
            .find(|channel| channel.topic == viewer_core::ODOM_TOPIC)
            .or_else(|| {
                summary.channels.values().find(|channel| {
                    channel
                        .schema
                        .as_ref()
                        .is_some_and(|schema| schema.name == "sensor_msgs/msg/CompressedImage")
                })
            })
            .unwrap();
        let topic = channel.topic.clone();
        let channel_id = channel.id;
        let stats = summary.stats.as_ref().unwrap();
        let target = stats.message_start_time
            + stats
                .message_end_time
                .saturating_sub(stats.message_start_time)
                / 2;
        let options = IndexedReaderOptions::new()
            .include_topics([topic.clone()])
            .log_time_on_or_after(target);
        let mut indexed = IndexedReader::new_with_options(&summary, options).unwrap();
        let mut chunk_reads = 0_u32;
        let mut chunk_bytes = 0_u64;
        let mut requested_chunks = Vec::new();
        let (message_time, pipeline) = loop {
            match indexed.next_event().unwrap().unwrap() {
                IndexedReadEvent::ReadChunkRequest { offset, length } => {
                    let index = summary
                        .chunk_indexes
                        .iter()
                        .find(|index| {
                            index
                                .compressed_data_offset()
                                .is_ok_and(|data_offset| data_offset == offset)
                        })
                        .unwrap();
                    assert_eq!(index.compressed_size, length as u64);
                    assert!(index.message_index_offsets.contains_key(&channel_id));
                    file.seek(SeekFrom::Start(offset)).unwrap();
                    let mut bytes = vec![0; length];
                    file.read_exact(&mut bytes).unwrap();
                    indexed.insert_chunk_record_data(offset, &bytes).unwrap();
                    chunk_reads += 1;
                    chunk_bytes += length as u64;
                    requested_chunks.push((
                        index.chunk_start_offset,
                        offset,
                        length,
                        index.message_index_length,
                    ));
                }
                IndexedReadEvent::Message { header, data } => {
                    assert!(header.log_time >= target);
                    let message_time = header.log_time;
                    break (
                        message_time,
                        feed_pipeline(&summary, header, data, &topic).unwrap(),
                    );
                }
            }
        };
        eprintln!("path={}", path.display());
        eprintln!("file_size={file_size}");
        eprintln!("summary_reader_range_reads={summary_reads}");
        eprintln!("summary_reader_bytes={summary_bytes}");
        eprintln!("target_topic={topic}");
        eprintln!("target_log_time={target}");
        eprintln!("message_log_time={message_time}");
        eprintln!("chunk_reads={chunk_reads}");
        eprintln!("chunk_bytes={chunk_bytes}");
        eprintln!("requested_chunks={requested_chunks:?}");
        eprintln!("pipeline_domain={}", pipeline.domain);
        eprintln!("domain_arrival={}", pipeline.arrival_time.0);
        assert_eq!(pipeline.arrival_time, ArrivalTime(message_time as i64));
    }

    fn measure_recording(relative_path: &str) {
        use std::time::Instant;

        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative_path);
        let file_size = path.metadata().unwrap().len();

        let footer_started = Instant::now();
        let footer_range = footer_tail_range(file_size).unwrap();
        let footer_bytes = read_exact_range(&path, footer_range);
        let footer_read = footer_started.elapsed();
        let footer = parse_footer_tail(file_size, &footer_bytes).unwrap();

        let summary_range = footer.summary_range.unwrap();
        let summary_read_started = Instant::now();
        let summary_bytes = read_exact_range(&path, summary_range);
        let summary_read = summary_read_started.elapsed();
        let summary_parse_started = Instant::now();
        let catalog = parse_summary_range(&summary_bytes).unwrap();
        let summary_parse = summary_parse_started.elapsed();

        let selected = catalog.chunk_indexes.first().unwrap();
        let selected_range = chunk_range(file_size, selected).unwrap();
        let chunk_read_started = Instant::now();
        let chunk_bytes = read_exact_range(&path, selected_range);
        let chunk_read = chunk_read_started.elapsed();
        let chunk_parse_started = Instant::now();
        let inspection = inspect_chunk(&chunk_bytes).unwrap();
        let chunk_parse = chunk_parse_started.elapsed();

        let catalog_bytes = footer_range.length as u64 + summary_range.length as u64;
        let total_bytes = catalog_bytes + selected_range.length as u64;
        eprintln!("path={}", path.display());
        eprintln!("file_size={file_size}");
        eprintln!(
            "footer_range={}..{}",
            footer_range.offset,
            footer_range.end()
        );
        eprintln!(
            "summary_range={}..{}",
            summary_range.offset,
            summary_range.end()
        );
        eprintln!("summary_offset_start={}", footer.summary_offset_start);
        eprintln!("summary_crc={:#010x}", footer.summary_crc);
        eprintln!("catalog_bytes={catalog_bytes}");
        eprintln!(
            "catalog_ratio_percent={:.9}",
            catalog_bytes as f64 / file_size as f64 * 100.0
        );
        eprintln!("range_reads=3");
        eprintln!("total_bytes_with_chunk={total_bytes}");
        eprintln!("footer_read_ms={:.3}", footer_read.as_secs_f64() * 1000.0);
        eprintln!("summary_read_ms={:.3}", summary_read.as_secs_f64() * 1000.0);
        eprintln!(
            "summary_parse_ms={:.3}",
            summary_parse.as_secs_f64() * 1000.0
        );
        eprintln!("schema_count={}", catalog.schemas.len());
        eprintln!("channel_count={}", catalog.channels.len());
        eprintln!("chunk_count={}", catalog.chunk_indexes.len());
        eprintln!(
            "attachment_index_count={}",
            catalog.attachment_indexes.len()
        );
        eprintln!("metadata_index_count={}", catalog.metadata_indexes.len());
        eprintln!("summary_offset_count={}", catalog.summary_offsets.len());
        eprintln!("has_message_indexes={}", catalog.has_message_indexes());
        eprintln!("time_range={:?}", catalog.time_range());
        let mut compression_counts = std::collections::BTreeMap::new();
        for chunk in &catalog.chunk_indexes {
            *compression_counts
                .entry(chunk.compression.as_str())
                .or_insert(0_usize) += 1;
        }
        eprintln!("compression_counts={compression_counts:?}");
        eprintln!("selected_chunk_offset={}", selected_range.offset);
        eprintln!("selected_chunk_length={}", selected_range.length);
        eprintln!("selected_chunk_compression={:?}", selected.compression);
        eprintln!(
            "selected_chunk_compressed_size={}",
            selected.compressed_size
        );
        eprintln!(
            "selected_chunk_uncompressed_size={}",
            selected.uncompressed_size
        );
        eprintln!(
            "selected_chunk_read_ms={:.3}",
            chunk_read.as_secs_f64() * 1000.0
        );
        eprintln!(
            "selected_chunk_parse_ms={:.3}",
            chunk_parse.as_secs_f64() * 1000.0
        );
        eprintln!("selected_chunk_status={}", inspection.status);
        eprintln!("selected_chunk_record_count={:?}", inspection.record_count);
        eprintln!(
            "selected_chunk_message_count={:?}",
            inspection.message_count
        );
    }
}
