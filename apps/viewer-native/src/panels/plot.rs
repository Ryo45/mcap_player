use super::{
    NativePanel, PLOT_CONFIG_VERSION, PanelDataRequirements, PanelFrameContext, PanelOutput,
    PlaceholderPanel,
};
use crate::{
    graphics::views::{PlotViewInput, show_plot_view},
    workspace::PlotViewState,
};
use serde::{Deserialize, Serialize};
use viewer_core::SignalId;
use viewer_layout::{PanelId, PanelNode};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum PlotSignal {
    VehicleSpeed,
    YawRate,
}

impl PlotSignal {
    pub(crate) fn signal_id(self) -> SignalId {
        match self {
            Self::VehicleSpeed => SignalId::Speed,
            Self::YawRate => SignalId::YawRate,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlotPanelConfig {
    pub(crate) signal: PlotSignal,
}

pub(crate) struct PlotPanel {
    id: PanelId,
    title: Option<String>,
    config: PlotPanelConfig,
    pub(crate) state: PlotViewState,
}

impl PlotPanel {
    pub(crate) fn create(node: &PanelNode) -> NativePanel {
        if node.config_version != PLOT_CONFIG_VERSION {
            return NativePanel::Placeholder(PlaceholderPanel::unsupported_version(
                node,
                PLOT_CONFIG_VERSION,
            ));
        }
        match serde_json::from_value::<PlotPanelConfig>(node.config.clone()) {
            Ok(config) => NativePanel::Plot(Self {
                id: node.id.clone(),
                title: node.title.clone(),
                config,
                state: PlotViewState::default(),
            }),
            Err(error) => NativePanel::Placeholder(PlaceholderPanel::invalid_config(
                node,
                format!("Invalid plot config: {error}"),
            )),
        }
    }

    pub(crate) fn reset_for_source(&mut self) {
        self.state = PlotViewState::default();
    }

    pub(crate) fn contribute_data_requirements(&self, requirements: &mut PanelDataRequirements) {
        requirements.signals.insert(self.config.signal.signal_id());
    }

    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        context: &PanelFrameContext<'_>,
    ) -> PanelOutput {
        ui.push_id((self.id.as_str(), self.title.as_deref()), |ui| {
            let Some(playback) = context.playback else {
                ui.centered_and_justified(|ui| {
                    ui.label("Speed plot is unavailable in live mode");
                });
                return PanelOutput::default();
            };
            let signal_id = self.config.signal.signal_id();
            let signal = context.signals.get(signal_id);
            let preview_signal = match signal_id {
                SignalId::Speed => context.preview.speed_overview,
                SignalId::YawRate => None,
            };
            let output = show_plot_view(
                ui,
                PlotViewInput {
                    signal_id,
                    signal: signal.signal,
                    loading: signal.loading,
                    error: signal.error,
                    playback,
                    display_time: context.interaction.display_time(playback),
                    preview_signal,
                    bookmarks: context.preview.bookmarks,
                    plot_height: 170.0,
                },
                &mut self.state,
            );
            PanelOutput {
                actions: output.actions,
                ..PanelOutput::default()
            }
        })
        .inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_plot_signals_map_to_the_closed_signal_ids() {
        assert_eq!(PlotSignal::VehicleSpeed.signal_id(), SignalId::Speed);
        assert_eq!(PlotSignal::YawRate.signal_id(), SignalId::YawRate);
    }
}
