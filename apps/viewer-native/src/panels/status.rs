use super::{
    NativePanel, PanelDataRequirements, PanelFrameContext, PanelOutput, PlaceholderPanel,
    STATUS_CONFIG_VERSION,
};
use crate::signal_query::SignalQueryView;
use serde::{Deserialize, Serialize};
use viewer_core::{ArrivalTime, PlaybackView, SignalId, SignalSample, sample_at_or_before};
use viewer_layout::{PanelId, PanelNode};

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
    }

    pub(crate) fn show(&self, ui: &mut egui::Ui, context: &PanelFrameContext<'_>) -> PanelOutput {
        ui.push_id((self.id.as_str(), self.title.as_deref()), |ui| {
            ui.heading(self.title.as_deref().unwrap_or("SESSION STATUS"));
            ui.separator();
            let main_camera = context.presentation.focused_camera();
            let display_time = context
                .playback
                .map(|playback| context.interaction.display_time(playback));
            let speed = display_time.and_then(|time| current_speed(context.signals, time));
            status_row(
                ui,
                "Playback",
                context.playback.map_or("Live", |playback| {
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
                &context
                    .playback
                    .map(format_playback_time)
                    .unwrap_or_else(|| "live".to_owned()),
            );
            status_row(
                ui,
                "Rate",
                context
                    .playback
                    .map_or("—", |playback| playback.speed.label()),
            );
            status_row(
                ui,
                "Main camera",
                main_camera.map_or("unavailable", |camera| camera.topic.as_str()),
            );
            status_row(
                ui,
                "Speed",
                &speed
                    .map(|sample| format!("{:.2} m/s", sample.value))
                    .unwrap_or_else(|| "waiting".to_owned()),
            );
            status_row(
                ui,
                "Overlay",
                &main_camera
                    .map(|camera| camera.overlay.to_string())
                    .unwrap_or_else(|| "waiting".to_owned()),
            );
            PanelOutput::default()
        })
        .inner
    }
}

fn current_speed(signals: SignalQueryView<'_>, time: ArrivalTime) -> Option<SignalSample> {
    signals
        .get(SignalId::Speed)
        .signal
        .and_then(|signal| sample_at_or_before(&signal.samples, time))
        .copied()
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
    use crate::signal_query::SignalDataView;
    use viewer_core::{LoadedSignal, PlotSeries};

    #[test]
    fn speed_status_reads_the_session_signal_query_view() {
        let speed = LoadedSignal {
            samples: vec![SignalSample {
                measurement_time: None,
                arrival_time: ArrivalTime(10),
                value: 4.5,
            }],
            display: PlotSeries {
                signal_id: SignalId::Speed,
                origin: ArrivalTime(0),
                x_seconds: vec![0.0],
                values: vec![4.5],
            },
        };
        let signals = SignalQueryView::new(
            SignalDataView {
                signal: Some(&speed),
                loading: false,
                error: None,
            },
            SignalDataView::default(),
        );

        assert_eq!(
            current_speed(signals, ArrivalTime(10)).map(|sample| sample.value),
            Some(4.5)
        );
    }
}
