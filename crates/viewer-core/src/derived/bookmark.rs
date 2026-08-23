//! Persistent user bookmarks and their source identity.

use crate::ArrivalTime;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, error::Error, fmt};

pub const CURRENT_BOOKMARK_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BookmarkValidationError {
    message: String,
}

impl BookmarkValidationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for BookmarkValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for BookmarkValidationError {}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct BookmarkId(String);

impl BookmarkId {
    pub fn new(value: impl Into<String>) -> Result<Self, BookmarkValidationError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(BookmarkValidationError::new(
                "bookmark id must not be empty or whitespace",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for BookmarkId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for BookmarkId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", try_from = "BookmarkWire")]
pub struct Bookmark {
    id: BookmarkId,
    time: ArrivalTime,
    end_time: Option<ArrivalTime>,
    label: String,
    note: Option<String>,
}

impl Bookmark {
    pub fn new(
        id: BookmarkId,
        time: ArrivalTime,
        end_time: Option<ArrivalTime>,
        label: impl Into<String>,
        note: Option<String>,
    ) -> Result<Self, BookmarkValidationError> {
        let bookmark = Self {
            id,
            time,
            end_time,
            label: label.into(),
            note,
        };
        bookmark.validate()?;
        Ok(bookmark)
    }

    pub fn point(
        id: BookmarkId,
        time: ArrivalTime,
        label: impl Into<String>,
        note: Option<String>,
    ) -> Result<Self, BookmarkValidationError> {
        Self::new(id, time, None, label, note)
    }

    pub fn interval(
        id: BookmarkId,
        time: ArrivalTime,
        end_time: ArrivalTime,
        label: impl Into<String>,
        note: Option<String>,
    ) -> Result<Self, BookmarkValidationError> {
        Self::new(id, time, Some(end_time), label, note)
    }

    pub fn id(&self) -> &BookmarkId {
        &self.id
    }

    pub fn time(&self) -> ArrivalTime {
        self.time
    }

    pub fn end_time(&self) -> Option<ArrivalTime> {
        self.end_time
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    pub fn validate(&self) -> Result<(), BookmarkValidationError> {
        if self.id.as_str().trim().is_empty() {
            return Err(BookmarkValidationError::new(
                "bookmark id must not be empty or whitespace",
            ));
        }
        if self.label.trim().is_empty() {
            return Err(BookmarkValidationError::new(
                "bookmark label must not be empty or whitespace",
            ));
        }
        if self.end_time.is_some_and(|end| end < self.time) {
            return Err(BookmarkValidationError::new(
                "bookmark endTime must not precede time",
            ));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BookmarkWire {
    id: BookmarkId,
    time: ArrivalTime,
    #[serde(default)]
    end_time: Option<ArrivalTime>,
    label: String,
    #[serde(default)]
    note: Option<String>,
}

impl TryFrom<BookmarkWire> for Bookmark {
    type Error = BookmarkValidationError;

    fn try_from(value: BookmarkWire) -> Result<Self, Self::Error> {
        Self::new(
            value.id,
            value.time,
            value.end_time,
            value.label,
            value.note,
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", try_from = "SourceFingerprintWire")]
pub struct SourceFingerprint {
    algorithm: String,
    value: String,
}

impl SourceFingerprint {
    pub fn new(
        algorithm: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<Self, BookmarkValidationError> {
        let fingerprint = Self {
            algorithm: algorithm.into(),
            value: value.into(),
        };
        fingerprint.validate()?;
        Ok(fingerprint)
    }

    pub fn algorithm(&self) -> &str {
        &self.algorithm
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn validate(&self) -> Result<(), BookmarkValidationError> {
        if self.algorithm.trim().is_empty() {
            return Err(BookmarkValidationError::new(
                "source fingerprint algorithm must not be empty",
            ));
        }
        if self.value.trim().is_empty() {
            return Err(BookmarkValidationError::new(
                "source fingerprint value must not be empty",
            ));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourceFingerprintWire {
    algorithm: String,
    value: String,
}

impl TryFrom<SourceFingerprintWire> for SourceFingerprint {
    type Error = BookmarkValidationError;

    fn try_from(value: SourceFingerprintWire) -> Result<Self, Self::Error> {
        Self::new(value.algorithm, value.value)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", try_from = "BookmarkDocumentWire")]
pub struct BookmarkDocument {
    schema_version: u32,
    source: SourceFingerprint,
    bookmarks: Vec<Bookmark>,
}

impl BookmarkDocument {
    pub fn new(
        source: SourceFingerprint,
        bookmarks: Vec<Bookmark>,
    ) -> Result<Self, BookmarkValidationError> {
        let document = Self {
            schema_version: CURRENT_BOOKMARK_SCHEMA_VERSION,
            source,
            bookmarks,
        };
        document.validate()?;
        Ok(document)
    }

    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub fn source(&self) -> &SourceFingerprint {
        &self.source
    }

    pub fn bookmarks(&self) -> &[Bookmark] {
        &self.bookmarks
    }

    pub fn validate(&self) -> Result<(), BookmarkValidationError> {
        if self.schema_version != CURRENT_BOOKMARK_SCHEMA_VERSION {
            return Err(BookmarkValidationError::new(format!(
                "unsupported bookmark schema version: {}",
                self.schema_version
            )));
        }
        self.source.validate()?;
        let mut ids = BTreeSet::new();
        for bookmark in &self.bookmarks {
            bookmark.validate()?;
            if !ids.insert(bookmark.id.clone()) {
                return Err(BookmarkValidationError::new(format!(
                    "duplicate bookmark id: {}",
                    bookmark.id
                )));
            }
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BookmarkDocumentWire {
    schema_version: u32,
    source: SourceFingerprint,
    #[serde(default)]
    bookmarks: Vec<Bookmark>,
}

impl TryFrom<BookmarkDocumentWire> for BookmarkDocument {
    type Error = BookmarkValidationError;

    fn try_from(value: BookmarkDocumentWire) -> Result<Self, Self::Error> {
        let document = Self {
            schema_version: value.schema_version,
            source: value.source,
            bookmarks: value.bookmarks,
        };
        document.validate()?;
        Ok(document)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", try_from = "PreviewBuildInfoWire")]
pub struct PreviewBuildInfo {
    schema_version: u32,
    generator_name: String,
    generator_version: String,
    source: SourceFingerprint,
}

impl PreviewBuildInfo {
    pub fn new(
        generator_name: impl Into<String>,
        generator_version: impl Into<String>,
        source: SourceFingerprint,
    ) -> Result<Self, BookmarkValidationError> {
        let info = Self {
            schema_version: crate::CURRENT_PREVIEW_SCHEMA_VERSION,
            generator_name: generator_name.into(),
            generator_version: generator_version.into(),
            source,
        };
        info.validate()?;
        Ok(info)
    }

    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub fn generator_name(&self) -> &str {
        &self.generator_name
    }

    pub fn generator_version(&self) -> &str {
        &self.generator_version
    }

    pub fn source(&self) -> &SourceFingerprint {
        &self.source
    }

    pub fn validate(&self) -> Result<(), BookmarkValidationError> {
        if self.schema_version != crate::CURRENT_PREVIEW_SCHEMA_VERSION {
            return Err(BookmarkValidationError::new(format!(
                "unsupported preview schema version: {}",
                self.schema_version
            )));
        }
        if self.generator_name.trim().is_empty() {
            return Err(BookmarkValidationError::new(
                "preview generatorName must not be empty",
            ));
        }
        if self.generator_version.trim().is_empty() {
            return Err(BookmarkValidationError::new(
                "preview generatorVersion must not be empty",
            ));
        }
        self.source.validate()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreviewBuildInfoWire {
    schema_version: u32,
    generator_name: String,
    generator_version: String,
    source: SourceFingerprint,
}

impl TryFrom<PreviewBuildInfoWire> for PreviewBuildInfo {
    type Error = BookmarkValidationError;

    fn try_from(value: PreviewBuildInfoWire) -> Result<Self, Self::Error> {
        let info = Self {
            schema_version: value.schema_version,
            generator_name: value.generator_name,
            generator_version: value.generator_version,
            source: value.source,
        };
        info.validate()?;
        Ok(info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> SourceFingerprint {
        SourceFingerprint::new("sha256", "0123456789abcdef").unwrap()
    }

    #[test]
    fn point_bookmark_json_round_trips_with_integer_time() {
        let document = BookmarkDocument::new(
            source(),
            vec![
                Bookmark::point(
                    BookmarkId::new("point-1").unwrap(),
                    ArrivalTime(1_234_567_890),
                    "Obstacle",
                    Some("inspect camera".to_owned()),
                )
                .unwrap(),
            ],
        )
        .unwrap();
        let json = document.to_json_pretty().unwrap();
        assert!(json.contains("\"time\": 1234567890"));
        assert_eq!(BookmarkDocument::from_json(&json).unwrap(), document);
    }

    #[test]
    fn interval_bookmark_json_round_trips() {
        let document = BookmarkDocument::new(
            source(),
            vec![
                Bookmark::interval(
                    BookmarkId::new("interval-1").unwrap(),
                    ArrivalTime(10),
                    ArrivalTime(20),
                    "Turn",
                    None,
                )
                .unwrap(),
            ],
        )
        .unwrap();
        let json = serde_json::to_string(&document).unwrap();
        assert!(json.contains("\"endTime\":20"));
        assert_eq!(
            serde_json::from_str::<BookmarkDocument>(&json).unwrap(),
            document
        );
    }

    #[test]
    fn rejects_invalid_interval_label_and_id() {
        assert!(BookmarkId::new("  ").is_err());
        assert!(
            Bookmark::point(
                BookmarkId::new("empty-label").unwrap(),
                ArrivalTime(10),
                "  ",
                None,
            )
            .is_err()
        );
        assert!(
            Bookmark::interval(
                BookmarkId::new("backwards").unwrap(),
                ArrivalTime(20),
                ArrivalTime(10),
                "Backwards",
                None,
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_duplicate_bookmark_ids() {
        let make = || {
            Bookmark::point(
                BookmarkId::new("duplicate").unwrap(),
                ArrivalTime(10),
                "Same",
                None,
            )
            .unwrap()
        };
        assert!(BookmarkDocument::new(source(), vec![make(), make()]).is_err());
    }

    #[test]
    fn deserialization_rejects_unknown_schema_versions() {
        let bookmark = r#"{
            "schemaVersion": 2,
            "source": {"algorithm":"sha256","value":"abc"},
            "bookmarks": []
        }"#;
        assert!(serde_json::from_str::<BookmarkDocument>(bookmark).is_err());

        let preview = r#"{
            "schemaVersion": 2,
            "generatorName": "viewer",
            "generatorVersion": "0.1",
            "source": {"algorithm":"sha256","value":"abc"}
        }"#;
        assert!(serde_json::from_str::<PreviewBuildInfo>(preview).is_err());
    }

    #[test]
    fn source_fingerprint_and_build_info_round_trip() {
        let fingerprint = source();
        let json = serde_json::to_string(&fingerprint).unwrap();
        assert_eq!(
            serde_json::from_str::<SourceFingerprint>(&json).unwrap(),
            fingerprint
        );
        assert!(SourceFingerprint::new("", "abc").is_err());
        assert!(SourceFingerprint::new("sha256", " ").is_err());

        let info = PreviewBuildInfo::new("mcap-viewer", "0.1.0", fingerprint).unwrap();
        let json = serde_json::to_string(&info).unwrap();
        assert_eq!(
            serde_json::from_str::<PreviewBuildInfo>(&json).unwrap(),
            info
        );
    }

    #[test]
    fn unknown_fields_are_accepted() {
        let json = r#"{
            "schemaVersion": 1,
            "source": {"algorithm":"sha256","value":"abc","future":true},
            "bookmarks": [],
            "futureField": {"anything": 1}
        }"#;
        assert!(serde_json::from_str::<BookmarkDocument>(json).is_ok());
    }
}
