use crate::panels::{PanelFrameContext, PanelOutput, PanelRuntimeStore};
use viewer_layout::{LayoutDocument, LayoutNode, SplitDirection};

pub(crate) const SPLIT_GAP: f32 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RectSpec {
    pub(crate) min_x: f32,
    pub(crate) min_y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
}

impl RectSpec {
    fn from_egui(rect: egui::Rect) -> Self {
        Self {
            min_x: rect.min.x,
            min_y: rect.min.y,
            width: rect.width().max(0.0),
            height: rect.height().max(0.0),
        }
    }

    fn to_egui(self) -> egui::Rect {
        egui::Rect::from_min_size(
            egui::pos2(self.min_x, self.min_y),
            egui::vec2(self.width.max(0.0), self.height.max(0.0)),
        )
    }
}

pub(crate) fn split_rect(
    parent: RectSpec,
    direction: SplitDirection,
    weights: &[f32],
    requested_gap: f32,
) -> Vec<RectSpec> {
    if weights.is_empty() {
        return Vec::new();
    }
    let axis_size = match direction {
        SplitDirection::Row => parent.width,
        SplitDirection::Column => parent.height,
    }
    .max(0.0);
    let gap_count = weights.len().saturating_sub(1);
    let gap = if gap_count == 0 {
        0.0
    } else {
        requested_gap.max(0.0).min(axis_size / gap_count as f32)
    };
    let available = (axis_size - gap * gap_count as f32).max(0.0);
    let total_weight = weights.iter().copied().sum::<f32>();
    let mut cursor = match direction {
        SplitDirection::Row => parent.min_x,
        SplitDirection::Column => parent.min_y,
    };
    weights
        .iter()
        .enumerate()
        .map(|(index, weight)| {
            let size = if index + 1 == weights.len() {
                let parent_end = match direction {
                    SplitDirection::Row => parent.min_x + parent.width,
                    SplitDirection::Column => parent.min_y + parent.height,
                };
                (parent_end - cursor).max(0.0)
            } else {
                (available * (*weight / total_weight)).max(0.0)
            };
            let rect = match direction {
                SplitDirection::Row => RectSpec {
                    min_x: cursor,
                    min_y: parent.min_y,
                    width: size,
                    height: parent.height.max(0.0),
                },
                SplitDirection::Column => RectSpec {
                    min_x: parent.min_x,
                    min_y: cursor,
                    width: parent.width.max(0.0),
                    height: size,
                },
            };
            cursor += size + gap;
            rect
        })
        .collect()
}

pub(crate) fn show_layout_host(
    ui: &mut egui::Ui,
    document: &LayoutDocument,
    panels: &mut PanelRuntimeStore,
    context: PanelFrameContext<'_>,
) -> PanelOutput {
    let rect = ui.available_rect_before_wrap();
    let output = show_node(ui, &document.root, panels, context, rect, "root");
    ui.allocate_rect(rect, egui::Sense::hover());
    output
}

fn show_node(
    ui: &mut egui::Ui,
    node: &LayoutNode,
    panels: &mut PanelRuntimeStore,
    context: PanelFrameContext<'_>,
    rect: egui::Rect,
    path: &str,
) -> PanelOutput {
    match node {
        LayoutNode::Split {
            direction,
            children,
        } => {
            let weights = children
                .iter()
                .map(|child| child.weight)
                .collect::<Vec<_>>();
            let rects = split_rect(RectSpec::from_egui(rect), *direction, &weights, SPLIT_GAP);
            let mut output = PanelOutput::default();
            for (index, (child, child_rect)) in children.iter().zip(rects).enumerate() {
                let child_path = format!("{path}.children[{index}]");
                let child_output = ui
                    .scope_builder(
                        egui::UiBuilder::new()
                            .id_salt(&child_path)
                            .max_rect(child_rect.to_egui())
                            .layout(egui::Layout::top_down(egui::Align::Min)),
                        |ui| {
                            show_node(
                                ui,
                                &child.node,
                                panels,
                                context,
                                child_rect.to_egui(),
                                &child_path,
                            )
                        },
                    )
                    .inner;
                output.merge(child_output);
            }
            output
        }
        LayoutNode::Panel(panel_node) => {
            ui.scope_builder(
                egui::UiBuilder::new()
                    .id_salt(panel_node.id.as_str())
                    .max_rect(rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
                |ui| {
                    if let Some(panel) = panels.get_mut(&panel_node.id) {
                        panel.show(ui, &context)
                    } else {
                        ui.group(|ui| {
                            ui.colored_label(
                                egui::Color32::RED,
                                format!(
                                    "Panel runtime is missing for {} ({})",
                                    panel_node.id, panel_node.panel_type
                                ),
                            );
                        });
                        PanelOutput::default()
                    }
                },
            )
            .inner
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        panels::{
            PanelFrameContext, PanelResourceView, PlotDataView, PreviewDataView, SceneDataView,
            SignalDataView,
        },
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
        egui::__run_test_ui(|ui| {
            let mut workspace = NativeWorkspace::default();
            let layout = workspace.layout.clone();
            let presentation = ViewerPresentation::default();
            let diagnostics = SceneDiagnostics::default();
            let interaction = workspace.interaction.clone();
            let output = show_layout_host(
                ui,
                &layout,
                &mut workspace.panels,
                PanelFrameContext {
                    playback: Some(PlaybackView {
                        start: ArrivalTime(0),
                        end: ArrivalTime(10),
                        cursor: ArrivalTime(2),
                        playing: false,
                        speed: PlaybackSpeed::Normal,
                    }),
                    presentation: &presentation,
                    camera_overlays: &viewer_renderer::CameraOverlayState::default(),
                    interaction: &interaction,
                    plot: PlotDataView {
                        speed: SignalDataView {
                            signal: None,
                            loading: false,
                            error: None,
                        },
                        yaw_rate: SignalDataView {
                            signal: None,
                            loading: false,
                            error: None,
                        },
                    },
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
                },
            );
            assert!(output.render_requests.bev_size.is_some());
            assert!(output.render_requests.scene.is_some());
        });
    }
}
