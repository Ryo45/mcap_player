#[cfg(test)]
use viewer_ui::action::ViewerAction;
mod diagnostics;
#[cfg(feature = "plugin-panels")]
mod plugin;
mod runtime;
mod topic_health;
mod topic_tree;
use crate::{inspection::TopicInspection, workspace::ViewerInteractionState};
pub(crate) use runtime::PanelRuntimeStore;
use std::collections::BTreeSet;
pub(crate) use topic_tree::{show_field_plots, show_topics};
use viewer_core::{PlaybackView, ViewerPresentation};
pub(crate) use viewer_ui::camera::CameraSelection;
pub(crate) use viewer_ui::inspector::InspectorPanelInput;
use viewer_ui::panel::{Panel, PanelRequirements};
pub(crate) use viewer_ui::panel_input::{PanelResourceView, PreviewDataView, SceneDataView};
pub(crate) use viewer_ui::placeholder::PlaceholderPanel;

pub(crate) enum NativePanel {
    Shared(Panel),
    #[cfg(feature = "plugin-panels")]
    Plugin(Box<plugin::PluginPanel>),
    Explore,
    TopicHealth(topic_health::TopicHealthPanel),
    Diagnostics(diagnostics::DiagnosticsPanel),
}
impl NativePanel {
    pub(crate) fn placeholder(panel: PlaceholderPanel) -> Self {
        Self::Shared(Panel::Placeholder(panel))
    }
    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        input: &PanelCompositionInput<'_>,
        shared: &viewer_ui::panel_input::PanelInput<'_>,
    ) -> PanelOutput {
        match self {
            Self::Shared(panel) => panel.show(ui, shared),
            #[cfg(feature = "plugin-panels")]
            Self::Plugin(panel) => panel.show(ui, input),
            Self::Explore => {
                ui.heading("Explore recording");
                ui.label("Open a topic under Topics and select a numeric field to add a plot.");
                PanelOutput::default()
            }
            Self::TopicHealth(panel) => panel.show(
                ui,
                input.topic_health,
                input.playback.map(|view| view.cursor),
            ),
            Self::Diagnostics(panel) => panel.show(
                ui,
                input.diagnostics,
                input.playback.map(|view| view.cursor),
            ),
        }
    }
    pub(crate) fn reset_for_source(&mut self, focused_camera: Option<viewer_core::CameraId>) {
        match self {
            Self::Shared(panel) => panel.reset_for_source(focused_camera),
            #[cfg(feature = "plugin-panels")]
            Self::Plugin(panel) => panel.reset(),
            Self::Explore | Self::TopicHealth(_) | Self::Diagnostics(_) => {}
        }
    }
    pub(crate) fn set_accumulate_points(&mut self, accumulate: bool) -> bool {
        let Self::Shared(Panel::Scene(panel)) = self else {
            return false;
        };
        panel.state.accumulate_points = accumulate;
        true
    }
    pub(crate) fn accumulate_points(&self) -> Option<bool> {
        match self {
            Self::Shared(Panel::Scene(panel)) => Some(panel.state.accumulate_points),
            _ => None,
        }
    }
    pub(crate) fn contribute_data_requirements(&self, requirements: &mut PanelDataRequirements) {
        match self {
            Self::Shared(panel) => panel.contribute_requirements(&mut requirements.shared),
            #[cfg(feature = "plugin-panels")]
            Self::Plugin(panel) => panel.requirements(requirements),
            Self::TopicHealth(panel) => panel.contribute_data_requirements(requirements),
            Self::Diagnostics(panel) => panel.contribute_data_requirements(requirements),
            Self::Explore => {}
        }
    }
    pub(crate) fn scheduler_priority_topic(&self) -> Option<&str> {
        match self {
            Self::Shared(Panel::Camera(panel)) => panel.scheduler_priority_topic(),
            _ => None,
        }
    }
    pub(crate) fn kind_name(&self) -> &'static str {
        match self {
            Self::Shared(panel) => panel.kind_name(),
            #[cfg(feature = "plugin-panels")]
            Self::Plugin(_) => "plugin",
            Self::Explore => "explore",
            Self::TopicHealth(_) => "topic-health",
            Self::Diagnostics(_) => "diagnostics",
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct PanelDataRequirements {
    pub(crate) shared: PanelRequirements,
    #[cfg(feature = "plugin-panels")]
    pub(crate) plugins: Vec<(String, viewer_extension::panel::PanelDefinition)>,
    pub(crate) topic_health: BTreeSet<viewer_mcap::TopicHealthRequirement>,
    pub(crate) diagnostics: BTreeSet<viewer_mcap::DiagnosticsRequirement>,
}

#[derive(Clone, Copy)]
pub(crate) struct PanelCompositionInput<'a> {
    #[cfg(feature = "plugin-panels")]
    pub(crate) plugin_generation: u64,
    #[cfg(feature = "plugin-panels")]
    pub(crate) plugin_data:
        &'a std::collections::BTreeMap<String, viewer_extension::panel::PanelUpdate>,
    pub(crate) playback: Option<PlaybackView>,
    pub(crate) presentation: &'a ViewerPresentation,
    pub(crate) camera_overlays: &'a viewer_renderer::CameraOverlayState,
    pub(crate) interaction: &'a ViewerInteractionState,
    pub(crate) signals: crate::signal_view::SignalView<'a>,
    pub(crate) preview: PreviewDataView<'a>,
    pub(crate) resources: PanelResourceView<'a>,
    pub(crate) scene: SceneDataView<'a>,
    pub(crate) inspections: &'a [TopicInspection],
    pub(crate) topic_health: &'a [crate::window_query_loader::TopicHealthQueryState],
    pub(crate) diagnostics: &'a [crate::window_query_loader::DiagnosticsQueryState],
    pub(crate) inspector_topic: Option<&'a str>,
    pub(crate) topic_tree: &'a viewer_core::TopicTree,
    pub(crate) numeric_series: &'a [crate::numeric_plot_loader::NumericSeriesState],
}

pub(crate) use viewer_ui::panel::{PanelOutput, PanelRenderRequests};

#[cfg(test)]
mod tests {
    use super::*;
    use viewer_core::PlaybackCommand;

    #[test]
    fn panel_output_aggregation_keeps_actions_and_render_requests() {
        let mut aggregate = PanelOutput::default();
        aggregate.merge(PanelOutput {
            actions: vec![ViewerAction::Playback(PlaybackCommand::Toggle)],
            render_requests: PanelRenderRequests {
                bev_size: Some(egui::vec2(320.0, 180.0)),
                scene: None,
                ..Default::default()
            },
        });
        assert_eq!(aggregate.actions.len(), 1);
        assert_eq!(
            aggregate.render_requests.bev_size,
            Some(egui::vec2(320.0, 180.0))
        );
        assert!(aggregate.render_requests.scene.is_none());
    }
}
