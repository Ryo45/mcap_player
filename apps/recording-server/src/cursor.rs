use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContinuationCursor {
    version: u32,
    pub(crate) recording_id: String,
    pub(crate) recording_revision: String,
    pub(crate) start_ns: u64,
    pub(crate) end_ns: u64,
    pub(crate) stream_ids: Vec<u32>,
    pub(crate) next_ordinal: u64,
}

impl ContinuationCursor {
    pub(crate) fn new(
        recording_id: String,
        recording_revision: String,
        start_ns: u64,
        end_ns: u64,
        stream_ids: Vec<u32>,
        next_ordinal: u64,
    ) -> Self {
        Self {
            version: 1,
            recording_id,
            recording_revision,
            start_ns,
            end_ns,
            stream_ids,
            next_ordinal,
        }
    }

    pub(crate) fn encode(&self) -> Result<String, String> {
        let json = serde_json::to_vec(self).map_err(|error| error.to_string())?;
        let mut protected = Vec::with_capacity(json.len() + 4);
        protected.extend_from_slice(&json);
        protected.extend_from_slice(&crc32fast::hash(&json).to_le_bytes());
        Ok(URL_SAFE_NO_PAD.encode(protected))
    }

    pub(crate) fn decode(encoded: &str) -> Result<Self, String> {
        let protected = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| "invalid cursor encoding".to_owned())?;
        let split = protected
            .len()
            .checked_sub(4)
            .ok_or_else(|| "truncated cursor".to_owned())?;
        let (json, checksum) = protected.split_at(split);
        if crc32fast::hash(json).to_le_bytes() != checksum {
            return Err("cursor checksum mismatch".into());
        }
        let cursor: Self =
            serde_json::from_slice(json).map_err(|_| "invalid cursor payload".to_owned())?;
        if cursor.version != 1 {
            return Err("unsupported cursor version".into());
        }
        Ok(cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trips_and_detects_tampering() {
        let cursor = ContinuationCursor::new("demo".into(), "revision".into(), 1, 2, vec![1, 2], 3);
        let encoded = cursor.encode().unwrap();
        assert_eq!(ContinuationCursor::decode(&encoded).unwrap(), cursor);
        let mut bytes = encoded.into_bytes();
        bytes[2] = if bytes[2] == b'A' { b'B' } else { b'A' };
        assert!(ContinuationCursor::decode(std::str::from_utf8(&bytes).unwrap()).is_err());
    }
}
