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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RemoteClientError {
    message: String,
}

impl RemoteClientError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for RemoteClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for RemoteClientError {}

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
            return Err(RemoteClientError::new("remote server URL is empty"));
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
            .ok_or_else(|| RemoteClientError::new("HTTP response was not text"))?;
        serde_json::from_str(&text)
            .map_err(|error| RemoteClientError::new(format!("invalid JSON response: {error}")))
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
        .ok_or_else(|| RemoteClientError::new("batch response has no Content-Type"))?;
    if content_type != BATCH_CONTENT_TYPE {
        return Err(RemoteClientError::new(format!(
            "unexpected batch Content-Type: {content_type}"
        )));
    }
    let recording_revision = metadata
        .recording_revision
        .ok_or_else(|| RemoteClientError::new("batch response has no recording revision"))?;
    if recording_revision != expected_revision {
        return Err(RemoteClientError::new("batch recording revision mismatch"));
    }
    let complete = match metadata.complete.as_deref() {
        Some("true") => true,
        Some("false") => false,
        _ => return Err(RemoteClientError::new("invalid batch complete header")),
    };
    let next_cursor = metadata.next_cursor.filter(|cursor| !cursor.is_empty());
    if complete && next_cursor.is_some() {
        return Err(RemoteClientError::new(
            "complete batch unexpectedly contains a continuation cursor",
        ));
    }
    if !complete && next_cursor.is_none() {
        return Err(RemoteClientError::new(
            "incomplete batch has no continuation cursor",
        ));
    }
    let message_count = metadata
        .message_count
        .ok_or_else(|| RemoteClientError::new("batch response has no message count"))?
        .parse::<usize>()
        .map_err(|_| RemoteClientError::new("invalid batch message count header"))?;
    let decoded = BatchDecoder::new(&body)
        .and_then(BatchDecoder::collect)
        .map_err(|error| RemoteClientError::new(error.to_string()))?;
    if decoded.len() != message_count {
        return Err(RemoteClientError::new(
            "batch body and message count header disagree",
        ));
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
        return Err(RemoteClientError::new(format!(
            "unsupported remote schema version: {schema_version}"
        )));
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
            .ok_or_else(|| RemoteClientError::new("window is unavailable"))?
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
    let detail = serde_json::from_str::<viewer_remote_protocol::RemoteErrorResponse>(&body)
        .map(|error| format!("{}: {}", error.code, error.message))
        .unwrap_or_else(|_| body);
    Err(RemoteClientError::new(format!(
        "remote HTTP {status}: {detail}"
    )))
}

#[cfg(target_arch = "wasm32")]
fn js_error(value: wasm_bindgen::JsValue) -> RemoteClientError {
    RemoteClientError::new(format!("browser API error: {value:?}"))
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
