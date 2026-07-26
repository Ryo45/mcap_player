//! Shared egui presentation model and playback controls.

use viewer_core::{
    ArrivalTime, CameraId, CameraStatus, PipelineCounters, PlaybackClock, PlaybackPerformance,
    PlaybackSpeed, PresentationSnapshot,
};

#[derive(Clone, Debug)]
pub struct ViewerPresentation {
    pub source: String,
    pub topic: String,
    pub camera_status: CameraStatus,
    pub counters: PipelineCounters,
    pub playback_performance: Option<PlaybackPerformance>,
    pub presentation_performance: PresentationSnapshot,
    pub focused_camera: Option<CameraId>,
    pub telemetry: Option<TelemetryPresentation>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TelemetryPresentation {
    pub frame_id: String,
    pub child_frame_id: String,
    pub position_x: f64,
    pub position_y: f64,
    pub yaw_radians: f64,
    pub forward_velocity: f64,
    pub speed: f64,
    pub yaw_rate: f64,
}

impl Default for ViewerPresentation {
    fn default() -> Self {
        Self {
            source: "No source".into(),
            topic: String::new(),
            camera_status: CameraStatus::WaitingForCameraFrame,
            counters: PipelineCounters::default(),
            playback_performance: None,
            presentation_performance: PresentationSnapshot::default(),
            focused_camera: None,
            telemetry: None,
            error: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PlaybackUiResponse {
    pub seeked: bool,
}

pub fn playback_controls(ui: &mut egui::Ui, clock: &mut PlaybackClock) -> PlaybackUiResponse {
    let mut response = PlaybackUiResponse::default();
    ui.horizontal(|ui| {
        let label = if clock.is_playing() { "Pause" } else { "Play" };
        if ui.button(label).clicked() {
            clock.toggle();
        }
        let mut selected = clock.speed();
        egui::ComboBox::from_id_salt("playback-speed")
            .selected_text(selected.label())
            .show_ui(ui, |ui| {
                for speed in PlaybackSpeed::ALL {
                    ui.selectable_value(&mut selected, speed, speed.label());
                }
            });
        if selected != clock.speed() {
            clock.set_speed(selected);
        }
    });
    let start = clock.start().0;
    let end = clock.end().0.max(start + 1);
    let mut cursor = clock.cursor().0;
    if ui
        .add(
            egui::Slider::new(&mut cursor, start..=end)
                .show_value(false)
                .text("timeline"),
        )
        .changed()
    {
        clock.seek(ArrivalTime(cursor));
        response.seeked = true;
    }
    ui.label(format!(
        "{:.3}s / {:.3}s",
        (clock.cursor().0 - start) as f64 / 1e9,
        (end - start) as f64 / 1e9
    ));
    response
}

pub fn source_status(ui: &mut egui::Ui, model: &ViewerPresentation) {
    ui.heading("JPEG Camera");
    ui.label(format!("Source: {}", model.source));
    ui.label(format!("Topic: {}", model.topic));
    let status = match model.camera_status {
        CameraStatus::WaitingForCameraFrame => "Waiting for camera frame",
        CameraStatus::Ready => "Ready",
        CameraStatus::Error => "Error",
    };
    ui.label(status);
    ui.label(format!(
        "Decoded: {}  Errors: {}  Unknown: {}  Dropped: {}",
        model.counters.decoded,
        model.counters.errors,
        model.counters.unknown_streams,
        model.counters.dropped
    ));
    ui.separator();
    ui.heading("Performance · 1 s window");
    let focused_fps = model
        .focused_camera
        .and_then(|camera_id| model.presentation_performance.camera_fps.get(&camera_id))
        .copied()
        .unwrap_or_default();
    ui.monospace(format!(
        "Camera total {:>5.1} Hz",
        model.presentation_performance.total_camera_fps
    ));
    if let Some(performance) = &model.playback_performance {
        ui.monospace(format!(
            "Focus {:>5.1}/{:.0} Hz · others ≤{:.0} Hz",
            focused_fps,
            performance.focused_camera_hz(),
            performance.background_camera_hz()
        ));
    } else {
        ui.monospace(format!("Focus {focused_fps:>5.1} Hz"));
    }
    ui.monospace(format!(
        "JPEG {:>5.2} ms · upload {:>5.2} ms",
        model.presentation_performance.jpeg_decode_ms, model.presentation_performance.upload_ms
    ));
    ui.monospace(format!(
        "UI/render {:>5.2} ms",
        model.presentation_performance.render_ms
    ));
    if let Some(performance) = &model.playback_performance {
        ui.monospace(format!(
            "MCAP {:>5.2} · CDR {:>5.2} · state {:>5.2} ms",
            performance.source_read.average_ms,
            performance.pipeline_decode.average_ms,
            performance.state_apply.average_ms
        ));
        ui.monospace(format!(
            "Camera input {} · presented {}",
            performance.camera_input_frames, performance.camera_presented_frames
        ));
    }
    ui.separator();
    ui.heading("Telemetry · /odom");
    if let Some(telemetry) = &model.telemetry {
        egui::Grid::new("telemetry-values")
            .num_columns(2)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                ui.label("Position");
                ui.monospace(format!(
                    "{:+.2}, {:+.2} m",
                    telemetry.position_x, telemetry.position_y
                ));
                ui.end_row();
                ui.label("Heading");
                ui.monospace(format!("{:+.1}°", telemetry.yaw_radians.to_degrees()));
                ui.end_row();
                ui.label("Speed");
                ui.monospace(format!(
                    "{:.2} m/s · {:.1} km/h",
                    telemetry.speed,
                    telemetry.speed * 3.6
                ));
                ui.end_row();
                ui.label("Forward");
                ui.monospace(format!("{:+.2} m/s", telemetry.forward_velocity));
                ui.end_row();
                ui.label("Yaw rate");
                ui.monospace(format!("{:+.1} °/s", telemetry.yaw_rate.to_degrees()));
                ui.end_row();
            });
        ui.small(format!(
            "{} → {}",
            telemetry.frame_id, telemetry.child_frame_id
        ));
    } else {
        ui.label("Waiting for odometry");
    }
    if let Some(error) = &model.error {
        ui.colored_label(egui::Color32::RED, error);
    }
}
