use crate::panels::{PanelCompositionInput, PanelOutput, PanelRuntimeStore};
use viewer_layout::LayoutDocument;
#[cfg(test)]
use viewer_layout::SplitDirection;
#[cfg(test)]
use viewer_ui::layout::{RectSpec, split_rect};
pub(crate) fn show_layout_host(
    ui: &mut egui::Ui,
    document: &LayoutDocument,
    panels: &mut PanelRuntimeStore,
    context: PanelCompositionInput<'_>,
) -> PanelOutput {
    use viewer_ui::panel_input::{PanelInput, SignalInput, SignalInputs};
    let inspections = context
        .inspections
        .iter()
        .map(crate::inspection::TopicInspection::view)
        .collect::<Vec<_>>();
    let numeric_series = context
        .numeric_series
        .iter()
        .map(|s| (s.request.stream_id, s.request.field_path.as_str()))
        .collect::<Vec<_>>();
    let signal = |id| {
        let signal = context.signals.get(id);
        SignalInput {
            signal: signal.signal,
            current: signal.current,
            loading: signal.loading,
            error: signal.error.map(|e| e as &dyn std::fmt::Display),
        }
    };
    let shared = PanelInput {
        playback: context.playback,
        display_time: context
            .playback
            .map(|p| context.interaction.display_time(p)),
        presentation: context.presentation,
        camera_overlays: context.camera_overlays,
        signals: SignalInputs {
            speed: signal(viewer_core::SignalId::Speed),
            yaw_rate: signal(viewer_core::SignalId::YawRate),
        },
        preview: context.preview,
        resources: context.resources,
        scene: Some(context.scene),
        inspections: &inspections,
        topic_inspections: &inspections,
        inspector_topic: context.inspector_topic,
        topic_tree: context.topic_tree,
        numeric_series: Some(&numeric_series),
    };
    viewer_ui::layout::show_layout(ui, document, &mut panels.panels, |ui, panel| {
        panel.show(ui, &context, &shared)
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        panels::{PanelCompositionInput, PanelResourceView, PreviewDataView, SceneDataView},
        signal_view::SignalView,
        workspace::NativeWorkspace,
    };
    use scene_renderer::SceneCameraMode;
    use viewer_core::{
        ArrivalTime, PlaybackSpeed, PlaybackView, SceneDiagnostics, ViewerPresentation,
    };

    fn rect(width: f32, height: f32) -> RectSpec {
        RectSpec {
            min_x: 10.0,
            min_y: 20.0,
            width,
            height,
        }
    }

    #[test]
    fn row_split_normalizes_weights_and_subtracts_gaps() {
        let children = split_rect(
            rect(104.0, 50.0),
            SplitDirection::Row,
            &[2.0, 1.0, 1.0],
            4.0,
        );
        assert_eq!(
            children,
            vec![
                RectSpec {
                    min_x: 10.0,
                    min_y: 20.0,
                    width: 48.0,
                    height: 50.0,
                },
                RectSpec {
                    min_x: 62.0,
                    min_y: 20.0,
                    width: 24.0,
                    height: 50.0,
                },
                RectSpec {
                    min_x: 90.0,
                    min_y: 20.0,
                    width: 24.0,
                    height: 50.0,
                },
            ]
        );
    }

    #[test]
    fn column_split_uses_vertical_axis() {
        let children = split_rect(rect(80.0, 104.0), SplitDirection::Column, &[1.0, 3.0], 4.0);
        assert_eq!(children[0].height, 25.0);
        assert_eq!(children[1].min_y, 49.0);
        assert_eq!(children[1].height, 75.0);
        assert!(children.iter().all(|child| child.width == 80.0));
    }

    #[test]
    fn tiny_viewport_never_produces_negative_or_out_of_bounds_rects() {
        let parent = rect(3.0, 2.0);
        let children = split_rect(parent, SplitDirection::Row, &[1.0, 1.0, 1.0], 4.0);
        let parent_end = parent.min_x + parent.width;
        assert!(children.iter().all(|child| {
            child.width >= 0.0
                && child.height >= 0.0
                && child.min_x >= parent.min_x
                && child.min_x + child.width <= parent_end
        }));
    }

    #[test]
    fn bundled_layout_invokes_all_panels_and_aggregates_render_requests() {
        use crate::workspace::WorkspaceLayout;
        for (preset, recording) in [
            (WorkspaceLayout::Standard, true),
            (WorkspaceLayout::Monitor, true),
            (WorkspaceLayout::Monitor, false),
            (WorkspaceLayout::Health, true),
            (WorkspaceLayout::Health, false),
        ] {
            egui::__run_test_ui(|ui| {
                let mut workspace = NativeWorkspace::load_bundled_or_fallback(preset);
                let layout = workspace.layout.clone();
                let presentation = ViewerPresentation::default();
                let diagnostics = SceneDiagnostics::default();
                let interaction = workspace.interaction.clone();
                let output = show_layout_host(
                    ui,
                    &layout,
                    &mut workspace.panels,
                    PanelCompositionInput {
                        playback: recording.then_some(PlaybackView {
                            start: ArrivalTime(0),
                            end: ArrivalTime(10),
                            cursor: ArrivalTime(2),
                            playing: false,
                            speed: PlaybackSpeed::Normal,
                        }),
                        presentation: &presentation,
                        camera_overlays: &viewer_renderer::CameraOverlayState::default(),
                        interaction: &interaction,
                        signals: SignalView::empty(),
                        preview: PreviewDataView {
                            active: false,
                            speed_overview: None,
                            bookmarks: &[],
                        },
                        resources: PanelResourceView {
                            camera_textures: &[],
                            preview_camera_textures: &[],
                            bev_texture: egui::TextureId::Managed(1),
                            scene_texture: egui::TextureId::Managed(2),
                        },
                        scene: SceneDataView {
                            diagnostics: &diagnostics,
                            visible_scan_points: 0,
                            camera_distance: 10.0,
                            camera_mode: SceneCameraMode::Chase,
                            static_transform_count: 0,
                            dynamic_transform_count: 0,
                        },
                        inspections: &[],
                        topic_health: &[],
                        diagnostics: &[],
                        inspector_topic: Some("/selected"),
                        topic_tree: &viewer_core::TopicTree::default(),
                        numeric_series: &[],
                        #[cfg(feature = "plugin-panels")]
                        plugin_generation: 0,
                        #[cfg(feature = "plugin-panels")]
                        plugin_data: &std::collections::BTreeMap::new(),
                    },
                );
                assert_eq!(
                    output.render_requests.bev_size.is_some(),
                    preset == WorkspaceLayout::Standard
                );
                assert_eq!(
                    output.render_requests.scene.is_some(),
                    preset != WorkspaceLayout::Health
                );
            });
        }
    }
}
