//! Bounded signal Plot, navigation state and drawing shared by both hosts.
use crate::{
    panel::PanelOutput,
    views::plot::{PlotViewInput, show_plot_view},
};
use egui_plot::PlotPoint;
use serde::{Deserialize, Serialize};
use viewer_core::{
    ArrivalTime, Bookmark, LoadedSignal, PlaybackView, SignalId, SignalOverview, SignalSample,
};
use viewer_layout::{PanelId, PanelNode};

pub const PLOT_CONFIG_VERSION: u32 = 1;

#[derive(Clone, Copy)]
pub struct PlotPanelInput<'a> {
    pub playback: Option<PlaybackView>,
    pub signal: Option<&'a LoadedSignal>,
    pub current: Option<SignalSample>,
    pub loading: bool,
    pub error: Option<&'a dyn std::fmt::Display>,
    pub display_time: Option<ArrivalTime>,
    pub preview_signal: Option<&'a SignalOverview>,
    pub bookmarks: &'a [Bookmark],
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
    pub start_seconds: f64,
    pub end_seconds: f64,
}

impl PlotViewport {
    pub fn new(start_seconds: f64, end_seconds: f64) -> Self {
        Self {
            start_seconds,
            end_seconds: end_seconds.max(start_seconds + f64::EPSILON),
        }
    }

    pub fn width(self) -> f64 {
        self.end_seconds - self.start_seconds
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PlotPanelState {
    pub mode: PlotMode,
    pub viewport: PlotViewport,
}

impl PlotPanelState {
    pub fn overview(start_seconds: f64, end_seconds: f64) -> Self {
        Self {
            mode: PlotMode::Overview,
            viewport: PlotViewport::new(start_seconds, end_seconds),
        }
    }

    pub fn follow(&mut self, playhead: f64) {
        self.mode = PlotMode::Follow {
            history_seconds: 8.0,
            lookahead_seconds: 2.0,
        };
        self.viewport = followed_viewport(playhead, 8.0, 2.0);
    }

    pub fn overview_with_viewport(&mut self, viewport: PlotViewport) {
        self.mode = PlotMode::Overview;
        self.viewport = viewport;
    }

    pub fn update_follow(&mut self, playhead: f64, playing: bool) -> bool {
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
    pub panel: Option<PlotPanelState>,
    pub cache: Option<SignalPlotCache>,
    pub preview_cache: Option<PreviewPlotCache>,
}

pub(crate) struct SignalPlotCache {
    pub origin: ArrivalTime,
    pub display_len: usize,
    pub input_sample_count: u64,
    pub points: Vec<PlotPoint>,
}

pub(crate) struct PreviewPlotCache {
    pub origin: ArrivalTime,
    pub first_bucket: Option<ArrivalTime>,
    pub bucket_len: usize,
    pub points: Vec<PlotPoint>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum PlotSignal {
    VehicleSpeed,
    YawRate,
}

impl PlotSignal {
    pub fn signal_id(self) -> SignalId {
        match self {
            Self::VehicleSpeed => SignalId::Speed,
            Self::YawRate => SignalId::YawRate,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlotPanelConfig {
    pub signal: PlotSignal,
}

pub struct PlotPanel {
    id: PanelId,
    title: Option<String>,
    config: PlotPanelConfig,
    state: PlotViewState,
}

impl PlotPanel {
    pub fn signal_id(&self) -> SignalId {
        self.config.signal.signal_id()
    }

    pub fn create(node: &PanelNode) -> Result<Self, String> {
        if node.config_version != PLOT_CONFIG_VERSION {
            return Err(format!(
                "Unsupported {} config version {}; expected {}",
                node.panel_type, node.config_version, PLOT_CONFIG_VERSION
            ));
        }
        match serde_json::from_value::<PlotPanelConfig>(node.config.clone()) {
            Ok(config) => Ok(Self {
                id: node.id.clone(),
                title: node.title.clone(),
                config,
                state: PlotViewState::default(),
            }),
            Err(error) => Err(format!("Invalid plot config: {error}")),
        }
    }

    pub fn reset_for_source(&mut self) {
        self.state = PlotViewState::default();
    }

    pub fn show(&mut self, ui: &mut egui::Ui, input: PlotPanelInput<'_>) -> PanelOutput {
        ui.push_id((self.id.as_str(), self.title.as_deref()), |ui| {
            let Some(playback) = input.playback else {
                ui.centered_and_justified(|ui| {
                    ui.label("Speed plot is unavailable in live mode");
                });
                return PanelOutput::default();
            };
            let signal_id = self.config.signal.signal_id();
            let plot_height = (ui.available_height() - 84.0).max(48.0);
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
                    plot_height,
                },
                &mut self.state,
            );
            PanelOutput {
                actions: output.actions,
                render_requests: crate::panel::PanelRenderRequests {
                    plot_rect: output.plot_rect,
                    ..Default::default()
                },
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
