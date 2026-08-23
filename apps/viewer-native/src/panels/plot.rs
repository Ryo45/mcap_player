use super::{
    NativePanel, PLOT_CONFIG_VERSION, PanelDataRequirements, PanelOutput, PlaceholderPanel,
};
use crate::graphics::views::{PlotViewInput, show_plot_view};
use egui_plot::PlotPoint;
use serde::{Deserialize, Serialize};
use viewer_core::{
    ArrivalTime, Bookmark, LoadedSignal, PlaybackView, SignalId, SignalOverview, SignalSample,
};
use viewer_layout::{PanelId, PanelNode};

#[derive(Clone, Copy)]
pub(crate) struct PlotPanelInput<'a> {
    pub(crate) playback: Option<PlaybackView>,
    pub(crate) signal: Option<&'a LoadedSignal>,
    pub(crate) current: Option<SignalSample>,
    pub(crate) loading: bool,
    pub(crate) error: Option<&'a str>,
    pub(crate) display_time: Option<ArrivalTime>,
    pub(crate) preview_signal: Option<&'a SignalOverview>,
    pub(crate) bookmarks: &'a [Bookmark],
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum PlotMode {
    #[default]
    Overview,
    Follow {
        history_seconds: f64,
        lookahead_seconds: f64,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct PlotViewport {
    pub(crate) start_seconds: f64,
    pub(crate) end_seconds: f64,
}

impl PlotViewport {
    pub(crate) fn new(start_seconds: f64, end_seconds: f64) -> Self {
        Self {
            start_seconds,
            end_seconds: end_seconds.max(start_seconds + f64::EPSILON),
        }
    }

    pub(crate) fn width(self) -> f64 {
        self.end_seconds - self.start_seconds
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PlotPanelState {
    pub(crate) mode: PlotMode,
    pub(crate) viewport: PlotViewport,
}

impl PlotPanelState {
    pub(crate) fn overview(start_seconds: f64, end_seconds: f64) -> Self {
        Self {
            mode: PlotMode::Overview,
            viewport: PlotViewport::new(start_seconds, end_seconds),
        }
    }

    pub(crate) fn follow(&mut self, playhead: f64) {
        self.mode = PlotMode::Follow {
            history_seconds: 8.0,
            lookahead_seconds: 2.0,
        };
        self.viewport = followed_viewport(playhead, 8.0, 2.0);
    }

    pub(crate) fn overview_with_viewport(&mut self, viewport: PlotViewport) {
        self.mode = PlotMode::Overview;
        self.viewport = viewport;
    }

    pub(crate) fn update_follow(&mut self, playhead: f64, playing: bool) -> bool {
        if !playing {
            return false;
        }
        let PlotMode::Follow {
            history_seconds,
            lookahead_seconds,
        } = self.mode
        else {
            return false;
        };
        if !should_shift_viewport(&self.viewport, playhead) {
            return false;
        }
        self.viewport = followed_viewport(playhead, history_seconds, lookahead_seconds);
        true
    }
}

fn should_shift_viewport(viewport: &PlotViewport, playhead: f64) -> bool {
    let threshold = viewport.start_seconds + viewport.width() * 0.8;
    playhead < viewport.start_seconds || playhead > threshold
}

fn followed_viewport(playhead: f64, history_seconds: f64, lookahead_seconds: f64) -> PlotViewport {
    PlotViewport::new(
        playhead - history_seconds.max(0.0),
        playhead + lookahead_seconds.max(0.0),
    )
}

#[derive(Default)]
pub(crate) struct PlotViewState {
    pub(crate) panel: Option<PlotPanelState>,
    pub(crate) cache: Option<SignalPlotCache>,
    pub(crate) preview_cache: Option<PreviewPlotCache>,
}

pub(crate) struct SignalPlotCache {
    pub(crate) origin: ArrivalTime,
    pub(crate) display_len: usize,
    pub(crate) input_sample_count: u64,
    pub(crate) points: Vec<PlotPoint>,
}

pub(crate) struct PreviewPlotCache {
    pub(crate) origin: ArrivalTime,
    pub(crate) first_bucket: Option<ArrivalTime>,
    pub(crate) bucket_len: usize,
    pub(crate) points: Vec<PlotPoint>,
}

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
    pub(crate) fn signal_id(&self) -> SignalId {
        self.config.signal.signal_id()
    }

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
        requirements.playback.require_odometry();
    }

    pub(crate) fn show(&mut self, ui: &mut egui::Ui, input: PlotPanelInput<'_>) -> PanelOutput {
        ui.push_id((self.id.as_str(), self.title.as_deref()), |ui| {
            let Some(playback) = input.playback else {
                ui.centered_and_justified(|ui| {
                    ui.label("Speed plot is unavailable in live mode");
                });
                return PanelOutput::default();
            };
            let signal_id = self.config.signal.signal_id();
            let output = show_plot_view(
                ui,
                PlotViewInput {
                    signal_id,
                    signal: input.signal,
                    current: input.current,
                    loading: input.loading,
                    error: input.error,
                    playback,
                    display_time: input.display_time.unwrap_or(playback.cursor),
                    preview_signal: input.preview_signal,
                    bookmarks: input.bookmarks,
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

    #[test]
    fn follow_shifts_at_boundary_but_not_while_paused() {
        let mut panel = PlotPanelState::overview(0.0, 100.0);
        panel.follow(20.0);
        assert_eq!(panel.viewport, PlotViewport::new(12.0, 22.0));
        assert!(!panel.update_follow(21.0, false));
        assert!(!panel.update_follow(19.9, true));
        assert!(panel.update_follow(20.1, true));
        assert!((panel.viewport.start_seconds - 12.1).abs() < 1e-12);
        assert!((panel.viewport.end_seconds - 22.1).abs() < 1e-12);
    }
}
