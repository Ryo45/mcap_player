use super::{
    NativePanel, PanelCompositionInput, PanelDataRequirements, PanelOutput, PlaceholderPanel,
};
use crate::interaction::ViewerAction;
use std::{collections::BTreeMap, path::PathBuf};
use viewer_extension::{
    WasmPanel,
    panel::{HostCommand, PanelEvent, PanelOutput as PluginOutput, UiNode},
};
use viewer_layout::{PanelId, PanelNode};

pub(crate) struct PluginPanel {
    pub id: PanelId,
    manifest: PathBuf,
    vm: WasmPanel,
    generation: Option<u64>,
    last_key: Option<(u64, u64)>,
    output: Option<PluginOutput>,
    error: Option<String>,
    events: Vec<PanelEvent>,
    plots: BTreeMap<String, Vec<egui_plot::PlotPoint>>,
}
impl PluginPanel {
    pub fn create(node: &PanelNode) -> NativePanel {
        let result = (|| -> anyhow::Result<Self> {
            anyhow::ensure!(node.config_version == 1, "unsupported PluginPanel config");
            let path = node
                .config
                .get("manifest")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("PluginPanel needs a manifest path"))?;
            let manifest = PathBuf::from(path);
            let vm = WasmPanel::load(&manifest)?;
            Ok(Self {
                id: node.id.clone(),
                manifest,
                vm,
                generation: None,
                last_key: None,
                output: None,
                error: None,
                events: vec![],
                plots: BTreeMap::new(),
            })
        })();
        match result {
            Ok(panel) => NativePanel::Plugin(Box::new(panel)),
            Err(error) => {
                NativePanel::Placeholder(PlaceholderPanel::invalid_config(node, error.to_string()))
            }
        }
    }
    pub fn reset(&mut self) {
        self.last_key = None;
        self.output = None;
        self.events.clear();
        self.plots.clear();
        self.error = self.vm.reset().err().map(|e| e.to_string());
    }
    pub fn requirements(&self, requirements: &mut PanelDataRequirements) {
        requirements
            .plugins
            .push((self.id.to_string(), self.vm.definition().clone()));
    }
    fn reload(&mut self) {
        self.events.clear();
        match WasmPanel::load(&self.manifest) {
            Ok(vm) if vm.definition() == self.vm.definition() => {
                self.vm = vm;
                self.output = None;
                self.error = None;
                self.last_key = None;
            }
            Ok(_) => {
                self.error =
                    Some("Input definition changed; restart the viewer to apply it".into());
                self.output = None;
            }
            Err(e) => {
                self.error = Some(e.to_string());
                self.output = None;
            }
        }
    }
    pub fn show(&mut self, ui: &mut egui::Ui, input: &PanelCompositionInput<'_>) -> PanelOutput {
        let generation = input.plugin_generation;
        if self.generation != Some(generation) {
            self.reset();
            self.generation = Some(generation);
        }
        ui.horizontal(|ui| {
            ui.heading(&self.vm.definition().title);
            if ui.small_button("Reload plugin").clicked() {
                self.reload();
            }
        });
        let Some(data) = input.plugin_data.get(self.id.as_str()) else {
            self.output = None;
            ui.label("Waiting for panel query");
            return PanelOutput::default();
        };
        let mut actions = vec![];
        if self.error.is_none()
            && (self.last_key != Some((data.generation, data.revision)) || !self.events.is_empty())
        {
            self.plots = data
                .inputs
                .iter()
                .flat_map(|(input, snapshot)| {
                    snapshot
                        .histories
                        .iter()
                        .map(move |(field, points)| (format!("{input}/{field}"), points))
                })
                .map(|(id, points)| {
                    (
                        id,
                        points
                            .iter()
                            .map(|p| egui_plot::PlotPoint::new(p[0], p[1]))
                            .collect(),
                    )
                })
                .collect();
            let mut update = data.clone();
            update.settings = self.vm.settings().clone();
            update.events = std::mem::take(&mut self.events);
            match self.vm.update(&update) {
                Ok(mut output) => {
                    for command in output.commands.drain(..) {
                        actions.push(ViewerAction::Playback(match command {
                            HostCommand::TogglePlayback => viewer_core::PlaybackCommand::Toggle,
                            HostCommand::Seek { time_ns } => {
                                viewer_core::PlaybackCommand::Seek(viewer_core::ArrivalTime(
                                    time_ns.parse().expect("validated command"),
                                ))
                            }
                        }));
                    }
                    self.output = Some(output);
                }
                Err(error) => {
                    self.output = None;
                    self.error = Some(error.to_string());
                }
            }
            self.last_key = Some((data.generation, data.revision));
        }
        if let Some(error) = &self.error {
            ui.colored_label(egui::Color32::LIGHT_RED, format!("PluginPanel: {error}"));
        }
        if let Some(output) = &self.output {
            draw(
                ui,
                &output.root,
                &self.plots,
                &mut self.events,
                self.id.as_str(),
            );
        }
        PanelOutput {
            actions,
            ..Default::default()
        }
    }
}
fn draw(
    ui: &mut egui::Ui,
    node: &UiNode,
    plots: &BTreeMap<String, Vec<egui_plot::PlotPoint>>,
    events: &mut Vec<PanelEvent>,
    scope: &str,
) {
    match node {
        UiNode::Text { text } => {
            ui.label(text);
        }
        UiNode::Value { label, value, unit } => {
            ui.vertical(|ui| {
                ui.label(label);
                ui.label(egui::RichText::new(format!("{value} {unit}")).size(24.));
            });
        }
        UiNode::Row { children } => {
            ui.horizontal_wrapped(|ui| {
                for child in children {
                    draw(ui, child, plots, events, scope);
                }
            });
        }
        UiNode::Column { children } => {
            ui.vertical(|ui| {
                for child in children {
                    draw(ui, child, plots, events, scope);
                }
            });
        }
        UiNode::Button { id, label } => {
            if ui.button(label).clicked() {
                events.clear();
                events.push(PanelEvent::Click { id: id.clone() });
            }
        }
        UiNode::Plot { title, series } => {
            ui.label(title);
            egui_plot::Plot::new((scope, title))
                .height(190.)
                .show(ui, |plot| {
                    for id in series {
                        if let Some(points) = plots.get(id) {
                            plot.line(egui_plot::Line::new(
                                id,
                                egui_plot::PlotPoints::Borrowed(points),
                            ));
                        }
                    }
                });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "build RobotStatus guest and an alternate guest module first"]
    fn actual_robot_status_reload_replaces_only_guest_and_clears_events() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../");
        let dir = root.join("target/plugin-panel/reload-test");
        std::fs::create_dir_all(&dir).unwrap();
        let primary=root.join("examples/robot-status-panel/target/wasm32-unknown-unknown/release/robot_status_panel.wasm");
        let alternate = root.join("target/plugin-panel/alternate-guest.wasm");
        assert_ne!(
            std::fs::read(&primary).unwrap(),
            std::fs::read(&alternate).unwrap()
        );
        std::fs::copy(&primary, dir.join("panel.wasm")).unwrap();
        std::fs::write(dir.join("panel.json"), r#"{"wasm":"panel.wasm"}"#).unwrap();
        let node = PanelNode {
            id: PanelId::new("reload-test").unwrap(),
            panel_type: "plugin".into(),
            config_version: 1,
            title: None,
            config: serde_json::json!({"manifest":dir.join("panel.json")}),
        };
        let NativePanel::Plugin(mut panel) = PluginPanel::create(&node) else {
            panic!("load sample panel")
        };
        let definition = panel.vm.definition().clone();
        let mut input = viewer_extension::panel::PanelUpdate {
            generation: 1,
            revision: 1,
            cursor_ns: "100".into(),
            source_start_ns: "0".into(),
            inputs: BTreeMap::from([(
                "status".into(),
                viewer_extension::panel::PanelInputSnapshot {
                    status: viewer_extension::panel::InputStatus::Ready,
                    log_time_ns: None,
                    values: BTreeMap::new(),
                    histories: BTreeMap::new(),
                    errors: vec![],
                },
            )]),
            events: vec![],
            settings: serde_json::Value::Null,
        };
        let before = panel.vm.update(&input).unwrap();
        panel.output = Some(before.clone());
        panel.events.push(PanelEvent::Click {
            id: "toggle".into(),
        });
        panel.last_key = Some((1, 1));
        std::fs::copy(&alternate, dir.join("panel.wasm")).unwrap();
        panel.reload();
        assert!(panel.error.is_none());
        assert!(panel.output.is_none());
        assert!(panel.events.is_empty());
        assert!(panel.last_key.is_none());
        assert_eq!(panel.vm.definition(), &definition);
        let after = panel.vm.update(&input).unwrap();
        assert_ne!(before, after);
        assert!(
            serde_json::to_string(&after)
                .unwrap()
                .contains("alternate guest")
        );
        // A bad replacement clears visible output without changing the playback/query definition.
        panel.output = Some(after);
        std::fs::write(dir.join("panel.wasm"), b"invalid Wasm").unwrap();
        panel.reload();
        assert!(panel.error.is_some());
        assert!(panel.output.is_none());
        assert_eq!(panel.vm.definition(), &definition);
        panel.reset();
        input.inputs.clear();
        assert!(panel.vm.update(&input).unwrap().commands.is_empty());
    }
}
