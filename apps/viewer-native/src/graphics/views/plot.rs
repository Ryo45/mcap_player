use crate::{
    interaction::ViewerAction,
    panels::{
        PlotMode, PlotPanelState, PlotViewState, PlotViewport, PreviewPlotCache, SignalPlotCache,
    },
};
use egui_plot::{Line, Plot, PlotPoints, VLine};
use viewer_core::{
    ArrivalTime, Bookmark, LoadedSignal, PlaybackCommand, PlaybackView, SignalId, SignalOverview,
    SignalSample, arrival_time_from_plot_x, cursor_seconds,
};

#[derive(Clone, Copy)]
struct SignalPresentation {
    heading: &'static str,
    loading_label: &'static str,
    empty_label: &'static str,
    plot_id: &'static str,
    series_name: &'static str,
    axis_label: &'static str,
    format_current: fn(f64) -> String,
}

fn signal_presentation(signal_id: SignalId) -> SignalPresentation {
    match signal_id {
        SignalId::Speed => SignalPresentation {
            heading: "SPEED",
            loading_label: "Loading speed series…",
            empty_label: "No speed samples in this log",
            plot_id: "vehicle-speed-plot",
            series_name: "speed",
            axis_label: "speed (m/s)",
            format_current: |value| format!("{value:.2} m/s · {:.1} km/h", value * 3.6),
        },
        SignalId::YawRate => SignalPresentation {
            heading: "YAW RATE",
            loading_label: "Loading yaw-rate series…",
            empty_label: "No yaw-rate samples in this log",
            plot_id: "yaw-rate-plot",
            series_name: "yaw rate",
            axis_label: "yaw rate (rad/s)",
            format_current: |value| format!("{value:.3} rad/s"),
        },
    }
}

pub(crate) struct PlotViewInput<'a> {
    pub(crate) signal_id: SignalId,
    pub(crate) signal: Option<&'a LoadedSignal>,
    pub(crate) current: Option<SignalSample>,
    pub(crate) loading: bool,
    pub(crate) error: Option<&'a str>,
    pub(crate) playback: PlaybackView,
    pub(crate) display_time: ArrivalTime,
    pub(crate) preview_signal: Option<&'a SignalOverview>,
    pub(crate) bookmarks: &'a [Bookmark],
    pub(crate) plot_height: f32,
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
    let presentation = signal_presentation(input.signal_id);
    ui.horizontal(|ui| {
        ui.heading(presentation.heading);
        if input.signal.is_some() || input.current.is_some() {
            ui.separator();
            ui.monospace(
                input
                    .current
                    .map(|sample| (presentation.format_current)(sample.value))
                    .unwrap_or_else(|| "waiting for first sample".to_owned()),
            );
        }
        if input.loading {
            ui.separator();
            ui.spinner();
            ui.small("building bounded overview");
        }
    });

    if input.signal.is_none() && input.preview_signal.is_none() {
        ui.allocate_ui(egui::vec2(ui.available_width(), input.plot_height), |ui| {
            ui.centered_and_justified(|ui| {
                if input.loading {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(presentation.loading_label);
                    });
                } else if let Some(error) = input.error {
                    ui.colored_label(egui::Color32::RED, format!("Plot load failed: {error}"));
                } else {
                    ui.label(presentation.empty_label);
                }
            });
        });
        return output;
    }

    let origin = input
        .signal
        .map_or(input.playback.start, |signal| signal.display.origin);
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
    show_scrub_timeline(ui, &input, &mut output);

    let navigation_input = ui.input(|input| {
        input.smooth_scroll_delta.x != 0.0 || (input.zoom_delta_2d().x - 1.0).abs() > f32::EPSILON
    });
    let use_preview = input.preview_signal.is_some()
        && (matches!(panel.mode, PlotMode::Overview) || input.signal.is_none());
    let points = if use_preview {
        let overview = input.preview_signal.expect("preview signal checked");
        let first_bucket = overview.buckets().first().map(|bucket| bucket.start_time());
        let cache_matches = state.preview_cache.as_ref().is_some_and(|cache| {
            cache.origin == origin
                && cache.bucket_len == overview.buckets().len()
                && cache.first_bucket == first_bucket
        });
        if !cache_matches {
            state.preview_cache = Some(PreviewPlotCache {
                origin,
                first_bucket,
                bucket_len: overview.buckets().len(),
                points: overview
                    .buckets()
                    .iter()
                    .flat_map(|bucket| {
                        let x = cursor_seconds(bucket.start_time(), origin);
                        [
                            egui_plot::PlotPoint::new(x, bucket.min()),
                            egui_plot::PlotPoint::new(x, bucket.max()),
                        ]
                    })
                    .collect(),
            });
        }
        PlotPoints::Borrowed(
            &state
                .preview_cache
                .as_ref()
                .expect("preview plot cache was initialized")
                .points,
        )
    } else {
        let signal = input
            .signal
            .expect("exact signal required outside preview overview");
        let cache_matches = state.cache.as_ref().is_some_and(|cache| {
            cache.origin == origin
                && cache.display_len == signal.display.x_seconds.len()
                && cache.input_sample_count == signal.input_sample_count
        });
        if !cache_matches {
            state.cache = Some(SignalPlotCache {
                origin,
                display_len: signal.display.x_seconds.len(),
                input_sample_count: signal.input_sample_count,
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
        PlotPoints::Borrowed(
            &state
                .cache
                .as_ref()
                .expect("speed plot cache was initialized")
                .points,
        )
    };
    let viewport = panel.viewport;
    let response = Plot::new(presentation.plot_id)
        .height(input.plot_height)
        .x_axis_label("arrival time (s)")
        .y_axis_label(presentation.axis_label)
        .allow_drag([true, false])
        .allow_scroll([true, false])
        .allow_zoom([true, false])
        .allow_axis_zoom_drag([true, false])
        .auto_bounds([false, true])
        .show(ui, |plot_ui| {
            plot_ui.set_plot_bounds_x(viewport.start_seconds..=viewport.end_seconds);
            plot_ui.line(
                Line::new(presentation.series_name, points)
                    .color(egui::Color32::from_rgb(80, 190, 255))
                    .allow_hover(false),
            );
            plot_ui.vline(
                VLine::new("playhead", playhead)
                    .color(egui::Color32::from_rgb(255, 190, 60))
                    .width(2.0)
                    .allow_hover(false),
            );
            for bookmark in input.bookmarks {
                let start = cursor_seconds(bookmark.time(), origin);
                plot_ui.vline(
                    VLine::new(format!("bookmark-{}", bookmark.id()), start)
                        .color(egui::Color32::from_rgb(225, 90, 180))
                        .width(1.0)
                        .allow_hover(true),
                );
                if let Some(end) = bookmark.end_time() {
                    plot_ui.vline(
                        VLine::new(
                            format!("bookmark-end-{}", bookmark.id()),
                            cursor_seconds(end, origin),
                        )
                        .color(egui::Color32::from_rgba_unmultiplied(225, 90, 180, 120))
                        .width(1.0)
                        .allow_hover(false),
                    );
                }
            }
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

fn show_scrub_timeline(ui: &mut egui::Ui, input: &PlotViewInput<'_>, output: &mut PlotViewOutput) {
    let desired = egui::vec2(ui.available_width(), 18.0);
    let (rect, response) = ui.allocate_exact_size(desired, egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, egui::Color32::from_gray(32));
    let duration = (input.playback.end.0 - input.playback.start.0).max(1) as f64;
    let time_to_x = |time: ArrivalTime| {
        rect.left()
            + ((time.0 - input.playback.start.0) as f64 / duration).clamp(0.0, 1.0) as f32
                * rect.width()
    };
    for bookmark in input.bookmarks {
        let start_x = time_to_x(bookmark.time());
        if let Some(end) = bookmark.end_time() {
            let range = egui::Rect::from_min_max(
                egui::pos2(start_x, rect.top()),
                egui::pos2(time_to_x(end), rect.bottom()),
            );
            painter.rect_filled(
                range,
                0.0,
                egui::Color32::from_rgba_unmultiplied(225, 90, 180, 55),
            );
        } else {
            painter.line_segment(
                [
                    egui::pos2(start_x, rect.top()),
                    egui::pos2(start_x, rect.bottom()),
                ],
                egui::Stroke::new(2.0, egui::Color32::from_rgb(225, 90, 180)),
            );
        }
    }
    let cursor_x = time_to_x(input.display_time);
    painter.line_segment(
        [
            egui::pos2(cursor_x, rect.top()),
            egui::pos2(cursor_x, rect.bottom()),
        ],
        egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 190, 60)),
    );
    let pointer_time = response.interact_pointer_pos().map(|position| {
        let fraction = ((position.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0);
        ArrivalTime(input.playback.start.0 + (fraction as f64 * duration).round() as i64)
    });
    if response.drag_started()
        && let Some(time) = pointer_time
    {
        output.actions.push(ViewerAction::BeginPreview(time));
    } else if response.dragged()
        && let Some(time) = pointer_time
    {
        output
            .actions
            .push(ViewerAction::SetPreviewTime(Some(time)));
    }
    if response.drag_stopped()
        && let Some(time) = pointer_time
    {
        output.actions.push(ViewerAction::CommitPreview(time));
    } else if response.clicked()
        && let Some(position) = response.interact_pointer_pos()
    {
        let bookmark = input
            .bookmarks
            .iter()
            .find(|bookmark| (time_to_x(bookmark.time()) - position.x).abs() <= 5.0);
        let target = bookmark.map_or_else(
            || pointer_time.expect("clicked response has pointer time"),
            Bookmark::time,
        );
        output
            .actions
            .push(ViewerAction::Playback(PlaybackCommand::Seek(target)));
    }
}
