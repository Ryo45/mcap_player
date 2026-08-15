use crate::{ProtocolError, REMOTE_PROTOCOL_SCHEMA_VERSION};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, str::FromStr};

/// Nanoseconds encoded as a decimal JSON string to avoid JavaScript precision loss.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TimestampNs(u64);

impl TimestampNs {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Recording-observed stream message count encoded as a decimal JSON string.
///
/// Counts use the same precision-safe wire representation as timestamps, but remain a
/// semantically distinct type so consumers cannot accidentally treat them as nanoseconds.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MessageCount(u64);

impl MessageCount {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl Serialize for MessageCount {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for MessageCount {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(serde::de::Error::custom(
                "message count must be an unsigned decimal integer",
            ));
        }
        value
            .parse::<u64>()
            .map(Self)
            .map_err(|_| serde::de::Error::custom("message count exceeds u64"))
    }
}

impl fmt::Display for TimestampNs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for TimestampNs {
    type Err = ProtocolError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(ProtocolError::new(
                "timestamp must be an unsigned decimal integer",
            ));
        }
        value
            .parse::<u64>()
            .map(Self)
            .map_err(|_| ProtocolError::new("timestamp exceeds u64"))
    }
}

impl Serialize for TimestampNs {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for TimestampNs {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteTimeRange {
    pub start_ns: TimestampNs,
    pub end_ns_exclusive: TimestampNs,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StreamSemantic {
    Camera,
    RosMessage,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamDescriptor {
    pub id: u32,
    pub topic: String,
    pub semantic: StreamSemantic,
    pub representation: String,
    pub schema_name: String,
    pub schema_encoding: String,
    pub message_encoding: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_count: Option<MessageCount>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogCapabilities {
    pub continuation: bool,
    pub preview: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogResponse {
    pub schema_version: u32,
    pub recording_id: String,
    pub recording_revision: String,
    pub time_basis: String,
    pub range_semantics: String,
    pub time_range: RemoteTimeRange,
    pub streams: Vec<StreamDescriptor>,
    pub capabilities: CatalogCapabilities,
}

impl CatalogResponse {
    pub fn new(
        recording_id: String,
        recording_revision: String,
        time_range: RemoteTimeRange,
        streams: Vec<StreamDescriptor>,
    ) -> Self {
        Self {
            schema_version: REMOTE_PROTOCOL_SCHEMA_VERSION,
            recording_id,
            recording_revision,
            time_basis: "mcap-log-time".into(),
            range_semantics: "start-inclusive-end-exclusive".into(),
            time_range,
            streams,
            capabilities: CatalogCapabilities {
                continuation: true,
                preview: false,
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingDescriptor {
    pub recording_id: String,
    pub display_name: String,
    pub recording_revision: String,
    pub start_ns: TimestampNs,
    pub end_ns_exclusive: TimestampNs,
    pub stream_count: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingsResponse {
    pub schema_version: u32,
    pub recordings: Vec<RecordingDescriptor>,
}

impl RecordingsResponse {
    pub fn new(recordings: Vec<RecordingDescriptor>) -> Self {
        Self {
            schema_version: REMOTE_PROTOCOL_SCHEMA_VERSION,
            recordings,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_is_a_decimal_json_string_and_rejects_overflow() {
        let timestamp = TimestampNs::new(1_785_591_563_485_080_407);
        let json = serde_json::to_string(&timestamp).unwrap();
        assert_eq!(json, "\"1785591563485080407\"");
        assert_eq!(
            serde_json::from_str::<TimestampNs>(&json).unwrap(),
            timestamp
        );
        assert!(serde_json::from_str::<TimestampNs>("\"18446744073709551616\"").is_err());
        assert!(serde_json::from_str::<TimestampNs>("-1").is_err());
    }

    #[test]
    fn catalog_round_trips() {
        let catalog = CatalogResponse::new(
            "demo".into(),
            "mcap-summary-identity-v1:value".into(),
            RemoteTimeRange {
                start_ns: TimestampNs::new(1),
                end_ns_exclusive: TimestampNs::new(2),
            },
            vec![StreamDescriptor {
                id: 1,
                topic: "/camera".into(),
                semantic: StreamSemantic::Camera,
                representation: "ros2-cdr".into(),
                schema_name: "sensor_msgs/msg/CompressedImage".into(),
                schema_encoding: "ros2msg".into(),
                message_encoding: "cdr".into(),
                message_count: Some(MessageCount::new(42)),
            }],
        );
        let json = serde_json::to_string(&catalog).unwrap();
        assert_eq!(
            serde_json::from_str::<CatalogResponse>(&json).unwrap(),
            catalog
        );
        assert!(json.contains("\"messageCount\":\"42\""));
    }

    #[test]
    fn message_count_is_precision_safe_and_rejects_invalid_values() {
        let count = MessageCount::new(u64::MAX);
        assert_eq!(
            serde_json::to_string(&count).unwrap(),
            format!("\"{}\"", u64::MAX)
        );
        assert_eq!(
            serde_json::from_str::<MessageCount>(&format!("\"{}\"", u64::MAX))
                .unwrap()
                .get(),
            u64::MAX
        );
        assert!(serde_json::from_str::<MessageCount>("\"-1\"").is_err());
        assert!(serde_json::from_str::<MessageCount>("\"18446744073709551616\"").is_err());
    }
}
