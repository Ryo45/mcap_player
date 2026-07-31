use crate::{
    interaction::ViewerAction,
    workspace::{PlotViewState, SpeedPlotCache},
};
use egui_plot::{Line, Plot, PlotPoints, VLine};
use viewer_core::{
    ArrivalTime, LoadedSignal, PlaybackCommand, PlaybackView, PlotMode, PlotPanelState,
    PlotViewport, arrival_time_from_plot_x, cursor_seconds, sample_at_or_before,
};

pub(crate) struct PlotViewInput<'a> {
    pub(crate) signal: Option<&'a LoadedSignal>,
    pub(crate) loading: bool,
    pub(crate) error: Option<&'a str>,
    pub(crate) playback: PlaybackView,
    pub(crate) display_time: ArrivalTime,
}

#[derive(Default)]
pub(crate) struct PlotViewOutput {
    pub(crate) actions: Vec<ViewerAction>,
}

pub(crate) fn show_plot_view(
    ui: &mut egui::Ui,
    input: PlotViewInput<'_>,
    state: &mut PlotViewState,
) -> PlotViewOutput {
    let mut output = PlotViewOutput::default();
    ui.horizontal(|ui| {
        ui.heading("SPEED · /odom");
        if let Some(signal) = input.signal {
            let current = sample_at_or_before(&signal.samples, input.display_time);
            ui.separator();
            ui.monospace(
                current
                    .map(|sample| {
                        format!("{:.2} m/s · {:.1} km/h", sample.value, sample.value * 3.6)
                    })
                    .unwrap_or_else(|| "waiting for first sample".to_owned()),
            );
        }
    });

    let Some(signal) = input.signal else {
        ui.allocate_ui(egui::vec2(ui.available_width(), 170.0), |ui| {
            ui.centered_and_justified(|ui| {
                if input.loading {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Loading /odom speed series…");
                    });
                } else if let Some(error) = input.error {
                    ui.colored_label(egui::Color32::RED, format!("Plot load failed: {error}"));
                } else {
                    ui.label("No /odom speed samples in this log");
                }
            });
        });
        return output;
    };

    let origin = signal.display.origin;
    let overview = PlotViewport::new(
        cursor_seconds(input.playback.start, origin),
        cursor_seconds(input.playback.end, origin),
    );
    let playhead = cursor_seconds(input.display_time, origin);
    let panel = state.panel.get_or_insert_with(|| {
        PlotPanelState::overview(overview.start_seconds, overview.end_seconds)
    });
    panel.update_follow(playhead, input.playback.playing);

    ui.horizontal(|ui| {
        let overview_selected = matches!(panel.mode, PlotMode::Overview);
        if ui.selectable_label(overview_selected, "Overview").clicked() {
            panel.overview_with_viewport(overview);
        }
        let follow_selected = matches!(panel.mode, PlotMode::Follow { .. });
        if ui.selectable_label(follow_selected, "Follow").clicked() {
            panel.follow(playhead);
        }
        ui.small(match panel.mode {
            PlotMode::Overview => "fixed range · drag/scroll to inspect · click to seek",
            PlotMode::Follow { .. } => "8 s history · 2 s lookahead",
        });
    });

    let navigation_input = ui.input(|input| {
        input.smooth_scroll_delta.x != 0.0 || (input.zoom_delta_2d().x - 1.0).abs() > f32::EPSILON
    });
    let cache_matches = state.cache.as_ref().is_some_and(|cache| {
        cache.origin == origin && cache.display_len == signal.display.x_seconds.len()
    });
    if !cache_matches {
        state.cache = Some(SpeedPlotCache {
            origin,
            display_len: signal.display.x_seconds.len(),
            points: signal
                .display
                .x_seconds
                .iter()
                .copied()
                .zip(signal.display.values.iter().copied())
                .map(|(x, value)| egui_plot::PlotPoint::new(x, value))
                .collect(),
        });
    }
    let points = PlotPoints::Borrowed(
        &state
            .cache
            .as_ref()
            .expect("speed plot cache was initialized")
            .points,
    );
    let viewport = panel.viewport;
    let response = Plot::new("vehicle-speed-plot")
        .height(170.0)
        .x_axis_label("arrival time (s)")
        .y_axis_label("speed (m/s)")
        .allow_drag([true, false])
        .allow_scroll([true, false])
        .allow_zoom([true, false])
        .allow_axis_zoom_drag([true, false])
        .auto_bounds([false, true])
        .show(ui, |plot_ui| {
            plot_ui.set_plot_bounds_x(viewport.start_seconds..=viewport.end_seconds);
            plot_ui.line(
                Line::new("speed", points)
                    .color(egui::Color32::from_rgb(80, 190, 255))
                    .allow_hover(false),
            );
            plot_ui.vline(
                VLine::new("playhead", playhead)
                    .color(egui::Color32::from_rgb(255, 190, 60))
                    .width(2.0)
                    .allow_hover(false),
            );
        });

    let bounds = response.transform.bounds();
    let visible = PlotViewport::new(bounds.min()[0], bounds.max()[0]);
    let comparison_epsilon = viewport.width().abs().max(1.0) * 1e-9;
    let bounds_changed = (visible.start_seconds - viewport.start_seconds).abs()
        > comparison_epsilon
        || (visible.end_seconds - viewport.end_seconds).abs() > comparison_epsilon;
    let manually_navigated = response.response.dragged_by(egui::PointerButton::Primary)
        || response.response.dragged_by(egui::PointerButton::Secondary)
        || (response.response.contains_pointer() && navigation_input)
        || bounds_changed;
    if manually_navigated {
        panel.overview_with_viewport(visible);
    } else {
        panel.viewport = visible;
    }

    if response.response.clicked_by(egui::PointerButton::Primary)
        && let Some(position) = response.response.interact_pointer_pos()
    {
        let clicked_x = response.transform.value_from_position(position).x;
        let clicked = arrival_time_from_plot_x(origin, clicked_x)
            .clamp(input.playback.start, input.playback.end);
        output
            .actions
            .push(ViewerAction::Playback(PlaybackCommand::Seek(clicked)));
    }
    output
}
