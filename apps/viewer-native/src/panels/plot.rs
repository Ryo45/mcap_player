use super::{
    NativePanel, PLOT_CONFIG_VERSION, PanelDataRequirements, PanelFrameContext, PanelOutput,
    PlaceholderPanel,
};
use crate::{
    graphics::views::{PlotViewInput, PlotViewKind, show_plot_view},
    workspace::PlotViewState,
};
use serde::{Deserialize, Serialize};
use viewer_layout::{PanelId, PanelNode};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum PlotSignal {
    VehicleSpeed,
    YawRate,
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
        match self.config.signal {
            PlotSignal::VehicleSpeed => requirements.vehicle_speed = true,
            PlotSignal::YawRate => requirements.yaw_rate = true,
        }
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
            match self.config.signal {
                PlotSignal::VehicleSpeed => {
                    let output = show_plot_view(
                        ui,
                        PlotViewInput {
                            kind: PlotViewKind::VehicleSpeed,
                            signal: context.plot.speed.signal,
                            loading: context.plot.speed.loading,
                            error: context.plot.speed.error,
                            playback,
                            display_time: context.interaction.display_time(playback),
                            preview_signal: context.preview.speed_overview,
                            bookmarks: context.preview.bookmarks,
                            plot_height: 170.0,
                        },
                        &mut self.state,
                    );
                    PanelOutput {
                        actions: output.actions,
                        ..PanelOutput::default()
                    }
                }
                PlotSignal::YawRate => {
                    let output = show_plot_view(
                        ui,
                        PlotViewInput {
                            kind: PlotViewKind::YawRate,
                            signal: context.plot.yaw_rate.signal,
                            loading: context.plot.yaw_rate.loading,
                            error: context.plot.yaw_rate.error,
                            playback,
                            display_time: context.interaction.display_time(playback),
                            preview_signal: None,
                            bookmarks: context.preview.bookmarks,
                            plot_height: 170.0,
                        },
                        &mut self.state,
                    );
                    PanelOutput {
                        actions: output.actions,
                        ..PanelOutput::default()
                    }
                }
            }
        })
        .inner
    }
}
