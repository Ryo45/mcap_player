use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use axum::{
    Json, Router,
    body::Body,
    extract::rejection::QueryRejection,
    extract::{Path, Query, State},
    http::{HeaderName, HeaderValue, Method, StatusCode, header::CONTENT_TYPE},
    response::Response,
    routing::get,
};
use serde::Deserialize;
use tokio::sync::Semaphore;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use viewer_remote_protocol::{
    BATCH_COMPLETE_HEADER, BATCH_CONTENT_TYPE, MESSAGE_COUNT_HEADER, NEXT_CURSOR_HEADER,
    RECORDING_REVISION_HEADER, RecordingsResponse,
};

use crate::{
    batch_service::{BatchRequest, read_batch},
    config::{Limits, ServerConfig},
    error::ServerError,
    recording::Recording,
};

#[derive(Clone)]
pub(crate) struct AppState {
    recordings: Arc<BTreeMap<String, Arc<Recording>>>,
    limits: Limits,
    blocking_requests: Arc<Semaphore>,
    request_sequence: Arc<AtomicU64>,
}

impl AppState {
    pub(crate) fn initialize(config: &ServerConfig) -> Result<Self, ServerError> {
        let recordings = config
            .recordings
            .iter()
            .map(|entry| {
                Recording::open(entry, &config.limits).map(|item| (entry.id.clone(), item))
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            recordings: Arc::new(recordings),
            limits: config.limits.clone(),
            blocking_requests: Arc::new(Semaphore::new(config.max_in_flight_requests)),
            request_sequence: Arc::new(AtomicU64::new(1)),
        })
    }
}

pub(crate) fn router(config: &ServerConfig, state: AppState) -> Result<Router, String> {
    let origins = config
        .allowed_origins
        .iter()
        .map(|origin| {
            origin
                .parse::<HeaderValue>()
                .map_err(|_| format!("invalid CORS origin header: {origin}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let exposed = [
        HeaderName::from_static(RECORDING_REVISION_HEADER),
        HeaderName::from_static(BATCH_COMPLETE_HEADER),
        HeaderName::from_static(NEXT_CURSOR_HEADER),
        HeaderName::from_static(MESSAGE_COUNT_HEADER),
    ];
    let cors = CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([Method::GET, Method::OPTIONS])
        .expose_headers(exposed);
    Ok(Router::new()
        .route("/healthz", get(health))
        .route("/readyz", get(ready))
        .route("/v1/recordings", get(list_recordings))
        .route("/v1/recordings/{recording_id}/catalog", get(catalog))
        .route("/v1/recordings/{recording_id}/messages", get(messages))
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .with_state(state))
}

async fn health() -> &'static str {
    "ok"
}

async fn ready() -> &'static str {
    "ready"
}

async fn list_recordings(State(state): State<AppState>) -> Json<RecordingsResponse> {
    Json(RecordingsResponse::new(
        state
            .recordings
            .values()
            .map(|recording| recording.descriptor())
            .collect(),
    ))
}

async fn catalog(
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
) -> Result<Json<viewer_remote_protocol::CatalogResponse>, ServerError> {
    let recording = find_recording(&state, &recording_id)?;
    Ok(Json(recording.catalog.clone()))
}

#[derive(Debug, Deserialize)]
struct MessageQuery {
    revision: String,
    streams: String,
    start_ns: String,
    end_ns: Option<String>,
    max_bytes: Option<usize>,
    max_messages: Option<usize>,
    cursor: Option<String>,
}

async fn messages(
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
    query: Result<Query<MessageQuery>, QueryRejection>,
) -> Result<Response, ServerError> {
    let Query(query) = query.map_err(|_| {
        ServerError::bad_request("invalid_query", "message query is missing or malformed")
    })?;
    let request_id = state.request_sequence.fetch_add(1, Ordering::Relaxed);
    let recording = find_recording(&state, &recording_id)?;
    let request = parse_batch_request(query, &state.limits)?;
    let requested_start_ns = request.start_ns;
    let requested_end_ns = request.end_ns;
    let requested_stream_count = request.stream_ids.len();
    let permit = Arc::clone(&state.blocking_requests)
        .acquire_owned()
        .await
        .map_err(|error| ServerError::internal("request limiter is unavailable", error))?;
    let limits = state.limits.clone();
    let recording_for_job = Arc::clone(&recording);
    let page = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        read_batch(recording_for_job, request, limits, request_id)
    })
    .await
    .map_err(|error| ServerError::internal("blocking message request failed", error))??;

    let total_response_ms = page.metrics.started.elapsed().as_secs_f64() * 1000.0;
    tracing::info!(
        request_id = page.metrics.request_id,
        recording_id = %recording.id,
        recording_revision = %recording.revision,
        start_ns = requested_start_ns,
        end_ns = requested_end_ns,
        stream_count = requested_stream_count,
        storage_read_calls = page.metrics.reads.calls,
        storage_read_bytes = page.metrics.reads.bytes,
        chunk_count = page.metrics.chunk_count,
        chunk_decompress_ms = page.metrics.chunk_decompress_ms,
        message_filter_ms = page.metrics.message_filter_ms,
        batch_encode_ms = page.metrics.batch_encode_ms,
        response_bytes = page.body.len(),
        messages_returned = page.message_count,
        complete = page.complete,
        total_response_ms,
        http_status = StatusCode::OK.as_u16(),
        "message batch served"
    );

    let mut response = Response::new(Body::from(page.body));
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(BATCH_CONTENT_TYPE));
    insert_header(
        &mut response,
        RECORDING_REVISION_HEADER,
        recording.revision.as_str(),
    )?;
    insert_header(
        &mut response,
        BATCH_COMPLETE_HEADER,
        if page.complete { "true" } else { "false" },
    )?;
    insert_header(
        &mut response,
        MESSAGE_COUNT_HEADER,
        &page.message_count.to_string(),
    )?;
    if let Some(cursor) = page.next_cursor {
        insert_header(&mut response, NEXT_CURSOR_HEADER, &cursor)?;
    }
    Ok(response)
}

fn parse_batch_request(query: MessageQuery, limits: &Limits) -> Result<BatchRequest, ServerError> {
    let start_ns = parse_timestamp("start_ns", &query.start_ns)?;
    let end_ns = match query.end_ns {
        Some(value) => parse_timestamp("end_ns", &value)?,
        None => start_ns
            .checked_add(limits.default_window_ns)
            .ok_or_else(|| {
                ServerError::bad_request("invalid_timestamp", "default end_ns overflows u64")
            })?,
    };
    let stream_ids = query
        .streams
        .split(',')
        .map(|value| {
            if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(ServerError::bad_request(
                    "invalid_streams",
                    "streams must be a comma-separated list of positive integers",
                ));
            }
            let stream_id = value.parse::<u32>().map_err(|_| {
                ServerError::bad_request("invalid_streams", "stream ID exceeds u32")
            })?;
            if stream_id == 0 {
                return Err(ServerError::bad_request(
                    "invalid_streams",
                    "stream IDs must be positive",
                ));
            }
            Ok(stream_id)
        })
        .collect::<Result<_, _>>()?;
    Ok(BatchRequest {
        revision: query.revision,
        stream_ids,
        start_ns,
        end_ns,
        max_bytes: query.max_bytes.unwrap_or(limits.default_response_bytes),
        max_messages: query.max_messages.unwrap_or(limits.default_max_messages),
        cursor: query.cursor,
    })
}

fn parse_timestamp(name: &str, value: &str) -> Result<u64, ServerError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ServerError::bad_request(
            "invalid_timestamp",
            format!("{name} must be an unsigned decimal integer"),
        ));
    }
    value
        .parse()
        .map_err(|_| ServerError::bad_request("invalid_timestamp", format!("{name} exceeds u64")))
}

fn find_recording(state: &AppState, id: &str) -> Result<Arc<Recording>, ServerError> {
    state
        .recordings
        .get(id)
        .cloned()
        .ok_or_else(|| ServerError::not_found("recording does not exist"))
}

fn insert_header(
    response: &mut Response,
    name: &'static str,
    value: &str,
) -> Result<(), ServerError> {
    let value = HeaderValue::from_str(value)
        .map_err(|error| ServerError::internal("response header creation failed", error))?;
    response
        .headers_mut()
        .insert(HeaderName::from_static(name), value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use tower::ServiceExt;

    fn fixture_config() -> ServerConfig {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/camera-jpeg/camera_front_3s.mcap")
            .canonicalize()
            .unwrap();
        ServerConfig::from_toml(&format!(
            r#"
allowed_origins = ["http://localhost:8080"]
[[recordings]]
id = "demo"
display_name = "Demo"
path = "{}"
"#,
            path.display()
        ))
        .unwrap()
    }

    #[tokio::test]
    async fn health_ready_list_and_catalog_work_without_a_socket() {
        let config = fixture_config();
        let state = AppState::initialize(&config).unwrap();
        let app = router(&config, state).unwrap();
        for path in ["/healthz", "/readyz", "/v1/recordings"] {
            let response = app
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/v1/recordings/demo/catalog")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let catalog: viewer_remote_protocol::CatalogResponse =
            serde_json::from_slice(&body).unwrap();
        assert!(!catalog.streams.is_empty());
    }

    #[tokio::test]
    async fn missing_recording_is_json_404_and_cors_preflight_is_restricted() {
        let config = fixture_config();
        let state = AppState::initialize(&config).unwrap();
        let app = router(&config, state).unwrap();
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/v1/recordings/missing/catalog")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/v1/recordings")
                    .header("origin", "http://localhost:8080")
                    .header("access-control-request-method", "GET")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["access-control-allow-origin"],
            "http://localhost:8080"
        );
    }

    #[tokio::test]
    async fn messages_return_binary_payload_continuation_headers_and_errors() {
        let config = fixture_config();
        let state = AppState::initialize(&config).unwrap();
        let recording = state.recordings["demo"].clone();
        let stream_id = recording.catalog.streams[0].id;
        let start = recording.catalog.time_range.start_ns.get();
        let end = recording.catalog.time_range.end_ns_exclusive.get();
        let revision = &recording.revision;
        let app = router(&config, state).unwrap();
        let uri = format!(
            "/v1/recordings/demo/messages?revision={revision}&streams={stream_id}&start_ns={start}&end_ns={end}&max_messages=1"
        );
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri(uri)
                    .header("origin", "http://localhost:8080")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CONTENT_TYPE], BATCH_CONTENT_TYPE);
        assert_eq!(response.headers()[BATCH_COMPLETE_HEADER], "false");
        assert_eq!(response.headers()[MESSAGE_COUNT_HEADER], "1");
        assert!(response.headers().contains_key(NEXT_CURSOR_HEADER));
        assert!(
            response.headers()["access-control-expose-headers"]
                .to_str()
                .unwrap()
                .contains("x-av-next-cursor")
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let messages = viewer_remote_protocol::BatchDecoder::new(&body)
            .unwrap()
            .collect()
            .unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].stream_id, stream_id);

        let stale = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!(
                        "/v1/recordings/demo/messages?revision=stale&streams={stream_id}&start_ns={start}&end_ns={end}"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(stale.status(), StatusCode::CONFLICT);

        let malformed = app
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!(
                        "/v1/recordings/demo/messages?revision={revision}&streams=x&start_ns={start}&end_ns={end}"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);
    }
}
