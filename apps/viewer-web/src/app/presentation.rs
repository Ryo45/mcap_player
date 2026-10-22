//! Viewer snapshot construction and DOM diagnostic presentation.
use super::{TOPIC, WebViewState, element, set_status};
use crate::playback::WebPlayback;
use std::collections::BTreeMap;
use viewer_core::{CameraId, DiagnosticsPresentation, PlaybackLoadState, ViewerPresentation};
use web_sys::{HtmlButtonElement, HtmlElement};
pub(super) fn build_viewer_presentation(
    session: &WebPlayback,
    view: &WebViewState,
    selected_camera: Option<CameraId>,
) -> ViewerPresentation {
    let start = session.clock().start().0;
    let overlay_status = view
        .camera
        .camera_overlays
        .snapshots()
        .map(|snapshot| (snapshot.camera_id, snapshot.status.clone()))
        .collect::<BTreeMap<_, _>>();
    ViewerPresentation::from_features(viewer_core::ViewerPresentationInput {
        cameras: session.cameras().state(),
        telemetry: session.odometry().state(),
        path: session.path().state(),
        point_cloud: session.point_cloud().map(|scene| scene.point_cloud()),
        camera_topics: session.camera_topics(),
        focused_camera: selected_camera,
        overlays: &overlay_status,
        diagnostics: DiagnosticsPresentation {
            source: session.source_label().to_owned(),
            primary_topic: session
                .camera_topics()
                .first()
                .map_or_else(|| TOPIC.to_owned(), |(_, topic)| topic.clone()),
            counters: session.counters(),
            playback_performance: Some(session.performance()),
            performance: view.presentation_metrics.snapshot().clone(),
            cursor_seconds: Some((session.clock().cursor().0 - start) as f64 / 1e9),
            ..DiagnosticsPresentation::default()
        },
    })
}

pub(super) fn update_dom_diagnostics(presentation: &ViewerPresentation, session: &WebPlayback) {
    let focus_label: HtmlElement = element("camera-focus-label");
    focus_label.set_inner_text(&presentation.focused_camera().map_or_else(
        || {
            if session.camera_topics().is_empty() {
                "No camera stream in this recording".to_owned()
            } else {
                "waiting".to_owned()
            }
        },
        |camera| format!("{} · {}", camera.topic, camera.overlay),
    ));
    for camera in &presentation.cameras {
        let camera_id = camera.id;
        let card: HtmlButtonElement = element(&format!("camera-card-{}", camera_id.0));
        card.set_class_name(if camera.focused {
            "camera-card selected"
        } else {
            "camera-card"
        });
        let label: HtmlElement = element(&format!("camera-label-{}", camera_id.0));
        label.set_inner_text(&format!("{} · {:.1} Hz", camera.topic, camera.fps));
    }
    let diagnostics = &presentation.diagnostics;
    let load = match session.load_state() {
        PlaybackLoadState::Ready => "Ready".to_owned(),
        PlaybackLoadState::Buffering {
            requested,
            committed,
        } => format!(
            "Buffering · waiting for +{:.3}s",
            requested.0.saturating_sub(committed.0) as f64 / 1e9
        ),
        PlaybackLoadState::Seeking { .. } => "Seeking".to_owned(),
        PlaybackLoadState::Failed { message } => format!("Failed: {message}"),
    };
    set_status(
        &format!(
            "{load} · {} decoded · {} errors · {} dropped · path {} pts · scan {} pts · {:.2}s",
            diagnostics.counters.decoded,
            diagnostics.counters.errors,
            diagnostics.counters.dropped,
            diagnostics.path_points,
            diagnostics.scan_points,
            diagnostics.cursor_seconds.unwrap_or_default()
        ),
        diagnostics.counters.errors > 0,
    );
    let playback_performance = diagnostics
        .playback_performance
        .as_ref()
        .expect("MCAP playback has performance diagnostics");
    let focused_fps = presentation
        .focused_camera()
        .map_or(0.0, |camera| camera.fps);
    let performance: HtmlElement = element("performance");
    let metrics = session.data_plane_diagnostics();
    let retention = metrics
        .retention_ratio
        .map_or_else(|| "n/a".to_owned(), |ratio| format!("{ratio:.2}x"));
    let data_plane = format!(
        " · {} {} loads/{} reads · {:.1} MiB loaded/{:.1} MiB decompressed · {} windows/{:.1} MiB logical/{:.1} MiB RAM ({retention}) · copied {:.1} MiB · ahead {:.2}/{:.2}s actual/target · {} evicted · last {:.1} ms (MCAP {:.1} ms) · underruns {} · stale {}",
        session.source_label(),
        metrics.load_requests,
        metrics.source_reads,
        metrics.source_bytes as f64 / (1024.0 * 1024.0),
        metrics.decompressed_bytes as f64 / (1024.0 * 1024.0),
        metrics.window_count,
        metrics.logical_payload_bytes as f64 / (1024.0 * 1024.0),
        metrics.resident_bytes as f64 / (1024.0 * 1024.0),
        metrics.per_message_copied_bytes as f64 / (1024.0 * 1024.0),
        metrics.actual_buffer_ahead.as_secs_f64(),
        metrics.target_ahead.as_secs_f64(),
        metrics.eviction_count,
        metrics.last_window_latency_ms,
        metrics.last_processing_ms,
        metrics.buffer_underrun_count,
        metrics.stale_results_discarded,
    );
    performance.set_inner_text(&format!(
        "Focus {focused_fps:.1} Hz · follows input · others ≤{:.0} Hz · image {:.2} ms · canvas {:.2} ms · tick {:.2} ms · source/controller {:.2}/{:.2} ms{data_plane}",
        playback_performance.background_camera_hz(),
        diagnostics.performance.camera_decode_ms,
        diagnostics.performance.upload_ms,
        diagnostics.performance.render_ms,
        playback_performance.source_read.average_ms,
        playback_performance.message_processing.average_ms,
    ));
    let telemetry: HtmlElement = element("telemetry");
    telemetry.set_inner_text(&presentation.telemetry.as_ref().map_or_else(
        || "Odometry: waiting".to_owned(),
        |frame| {
            format!(
                "x {:+.2} m · y {:+.2} m · yaw {:+.1}° · {:.2} m/s · yaw rate {:+.1}°/s",
                frame.position_x,
                frame.position_y,
                frame.yaw_radians.to_degrees(),
                frame.speed,
                frame.yaw_rate.to_degrees()
            )
        },
    ));
}
