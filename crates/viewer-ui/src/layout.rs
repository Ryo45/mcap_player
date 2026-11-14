//! Shared bounded layout traversal for concrete Native/Web panel stores.
use crate::{panel::PanelOutput, workspace::PanelStore};
use viewer_layout::{LayoutDocument, LayoutNode, SplitDirection};
pub const SPLIT_GAP: f32 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RectSpec {
    pub min_x: f32,
    pub min_y: f32,
    pub width: f32,
    pub height: f32,
}

impl RectSpec {
    pub fn from_egui(rect: egui::Rect) -> Self {
        Self {
            min_x: rect.min.x,
            min_y: rect.min.y,
            width: rect.width().max(0.0),
            height: rect.height().max(0.0),
        }
    }

    pub fn to_egui(self) -> egui::Rect {
        egui::Rect::from_min_size(
            egui::pos2(self.min_x, self.min_y),
            egui::vec2(self.width.max(0.0), self.height.max(0.0)),
        )
    }
}

pub fn split_rect(
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

pub fn show_layout<P>(
    ui: &mut egui::Ui,
    document: &LayoutDocument,
    panels: &mut PanelStore<P>,
    mut show: impl FnMut(&mut egui::Ui, &mut P) -> PanelOutput,
) -> PanelOutput {
    let rect = ui.available_rect_before_wrap();
    let out = show_node(ui, &document.root, panels, rect, &mut show);
    ui.allocate_rect(rect, egui::Sense::hover());
    out
}
fn show_node<P>(
    ui: &mut egui::Ui,
    node: &LayoutNode,
    panels: &mut PanelStore<P>,
    rect: egui::Rect,
    show: &mut impl FnMut(&mut egui::Ui, &mut P) -> PanelOutput,
) -> PanelOutput {
    match node {
        LayoutNode::Split {
            direction,
            children,
        } => {
            let weights = children.iter().map(|c| c.weight).collect::<Vec<_>>();
            let rects = split_rect(RectSpec::from_egui(rect), *direction, &weights, SPLIT_GAP);
            let mut out = PanelOutput::default();
            for (child, rect) in children.iter().zip(rects) {
                out.merge(show_node(ui, &child.node, panels, rect.to_egui(), show));
            }
            out
        }
        LayoutNode::Panel(node) => {
            ui.scope_builder(
                egui::UiBuilder::new()
                    .id_salt(node.id.as_str())
                    .max_rect(rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
                |ui| {
                    ui.set_clip_rect(ui.clip_rect().intersect(rect));
                    panels
                        .get_mut(&node.id)
                        .map_or_else(PanelOutput::default, |panel| show(ui, panel))
                },
            )
            .inner
        }
    }
}
