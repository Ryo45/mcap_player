use bytes::Bytes;
use std::{error::Error, fmt};
use viewer_remote_protocol::{BATCH_CONTENT_TYPE, BatchDecoder, REMOTE_PROTOCOL_SCHEMA_VERSION};

#[cfg(target_arch = "wasm32")]
use {
    js_sys::Uint8Array,
    serde::de::DeserializeOwned,
    viewer_remote_protocol::{
        BATCH_COMPLETE_HEADER, CatalogResponse, MESSAGE_COUNT_HEADER, NEXT_CURSOR_HEADER,
        RECORDING_REVISION_HEADER, RecordingsResponse,
    },
    wasm_bindgen::JsCast,
    wasm_bindgen_futures::JsFuture,
    web_sys::{AbortSignal, Request, RequestInit, RequestMode, Response, Url},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RemoteBatchRequest {
    pub recording_id: String,
    pub revision: String,
    pub stream_ids: Vec<u32>,
    pub start_ns: u64,
    pub end_ns: u64,
    pub max_bytes: Option<usize>,
    pub max_messages: Option<usize>,
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RemoteRestoreRequest {
    pub recording_id: String,
    pub revision: String,
    pub target_ns: u64,
    pub latest_streams: Vec<u32>,
    pub history_streams: Vec<u32>,
    pub history_start_ns: Option<u64>,
    pub persistent_streams: Vec<u32>,
}

impl RemoteRestoreRequest {
    fn query_pairs(&self) -> Vec<(&'static str, String)> {
        let streams = |values: &[u32]| {
            values
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",")
        };
        let mut pairs = vec![
            ("revision", self.revision.clone()),
            ("target_ns", self.target_ns.to_string()),
        ];
        if !self.latest_streams.is_empty() {
            pairs.push(("latest_streams", streams(&self.latest_streams)));
        }
        if !self.history_streams.is_empty() {
            pairs.push(("history_streams", streams(&self.history_streams)));
        }
        if let Some(start) = self.history_start_ns {
            pairs.push(("history_start_ns", start.to_string()));
        }
        if !self.persistent_streams.is_empty() {
            pairs.push(("persistent_streams", streams(&self.persistent_streams)));
        }
        pairs
    }
}

impl RemoteBatchRequest {
    fn query_pairs(&self) -> Vec<(&'static str, String)> {
        let mut pairs = vec![
            ("revision", self.revision.clone()),
            (
                "streams",
                self.stream_ids
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
            ),
            ("start_ns", self.start_ns.to_string()),
            ("end_ns", self.end_ns.to_string()),
        ];
        if let Some(max_bytes) = self.max_bytes {
            pairs.push(("max_bytes", max_bytes.to_string()));
        }
        if let Some(max_messages) = self.max_messages {
            pairs.push(("max_messages", max_messages.to_string()));
        }
        if let Some(cursor) = &self.cursor {
            pairs.push(("cursor", cursor.clone()));
        }
        pairs
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RemoteBatchPage {
    pub body: Bytes,
    pub complete: bool,
    pub next_cursor: Option<String>,
    pub message_count: usize,
    pub recording_revision: String,
}

#[derive(Clone, Debug)]
pub(crate) enum RemoteClientError {
    EmptyUrl,
    #[cfg(target_arch = "wasm32")]
    NotText,
    MissingHeader(&'static str),
    UnexpectedContentType(String),
    RevisionMismatch {
        expected: String,
        actual: String,
    },
    InvalidCompleteHeader(Option<String>),
    UnexpectedCursor,
    MissingCursor,
    InvalidMessageCount(std::sync::Arc<std::num::ParseIntError>),
    MessageCountMismatch {
        header: usize,
        actual: usize,
    },
    #[cfg(target_arch = "wasm32")]
    IncompleteRestore,
    UnsupportedSchemaVersion {
        actual: u32,
        expected: u32,
    },
    Protocol(viewer_remote_protocol::ProtocolError),
    Json(std::sync::Arc<serde_json::Error>),
    #[cfg(target_arch = "wasm32")]
    Http {
        status: u16,
        code: Option<String>,
        message: String,
    },
    #[cfg(target_arch = "wasm32")]
    Browser(std::rc::Rc<wasm_bindgen::JsValue>),
    #[cfg(target_arch = "wasm32")]
    MissingBrowserWindow,
}
impl fmt::Display for RemoteClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyUrl => write!(f, "remote server URL is empty"),
            #[cfg(target_arch = "wasm32")]
            Self::NotText => write!(f, "HTTP response was not text"),
            Self::MissingHeader(name) => write!(f, "batch response has no {name}"),
            Self::UnexpectedContentType(value) => {
                write!(f, "unexpected batch Content-Type: {value}")
            }
            Self::RevisionMismatch { expected, actual } => write!(
                f,
                "batch recording revision mismatch: expected {expected}, got {actual}"
            ),
            Self::InvalidCompleteHeader(value) => {
                write!(f, "invalid batch complete header: {value:?}")
            }
            Self::UnexpectedCursor => write!(
                f,
                "complete batch unexpectedly contains a continuation cursor"
            ),
            Self::MissingCursor => write!(f, "incomplete batch has no continuation cursor"),
            Self::InvalidMessageCount(error) => {
                write!(f, "invalid batch message count header: {error}")
            }
            Self::MessageCountMismatch { header, actual } => write!(
                f,
                "batch body and message count header disagree: header {header}, body {actual}"
            ),
            #[cfg(target_arch = "wasm32")]
            Self::IncompleteRestore => write!(f, "restore response must be complete"),
            Self::UnsupportedSchemaVersion { actual, expected } => write!(
                f,
                "unsupported remote schema version: {actual}, expected {expected}"
            ),
            Self::Protocol(error) => error.fmt(f),
            Self::Json(error) => write!(f, "invalid JSON response: {error}"),
            #[cfg(target_arch = "wasm32")]
            Self::Http {
                status,
                code,
                message,
            } => {
                write!(f, "remote HTTP {status}: ")?;
                if let Some(code) = code {
                    write!(f, "{code}: ")?;
                }
                f.write_str(message)
            }
            #[cfg(target_arch = "wasm32")]
            Self::Browser(value) => write!(f, "browser API error: {value:?}"),
            #[cfg(target_arch = "wasm32")]
            Self::MissingBrowserWindow => write!(f, "window is unavailable"),
        }
    }
}
impl Error for RemoteClientError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Protocol(error) => Some(error),
            Self::Json(error) => Some(error.as_ref()),
            Self::InvalidMessageCount(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}
impl From<viewer_remote_protocol::ProtocolError> for RemoteClientError {
    fn from(error: viewer_remote_protocol::ProtocolError) -> Self {
        Self::Protocol(error)
    }
}
impl From<serde_json::Error> for RemoteClientError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(std::sync::Arc::new(error))
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RequestGeneration(u64);

impl RequestGeneration {
    pub(crate) fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(1);
        self.0
    }

    pub(crate) fn is_current(self, generation: u64) -> bool {
        self.0 == generation
    }
}

#[derive(Clone, Debug)]
pub(crate) struct RemoteApiClient {
    base_url: String,
}

impl RemoteApiClient {
    pub(crate) fn new(base_url: impl Into<String>) -> Result<Self, RemoteClientError> {
        let base_url = base_url.into().trim_end_matches('/').to_owned();
        if base_url.is_empty() {
            return Err(RemoteClientError::EmptyUrl);
        }
        Ok(Self { base_url })
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) async fn list_recordings(
        &self,
        abort: Option<&AbortSignal>,
    ) -> Result<RecordingsResponse, RemoteClientError> {
        let response: RecordingsResponse = self
            .fetch_json(&format!("{}/v1/recordings", self.base_url), abort)
            .await?;
        validate_schema(response.schema_version)?;
        Ok(response)
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) async fn fetch_catalog(
        &self,
        recording_id: &str,
        abort: Option<&AbortSignal>,
    ) -> Result<CatalogResponse, RemoteClientError> {
        let recording_id = encode_path_segment(recording_id);
        let response: CatalogResponse = self
            .fetch_json(
                &format!("{}/v1/recordings/{recording_id}/catalog", self.base_url),
                abort,
            )
            .await?;
        validate_schema(response.schema_version)?;
        Ok(response)
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) async fn fetch_batch_page(
        &self,
        request: &RemoteBatchRequest,
        abort: Option<&AbortSignal>,
    ) -> Result<RemoteBatchPage, RemoteClientError> {
        let recording_id = encode_path_segment(&request.recording_id);
        let url = Url::new(&format!(
            "{}/v1/recordings/{recording_id}/messages",
            self.base_url
        ))
        .map_err(js_error)?;
        let search = url.search_params();
        for (name, value) in request.query_pairs() {
            search.set(name, &value);
        }
        let response = fetch(&url.href(), abort).await?;
        ensure_success(&response).await?;
        let headers = response.headers();
        let metadata = BatchResponseMetadata {
            content_type: headers.get("content-type").map_err(js_error)?,
            recording_revision: headers.get(RECORDING_REVISION_HEADER).map_err(js_error)?,
            complete: headers.get(BATCH_COMPLETE_HEADER).map_err(js_error)?,
            next_cursor: headers.get(NEXT_CURSOR_HEADER).map_err(js_error)?,
            message_count: headers.get(MESSAGE_COUNT_HEADER).map_err(js_error)?,
        };
        let buffer = JsFuture::from(response.array_buffer().map_err(js_error)?)
            .await
            .map_err(js_error)?;
        let body = Bytes::from(Uint8Array::new(&buffer).to_vec());
        validate_batch_response(metadata, body, &request.revision)
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) async fn fetch_restore(
        &self,
        request: &RemoteRestoreRequest,
        abort: Option<&AbortSignal>,
    ) -> Result<RemoteBatchPage, RemoteClientError> {
        let recording_id = encode_path_segment(&request.recording_id);
        let url = Url::new(&format!(
            "{}/v1/recordings/{recording_id}/restore",
            self.base_url
        ))
        .map_err(js_error)?;
        let search = url.search_params();
        for (name, value) in request.query_pairs() {
            search.set(name, &value);
        }
        let response = fetch(&url.href(), abort).await?;
        ensure_success(&response).await?;
        let headers = response.headers();
        let metadata = BatchResponseMetadata {
            content_type: headers.get("content-type").map_err(js_error)?,
            recording_revision: headers.get(RECORDING_REVISION_HEADER).map_err(js_error)?,
            complete: headers.get(BATCH_COMPLETE_HEADER).map_err(js_error)?,
            next_cursor: headers.get(NEXT_CURSOR_HEADER).map_err(js_error)?,
            message_count: headers.get(MESSAGE_COUNT_HEADER).map_err(js_error)?,
        };
        let buffer = JsFuture::from(response.array_buffer().map_err(js_error)?)
            .await
            .map_err(js_error)?;
        let body = Bytes::from(Uint8Array::new(&buffer).to_vec());
        let page = validate_batch_response(metadata, body, &request.revision)?;
        if !page.complete {
            return Err(RemoteClientError::IncompleteRestore);
        }
        Ok(page)
    }

    #[cfg(target_arch = "wasm32")]
    async fn fetch_json<T: DeserializeOwned>(
        &self,
        url: &str,
        abort: Option<&AbortSignal>,
    ) -> Result<T, RemoteClientError> {
        let response = fetch(url, abort).await?;
        ensure_success(&response).await?;
        let text = JsFuture::from(response.text().map_err(js_error)?)
            .await
            .map_err(js_error)?
            .as_string()
            .ok_or_else(|| RemoteClientError::NotText)?;
        serde_json::from_str(&text).map_err(RemoteClientError::from)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct BatchResponseMetadata {
    content_type: Option<String>,
    recording_revision: Option<String>,
    complete: Option<String>,
    next_cursor: Option<String>,
    message_count: Option<String>,
}

fn validate_batch_response(
    metadata: BatchResponseMetadata,
    body: Bytes,
    expected_revision: &str,
) -> Result<RemoteBatchPage, RemoteClientError> {
    let content_type = metadata
        .content_type
        .ok_or(RemoteClientError::MissingHeader("Content-Type"))?;
    if content_type != BATCH_CONTENT_TYPE {
        return Err(RemoteClientError::UnexpectedContentType(content_type));
    }
    let recording_revision = metadata
        .recording_revision
        .ok_or(RemoteClientError::MissingHeader("recording revision"))?;
    if recording_revision != expected_revision {
        return Err(RemoteClientError::RevisionMismatch {
            expected: expected_revision.into(),
            actual: recording_revision.clone(),
        });
    }
    let complete = match metadata.complete.as_deref() {
        Some("true") => true,
        Some("false") => false,
        _ => {
            return Err(RemoteClientError::InvalidCompleteHeader(
                metadata.complete.clone(),
            ));
        }
    };
    let next_cursor = metadata.next_cursor.filter(|cursor| !cursor.is_empty());
    if complete && next_cursor.is_some() {
        return Err(RemoteClientError::UnexpectedCursor);
    }
    if !complete && next_cursor.is_none() {
        return Err(RemoteClientError::MissingCursor);
    }
    let message_count = metadata
        .message_count
        .ok_or(RemoteClientError::MissingHeader("message count"))?
        .parse::<usize>()
        .map_err(|error| RemoteClientError::InvalidMessageCount(std::sync::Arc::new(error)))?;
    let decoded = BatchDecoder::new(&body)
        .and_then(BatchDecoder::collect)
        .map_err(RemoteClientError::from)?;
    if decoded.len() != message_count {
        return Err(RemoteClientError::MessageCountMismatch {
            header: message_count,
            actual: decoded.len(),
        });
    }
    Ok(RemoteBatchPage {
        body,
        complete,
        next_cursor,
        message_count,
        recording_revision,
    })
}

#[cfg(any(test, target_arch = "wasm32"))]
fn validate_schema(schema_version: u32) -> Result<(), RemoteClientError> {
    if schema_version != REMOTE_PROTOCOL_SCHEMA_VERSION {
        return Err(RemoteClientError::UnsupportedSchemaVersion {
            actual: schema_version,
            expected: REMOTE_PROTOCOL_SCHEMA_VERSION,
        });
    }
    Ok(())
}

fn encode_path_segment(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    encoded
}

#[cfg(target_arch = "wasm32")]
async fn fetch(url: &str, abort: Option<&AbortSignal>) -> Result<Response, RemoteClientError> {
    let init = RequestInit::new();
    init.set_method("GET");
    init.set_mode(RequestMode::Cors);
    init.set_signal(abort);
    let request = Request::new_with_str_and_init(url, &init).map_err(js_error)?;
    JsFuture::from(
        web_sys::window()
            .ok_or_else(|| RemoteClientError::MissingBrowserWindow)?
            .fetch_with_request(&request),
    )
    .await
    .map_err(js_error)?
    .dyn_into::<Response>()
    .map_err(js_error)
}

#[cfg(target_arch = "wasm32")]
async fn ensure_success(response: &Response) -> Result<(), RemoteClientError> {
    if response.ok() {
        return Ok(());
    }
    let status = response.status();
    let body = JsFuture::from(response.text().map_err(js_error)?)
        .await
        .map_err(js_error)?
        .as_string()
        .unwrap_or_default();
    let (code, message) =
        match serde_json::from_str::<viewer_remote_protocol::RemoteErrorResponse>(&body) {
            Ok(error) => (Some(error.code), error.message),
            Err(_) => (None, body),
        };
    Err(RemoteClientError::Http {
        status,
        code,
        message,
    })
}

#[cfg(target_arch = "wasm32")]
fn js_error(value: wasm_bindgen::JsValue) -> RemoteClientError {
    RemoteClientError::Browser(std::rc::Rc::new(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use viewer_remote_protocol::{BatchEncoder, RemoteMessageRef};

    fn valid_metadata(complete: bool) -> BatchResponseMetadata {
        BatchResponseMetadata {
            content_type: Some(BATCH_CONTENT_TYPE.to_owned()),
            recording_revision: Some("revision".to_owned()),
            complete: Some(complete.to_string()),
            next_cursor: (!complete).then(|| "cursor".to_owned()),
            message_count: Some("1".to_owned()),
        }
    }

    fn one_message_body() -> Bytes {
        let mut encoder = BatchEncoder::new();
        encoder
            .push(RemoteMessageRef {
                stream_id: 1,
                sequence: 2,
                log_time_ns: 18_446_744_073_709_551_000,
                publish_time_ns: 3,
                payload: b"cdr",
            })
            .unwrap();
        encoder.finish()
    }

    #[test]
    fn batch_query_preserves_nanoseconds_as_decimal_strings() {
        let request = RemoteBatchRequest {
            recording_id: "run one".into(),
            revision: "revision:value".into(),
            stream_ids: vec![1, 3],
            start_ns: 18_446_744_073_709_551_000,
            end_ns: u64::MAX,
            max_bytes: Some(1024),
            max_messages: Some(10),
            cursor: Some("next/page".into()),
        };
        let pairs = request.query_pairs();
        assert!(pairs.contains(&("start_ns", "18446744073709551000".into())));
        assert!(pairs.contains(&("end_ns", "18446744073709551615".into())));
        assert_eq!(encode_path_segment(&request.recording_id), "run%20one");
    }

    #[test]
    fn restore_query_preserves_exact_timestamp_and_closed_stream_sets() {
        let request = RemoteRestoreRequest {
            recording_id: "demo".into(),
            revision: "revision".into(),
            target_ns: u64::MAX,
            latest_streams: vec![1, 2],
            history_streams: vec![3],
            history_start_ns: Some(18_446_744_073_000_000_000),
            persistent_streams: vec![4],
        };
        let pairs = request.query_pairs();
        assert!(pairs.contains(&("target_ns", "18446744073709551615".into())));
        assert!(pairs.contains(&("latest_streams", "1,2".into())));
        assert!(pairs.contains(&("history_streams", "3".into())));
        assert!(pairs.contains(&("persistent_streams", "4".into())));
    }

    #[test]
    fn validates_continuation_revision_and_message_count_headers() {
        let page =
            validate_batch_response(valid_metadata(false), one_message_body(), "revision").unwrap();
        assert!(!page.complete);
        assert_eq!(page.next_cursor.as_deref(), Some("cursor"));
        assert_eq!(page.message_count, 1);

        let mut missing_cursor = valid_metadata(false);
        missing_cursor.next_cursor = None;
        assert!(validate_batch_response(missing_cursor, one_message_body(), "revision").is_err());

        assert!(
            validate_batch_response(valid_metadata(true), one_message_body(), "stale").is_err()
        );

        let mut wrong_count = valid_metadata(true);
        wrong_count.message_count = Some("2".into());
        assert!(validate_batch_response(wrong_count, one_message_body(), "revision").is_err());
    }

    #[test]
    fn request_generation_rejects_stale_results() {
        let mut generation = RequestGeneration::default();
        let first = generation.next();
        let second = generation.next();
        assert!(!generation.is_current(first));
        assert!(generation.is_current(second));
    }

    #[test]
    fn client_normalizes_base_url_and_rejects_unknown_schema() {
        let client = RemoteApiClient::new("http://localhost:8081/").unwrap();
        assert_eq!(client.base_url, "http://localhost:8081");
        assert!(RemoteApiClient::new("").is_err());
        assert!(validate_schema(REMOTE_PROTOCOL_SCHEMA_VERSION).is_ok());
        assert!(validate_schema(REMOTE_PROTOCOL_SCHEMA_VERSION + 1).is_err());
    }
}
