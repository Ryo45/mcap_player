use super::{
    NativePanel, PanelDataRequirements, PanelOutput, PlaceholderPanel, STATUS_CONFIG_VERSION,
};
use serde::{Deserialize, Serialize};
use viewer_core::{CameraPresentation, PlaybackView, SignalId, SignalSample};
use viewer_layout::{PanelId, PanelNode};

#[derive(Clone, Copy)]
pub(crate) struct StatusPanelInput<'a> {
    pub(crate) playback: Option<PlaybackView>,
    pub(crate) main_camera: Option<&'a CameraPresentation>,
    pub(crate) speed: Option<SignalSample>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct StatusPanelConfig {}

pub(crate) struct StatusPanel {
    id: PanelId,
    title: Option<String>,
}

impl StatusPanel {
    pub(crate) fn create(node: &PanelNode) -> NativePanel {
        if node.config_version != STATUS_CONFIG_VERSION {
            return NativePanel::Placeholder(PlaceholderPanel::unsupported_version(
                node,
                STATUS_CONFIG_VERSION,
            ));
        }
        match serde_json::from_value::<StatusPanelConfig>(node.config.clone()) {
            Ok(_) => NativePanel::Status(Self {
                id: node.id.clone(),
                title: node.title.clone(),
            }),
            Err(error) => NativePanel::Placeholder(PlaceholderPanel::invalid_config(
                node,
                format!("Invalid status config: {error}"),
            )),
        }
    }

    pub(crate) fn contribute_data_requirements(&self, requirements: &mut PanelDataRequirements) {
        requirements.signals.insert(SignalId::Speed);
        requirements.playback.require_odometry();
    }

    pub(crate) fn show(&self, ui: &mut egui::Ui, input: StatusPanelInput<'_>) -> PanelOutput {
        ui.push_id((self.id.as_str(), self.title.as_deref()), |ui| {
            ui.heading(self.title.as_deref().unwrap_or("SESSION STATUS"));
            ui.separator();
            status_row(
                ui,
                "Playback",
                input.playback.map_or("Live", |playback| {
                    if playback.playing {
                        "Playing"
                    } else {
                        "Paused"
                    }
                }),
            );
            status_row(
                ui,
                "Time",
                &input
                    .playback
                    .map(format_playback_time)
                    .unwrap_or_else(|| "live".to_owned()),
            );
            status_row(
                ui,
                "Rate",
                input
                    .playback
                    .map_or("—", |playback| playback.speed.label()),
            );
            status_row(
                ui,
                "Main camera",
                input
                    .main_camera
                    .map_or("unavailable", |camera| camera.topic.as_str()),
            );
            status_row(
                ui,
                "Speed",
                &input
                    .speed
                    .map(|sample| format!("{:.2} m/s", sample.value))
                    .unwrap_or_else(|| "waiting".to_owned()),
            );
            status_row(
                ui,
                "Overlay",
                &input
                    .main_camera
                    .map(|camera| camera.overlay.to_string())
                    .unwrap_or_else(|| "waiting".to_owned()),
            );
            PanelOutput::default()
        })
        .inner
    }
}

fn status_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(format!("{label}:")).color(egui::Color32::GRAY));
        ui.monospace(value);
    });
}

fn format_playback_time(playback: PlaybackView) -> String {
    let seconds = playback.cursor.0.saturating_sub(playback.start.0) as f64 / 1_000_000_000.0;
    format!("{seconds:.2} s")
}

#[cfg(test)]
mod tests {
    use super::*;
    use viewer_core::ArrivalTime;

    #[test]
    fn status_accepts_a_cursor_filtered_current_speed() {
        let speed = SignalSample {
            measurement_time: None,
            arrival_time: ArrivalTime(10),
            value: 4.5,
        };
        let input = StatusPanelInput {
            playback: None,
            main_camera: None,
            speed: Some(speed),
        };
        assert_eq!(input.speed.map(|sample| sample.value), Some(4.5));
    }
}
