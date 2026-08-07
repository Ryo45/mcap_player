use crate::ProtocolError;
use bytes::{BufMut, Bytes, BytesMut};
use std::ops::Range;

const MAGIC: &[u8; 4] = b"AVBT";
const BATCH_HEADER_LEN: usize = 16;
const MESSAGE_HEADER_LEN: usize = 28;
pub const BATCH_VERSION: u16 = 1;
pub const BATCH_CONTENT_TYPE: &str = "application/vnd.autonomous-viewer.batch; version=1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RemoteMessageRef<'a> {
    pub stream_id: u32,
    pub sequence: u32,
    pub log_time_ns: u64,
    pub publish_time_ns: u64,
    pub payload: &'a [u8],
}

impl RemoteMessageRef<'_> {
    /// Locates this borrowed payload within the batch body that owns it.
    pub fn payload_range_in(&self, body: &[u8]) -> Option<Range<usize>> {
        let body_start = body.as_ptr() as usize;
        let payload_start = self.payload.as_ptr() as usize;
        let start = payload_start.checked_sub(body_start)?;
        let end = start.checked_add(self.payload.len())?;
        (end <= body.len()).then_some(start..end)
    }
}

#[derive(Debug, Default)]
pub struct BatchEncoder {
    frames: BytesMut,
    message_count: u32,
}

impl BatchEncoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn encoded_len(&self) -> usize {
        BATCH_HEADER_LEN + self.frames.len()
    }

    pub fn frame_len(payload_len: usize) -> Result<usize, ProtocolError> {
        let _ = u32::try_from(payload_len)
            .map_err(|_| ProtocolError::new("message payload exceeds u32"))?;
        MESSAGE_HEADER_LEN
            .checked_add(payload_len)
            .ok_or_else(|| ProtocolError::new("message frame length overflow"))
    }

    pub fn push(&mut self, message: RemoteMessageRef<'_>) -> Result<(), ProtocolError> {
        let payload_len = u32::try_from(message.payload.len())
            .map_err(|_| ProtocolError::new("message payload exceeds u32"))?;
        self.message_count = self
            .message_count
            .checked_add(1)
            .ok_or_else(|| ProtocolError::new("message count exceeds u32"))?;
        self.frames.put_u32_le(message.stream_id);
        self.frames.put_u32_le(message.sequence);
        self.frames.put_u64_le(message.log_time_ns);
        self.frames.put_u64_le(message.publish_time_ns);
        self.frames.put_u32_le(payload_len);
        self.frames.extend_from_slice(message.payload);
        Ok(())
    }

    pub fn finish(self) -> Bytes {
        let mut output = BytesMut::with_capacity(BATCH_HEADER_LEN + self.frames.len());
        output.extend_from_slice(MAGIC);
        output.put_u16_le(BATCH_VERSION);
        output.put_u16_le(0);
        output.put_u32_le(self.message_count);
        output.put_u32_le(0);
        output.unsplit(self.frames);
        output.freeze()
    }
}

pub struct BatchDecoder<'a> {
    body: &'a [u8],
    offset: usize,
    remaining: u32,
}

impl<'a> BatchDecoder<'a> {
    pub fn new(body: &'a [u8]) -> Result<Self, ProtocolError> {
        if body.len() < BATCH_HEADER_LEN {
            return Err(ProtocolError::new("truncated batch header"));
        }
        if &body[..4] != MAGIC {
            return Err(ProtocolError::new("bad batch magic"));
        }
        let version = u16::from_le_bytes(body[4..6].try_into().unwrap());
        if version != BATCH_VERSION {
            return Err(ProtocolError::new(format!(
                "unsupported batch version: {version}"
            )));
        }
        let flags = u16::from_le_bytes(body[6..8].try_into().unwrap());
        let reserved = u32::from_le_bytes(body[12..16].try_into().unwrap());
        if flags != 0 || reserved != 0 {
            return Err(ProtocolError::new(
                "unsupported batch flags or reserved data",
            ));
        }
        Ok(Self {
            body,
            offset: BATCH_HEADER_LEN,
            remaining: u32::from_le_bytes(body[8..12].try_into().unwrap()),
        })
    }

    pub fn message_count(&self) -> u32 {
        self.remaining
    }

    pub fn next_message(&mut self) -> Result<Option<RemoteMessageRef<'a>>, ProtocolError> {
        if self.remaining == 0 {
            if self.offset != self.body.len() {
                return Err(ProtocolError::new("trailing malformed batch bytes"));
            }
            return Ok(None);
        }
        let end_header = self
            .offset
            .checked_add(MESSAGE_HEADER_LEN)
            .ok_or_else(|| ProtocolError::new("message header offset overflow"))?;
        let header = self
            .body
            .get(self.offset..end_header)
            .ok_or_else(|| ProtocolError::new("truncated message frame header"))?;
        let payload_len = u32::from_le_bytes(header[24..28].try_into().unwrap()) as usize;
        let end_payload = end_header
            .checked_add(payload_len)
            .ok_or_else(|| ProtocolError::new("payload length overflow"))?;
        let payload = self
            .body
            .get(end_header..end_payload)
            .ok_or_else(|| ProtocolError::new("truncated message payload"))?;
        self.offset = end_payload;
        self.remaining -= 1;
        Ok(Some(RemoteMessageRef {
            stream_id: u32::from_le_bytes(header[0..4].try_into().unwrap()),
            sequence: u32::from_le_bytes(header[4..8].try_into().unwrap()),
            log_time_ns: u64::from_le_bytes(header[8..16].try_into().unwrap()),
            publish_time_ns: u64::from_le_bytes(header[16..24].try_into().unwrap()),
            payload,
        }))
    }

    pub fn collect(mut self) -> Result<Vec<RemoteMessageRef<'a>>, ProtocolError> {
        let mut messages = Vec::with_capacity(self.remaining as usize);
        while let Some(message) = self.next_message()? {
            messages.push(message);
        }
        Ok(messages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(stream_id: u32, time: u64, payload: &[u8]) -> RemoteMessageRef<'_> {
        RemoteMessageRef {
            stream_id,
            sequence: stream_id + 10,
            log_time_ns: time,
            publish_time_ns: time + 1,
            payload,
        }
    }

    #[test]
    fn empty_single_and_multiple_messages_round_trip_without_payload_copy() {
        let empty = BatchEncoder::new().finish();
        assert!(
            BatchDecoder::new(&empty)
                .unwrap()
                .collect()
                .unwrap()
                .is_empty()
        );

        let mut encoder = BatchEncoder::new();
        encoder.push(message(1, 5, b"abc")).unwrap();
        encoder.push(message(2, 6, b"defg")).unwrap();
        let body = encoder.finish();
        let messages = BatchDecoder::new(&body).unwrap().collect().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0], message(1, 5, b"abc"));
        assert_eq!(messages[1], message(2, 6, b"defg"));
        assert!(std::ptr::eq(
            messages[0].payload.as_ptr(),
            body[44..].as_ptr()
        ));
        assert_eq!(messages[0].payload_range_in(&body), Some(44..47));
    }

    #[test]
    fn rejects_bad_magic_version_truncation_and_count_mismatch() {
        let mut body = BatchEncoder::new().finish().to_vec();
        body[0] = b'X';
        assert!(BatchDecoder::new(&body).is_err());

        let mut body = BatchEncoder::new().finish().to_vec();
        body[4..6].copy_from_slice(&2u16.to_le_bytes());
        assert!(BatchDecoder::new(&body).is_err());
        assert!(BatchDecoder::new(&[0; 15]).is_err());

        let mut body = BatchEncoder::new().finish().to_vec();
        body[8..12].copy_from_slice(&1u32.to_le_bytes());
        assert!(BatchDecoder::new(&body).unwrap().collect().is_err());
    }

    #[test]
    fn rejects_truncated_payload_and_trailing_bytes() {
        let mut encoder = BatchEncoder::new();
        encoder.push(message(1, 1, b"payload")).unwrap();
        let mut truncated = encoder.finish().to_vec();
        truncated.pop();
        assert!(BatchDecoder::new(&truncated).unwrap().collect().is_err());

        let mut trailing = BatchEncoder::new().finish().to_vec();
        trailing.push(1);
        assert!(BatchDecoder::new(&trailing).unwrap().collect().is_err());
    }
}
