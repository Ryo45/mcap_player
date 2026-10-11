//! I/O-independent decoding and traversal of an owned MCAP Chunk backing.

use crate::ArrivalTime;
use bytes::Bytes;
use mcap::records::op;
use std::fmt;
#[cfg(feature = "mcap-lz4")]
use std::io::Read;
use viewer_core::{RawMessage, StreamId};

#[derive(Debug)]
pub enum McapChunkError {
    Mcap(mcap::McapError),
    InvalidRecord(&'static str),
    TimestampOverflow,
    UnsupportedCompression(String),
    Decompression {
        format: &'static str,
        source: std::io::Error,
    },
}

impl fmt::Display for McapChunkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mcap(error) => write!(f, "MCAP error: {error}"),
            Self::InvalidRecord(reason) => write!(f, "invalid MCAP Chunk: {reason}"),
            Self::TimestampOverflow => write!(f, "MCAP timestamp exceeds signed nanosecond range"),
            Self::UnsupportedCompression(format) => {
                write!(f, "unsupported MCAP Chunk compression: {format}")
            }
            Self::Decompression { format, source } => write!(f, "{format} decompression: {source}"),
        }
    }
}

impl std::error::Error for McapChunkError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Mcap(error) => Some(error),
            Self::Decompression { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<mcap::McapError> for McapChunkError {
    fn from(error: mcap::McapError) -> Self {
        Self::Mcap(error)
    }
}

/// Decode a previously read Chunk payload. The caller bounds input/output sizes and
/// decides which compression formats its source accepts. Uncompressed bytes retain
/// their original backing; compressed bytes create one shared decompressed backing.
pub fn decode_mcap_chunk(
    compression: &str,
    compressed: Bytes,
    expected_size: usize,
) -> Result<Bytes, McapChunkError> {
    let backing = match compression {
        "" => compressed,
        #[cfg(feature = "mcap-zstd")]
        "zstd" => Bytes::from(zstd::bulk::decompress(&compressed, expected_size).map_err(
            |source| McapChunkError::Decompression {
                format: "zstd",
                source,
            },
        )?),
        #[cfg(feature = "mcap-lz4")]
        "lz4" => {
            let limit = u64::try_from(expected_size)
                .ok()
                .and_then(|size| size.checked_add(1))
                .ok_or(McapChunkError::InvalidRecord("decompressed size overflow"))?;
            let mut output = Vec::with_capacity(expected_size);
            lz4::Decoder::new(std::io::Cursor::new(compressed))
                .map_err(|source| McapChunkError::Decompression {
                    format: "lz4",
                    source,
                })?
                .take(limit)
                .read_to_end(&mut output)
                .map_err(|source| McapChunkError::Decompression {
                    format: "lz4",
                    source,
                })?;
            Bytes::from(output)
        }
        value => return Err(McapChunkError::UnsupportedCompression(value.into())),
    };
    if backing.len() != expected_size {
        return Err(McapChunkError::InvalidRecord(if compression.is_empty() {
            "uncompressed data size does not match Chunk Index"
        } else {
            "decompressed data size does not match Chunk Index"
        }));
    }
    Ok(backing)
}

/// Visit each physical record in an uncompressed Chunk. `None` denotes a non-message
/// record, allowing the caller to check cancellation even between metadata records.
/// Message payloads are slices of `backing`, and offsets are relative to that backing.
///
/// The visitor owns selection, admission budgets and cancellation. An error stops
/// traversal immediately; callers must stage results until the whole visit succeeds.
pub fn visit_mcap_chunk_records<E: From<McapChunkError>>(
    backing: &Bytes,
    mut visit: impl FnMut(u64, Option<RawMessage>) -> Result<(), E>,
) -> Result<(), E> {
    let mut offset = 0usize;
    while offset < backing.len() {
        let record_offset = u64::try_from(offset)
            .map_err(|_| McapChunkError::InvalidRecord("record offset overflow"))?;
        let header_end = offset
            .checked_add(9)
            .ok_or(McapChunkError::InvalidRecord("record header overflow"))?;
        let head = backing
            .get(offset..header_end)
            .ok_or(McapChunkError::InvalidRecord("truncated record header"))?;
        let body_length = usize::try_from(u64::from_le_bytes(
            head[1..9]
                .try_into()
                .expect("record length has eight bytes"),
        ))
        .map_err(|_| McapChunkError::InvalidRecord("record body is too large"))?;
        let body_end = header_end
            .checked_add(body_length)
            .ok_or(McapChunkError::InvalidRecord("record body overflow"))?;
        let body = backing
            .get(header_end..body_end)
            .ok_or(McapChunkError::InvalidRecord("truncated record body"))?;
        let message = if head[0] == op::MESSAGE {
            let mcap::records::Record::Message { header, data } =
                mcap::parse_record(op::MESSAGE, body).map_err(McapChunkError::from)?
            else {
                return Err(McapChunkError::InvalidRecord(
                    "message opcode parsed as another record",
                )
                .into());
            };
            let payload_start = body_end
                .checked_sub(data.len())
                .ok_or(McapChunkError::InvalidRecord("payload range underflow"))?;
            Some(RawMessage {
                stream_id: StreamId(u32::from(header.channel_id)),
                arrival_time: ArrivalTime(
                    i64::try_from(header.log_time)
                        .map_err(|_| McapChunkError::TimestampOverflow)?,
                ),
                publish_time_ns: Some(header.publish_time),
                sequence: Some(header.sequence),
                payload: backing.slice(payload_start..body_end),
            })
        } else {
            None
        };
        visit(record_offset, message)?;
        offset = body_end;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcap::{Summary, WriteOptions, Writer, records::MessageHeader};
    use std::{collections::BTreeMap, io::Cursor};

    fn chunk() -> Bytes {
        let mut file = Cursor::new(Vec::new());
        {
            let mut writer = Writer::with_options(
                &mut file,
                WriteOptions::new().compression(None).chunk_size(None),
            )
            .unwrap();
            let schema = writer
                .add_schema("demo/msg/Sample", "ros2msg", b"uint8 value")
                .unwrap();
            let channel = writer
                .add_channel(schema, "/a", "cdr", &BTreeMap::new())
                .unwrap();
            for sequence in 0..2 {
                writer
                    .write_to_known_channel(
                        &MessageHeader {
                            channel_id: channel,
                            sequence,
                            log_time: u64::from(sequence),
                            publish_time: 0,
                        },
                        &[sequence as u8, 9],
                    )
                    .unwrap();
            }
            writer.finish().unwrap();
        }
        let file = Bytes::from(file.into_inner());
        let summary = Summary::read(&file).unwrap().unwrap();
        let chunk = &summary.chunk_indexes[0];
        let start = chunk.compressed_data_offset().unwrap() as usize;
        file.slice(start..start + chunk.compressed_size as usize)
    }

    fn messages(backing: &Bytes) -> Result<Vec<(u64, RawMessage)>, McapChunkError> {
        let mut messages = Vec::new();
        visit_mcap_chunk_records(backing, |offset, message| {
            if let Some(message) = message {
                messages.push((offset, message));
            }
            Ok::<_, McapChunkError>(())
        })?;
        Ok(messages)
    }

    #[test]
    fn visits_metadata_and_exact_messages_with_shared_payloads_and_original_zero_values() {
        let backing = chunk();
        let mut metadata = 0;
        visit_mcap_chunk_records(&backing, |_, message| {
            metadata += usize::from(message.is_none());
            Ok::<_, McapChunkError>(())
        })
        .unwrap();
        assert!(metadata > 0);
        let messages = messages(&backing).unwrap();
        assert_eq!(messages.len(), 2);
        assert!(messages[0].0 < messages[1].0);
        for (sequence, (offset, raw)) in messages.iter().enumerate() {
            assert_eq!(backing[*offset as usize], op::MESSAGE);
            assert_eq!(raw.arrival_time, ArrivalTime(sequence as i64));
            assert_eq!(raw.publish_time_ns, Some(0));
            assert_eq!(raw.sequence, Some(sequence as u32));
            assert_eq!(raw.payload.as_ref(), &[sequence as u8, 9]);
            let start = raw.payload.as_ptr() as usize;
            assert!(start >= backing.as_ptr() as usize);
            assert!(start + raw.payload.len() <= backing.as_ptr() as usize + backing.len());
        }
    }

    #[test]
    fn rejects_truncated_records_overflowing_lengths_and_out_of_range_timestamps() {
        let backing = chunk();
        let first = messages(&backing).unwrap()[0].0 as usize;
        for bytes in [backing.slice(..8), backing.slice(..backing.len() - 1)] {
            assert!(matches!(
                messages(&bytes),
                Err(McapChunkError::InvalidRecord(_))
            ));
        }
        let mut invalid = backing.to_vec();
        invalid[first + 1..first + 9].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(matches!(
            messages(&Bytes::from(invalid)),
            Err(McapChunkError::InvalidRecord(_))
        ));
        let mut invalid = backing.to_vec();
        invalid[first + 15..first + 23].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(matches!(
            messages(&Bytes::from(invalid)),
            Err(McapChunkError::TimestampOverflow)
        ));
    }

    #[test]
    fn visitor_failure_stops_before_admitting_another_message() {
        let mut admitted = 0;
        let result = visit_mcap_chunk_records(&chunk(), |_, message| {
            if message.is_some() {
                admitted += 1;
                return Err(McapChunkError::InvalidRecord("admission cancelled"));
            }
            Ok(())
        });
        assert!(matches!(
            result,
            Err(McapChunkError::InvalidRecord("admission cancelled"))
        ));
        assert_eq!(admitted, 1);
    }

    #[test]
    fn uncompressed_decode_preserves_backing_and_checks_the_declared_length() {
        let bytes = chunk();
        assert_eq!(
            decode_mcap_chunk("", bytes.clone(), bytes.len())
                .unwrap()
                .as_ptr(),
            bytes.as_ptr()
        );
        assert!(matches!(
            decode_mcap_chunk("", bytes.clone(), bytes.len() + 1),
            Err(McapChunkError::InvalidRecord(_))
        ));
        assert!(matches!(
            decode_mcap_chunk("unknown", bytes, 0),
            Err(McapChunkError::UnsupportedCompression(_))
        ));
    }

    #[cfg(feature = "mcap-zstd")]
    #[test]
    fn zstd_decode_checks_size_and_retains_the_decompression_error_cause() {
        let bytes = chunk();
        let compressed = Bytes::from(zstd::bulk::compress(&bytes, 1).unwrap());
        assert_eq!(
            decode_mcap_chunk("zstd", compressed.clone(), bytes.len()).unwrap(),
            bytes
        );
        assert!(matches!(
            decode_mcap_chunk("zstd", compressed, bytes.len() + 1),
            Err(McapChunkError::InvalidRecord(_))
        ));
        let error =
            decode_mcap_chunk("zstd", Bytes::from_static(b"bad compressed input"), 32).unwrap_err();
        assert!(
            std::error::Error::source(&error)
                .unwrap()
                .is::<std::io::Error>()
        );
    }

    #[cfg(feature = "mcap-lz4")]
    #[test]
    fn lz4_decode_rejects_output_beyond_the_declared_size() {
        use std::io::Write;
        let mut encoder = lz4::EncoderBuilder::new().build(Vec::new()).unwrap();
        encoder.write_all(&[1; 4096]).unwrap();
        let (compressed, status) = encoder.finish();
        status.unwrap();
        assert!(matches!(
            decode_mcap_chunk("lz4", Bytes::from(compressed), 16),
            Err(McapChunkError::InvalidRecord(_))
        ));
    }
}
