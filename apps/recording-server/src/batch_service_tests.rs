use super::*;
use crate::config::{RecordingConfig, ServerConfig};
use std::path::Path;
use viewer_remote_protocol::BatchDecoder;

fn fixture() -> (Arc<Recording>, Limits) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/camera-jpeg/camera_front_3s.mcap")
        .canonicalize()
        .unwrap();
    let config = ServerConfig::from_toml(&format!(
        r#"
allowed_origins = ["http://localhost:8080"]
[[recordings]]
id = "demo"
display_name = "Demo"
path = "{}"
"#,
        path.display()
    ))
    .unwrap();
    let recording = Recording::open(
        &RecordingConfig {
            id: "demo".into(),
            display_name: "Demo".into(),
            path,
        },
        &config.limits,
    )
    .unwrap();
    (recording, config.limits)
}

fn request(recording: &Recording, max_messages: usize) -> BatchRequest {
    BatchRequest {
        revision: recording.revision.clone(),
        stream_ids: vec![recording.catalog.streams[0].id],
        start_ns: recording.catalog.time_range.start_ns.get(),
        end_ns: recording.catalog.time_range.end_ns_exclusive.get(),
        max_bytes: 64 * 1024 * 1024,
        max_messages,
        cursor: None,
    }
}

fn keys(page: &BatchPage) -> Vec<(u64, u32, u32, Vec<u8>)> {
    BatchDecoder::new(&page.body)
        .unwrap()
        .collect()
        .unwrap()
        .into_iter()
        .map(|message| {
            (
                message.log_time_ns,
                message.stream_id,
                message.sequence,
                message.payload.to_vec(),
            )
        })
        .collect()
}

#[test]
fn exact_batch_preserves_payload_and_time_bounds() {
    let (recording, limits) = fixture();
    let request = request(&recording, limits.max_messages);
    let page = read_batch(Arc::clone(&recording), request.clone(), limits, 1).unwrap();
    assert!(page.complete);
    let messages = keys(&page);
    assert!(!messages.is_empty());
    assert!(
        messages
            .windows(2)
            .all(|pair| { (pair[0].0, pair[0].1) <= (pair[1].0, pair[1].1) })
    );
    assert!(messages.iter().all(|message| {
        message.0 >= request.start_ns && message.0 < request.end_ns && !message.3.is_empty()
    }));
}

#[test]
fn continuation_pages_have_no_duplicates_or_gaps() {
    let (recording, limits) = fixture();
    let complete = read_batch(
        Arc::clone(&recording),
        request(&recording, limits.max_messages),
        limits.clone(),
        1,
    )
    .unwrap();
    let expected = keys(&complete);
    assert!(expected.len() > 1);

    let mut paged = Vec::new();
    let mut cursor = None;
    loop {
        let mut request = request(&recording, 1);
        request.cursor = cursor;
        let page = read_batch(Arc::clone(&recording), request, limits.clone(), 2).unwrap();
        paged.extend(keys(&page));
        if page.complete {
            break;
        }
        cursor = page.next_cursor;
    }
    assert_eq!(paged, expected);
}

#[test]
fn rejects_mismatches_unknown_stream_window_and_hard_message_limit() {
    let (recording, mut limits) = fixture();
    let mut wrong_revision = request(&recording, 1);
    wrong_revision.revision = "stale".into();
    assert_eq!(
        read_batch(Arc::clone(&recording), wrong_revision, limits.clone(), 1)
            .unwrap_err()
            .kind,
        crate::error::ErrorKind::Conflict
    );

    let mut unknown = request(&recording, 1);
    unknown.stream_ids = vec![u32::MAX];
    assert!(read_batch(Arc::clone(&recording), unknown, limits.clone(), 1).is_err());

    let mut window = request(&recording, 1);
    window.end_ns = window.start_ns + limits.max_window_ns + 1;
    assert!(read_batch(Arc::clone(&recording), window, limits.clone(), 1).is_err());

    limits.max_response_bytes = 40;
    let mut oversized = request(&recording, 1);
    oversized.max_bytes = 40;
    assert_eq!(
        read_batch(recording, oversized, limits, 1)
            .unwrap_err()
            .kind,
        crate::error::ErrorKind::TooLarge
    );
}

#[test]
fn cursor_is_bound_to_the_original_query() {
    let (recording, limits) = fixture();
    let first = read_batch(
        Arc::clone(&recording),
        request(&recording, 1),
        limits.clone(),
        1,
    )
    .unwrap();
    let mut mismatched = request(&recording, 1);
    mismatched.start_ns += 1;
    mismatched.cursor = first.next_cursor;
    assert!(read_batch(recording, mismatched, limits, 2).is_err());
}
