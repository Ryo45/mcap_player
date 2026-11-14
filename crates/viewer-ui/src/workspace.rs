//! Layout-owned panel instances. Hosts supply construction for their concrete panel enum.
use std::collections::{BTreeMap, BTreeSet};
use viewer_layout::{LayoutDocument, LayoutNode, PanelId, PanelNode};

pub struct PanelStore<P> {
    panels: BTreeMap<PanelId, P>,
}
impl<P> Default for PanelStore<P> {
    fn default() -> Self {
        Self {
            panels: BTreeMap::new(),
        }
    }
}
impl<P> PanelStore<P> {
    pub fn from_layout(
        document: &LayoutDocument,
        mut create: impl FnMut(&PanelNode) -> P,
        kind: impl Fn(&P) -> &'static str,
        mut placeholder: impl FnMut(&PanelNode, String) -> P,
    ) -> (Self, Vec<String>) {
        let mut store = Self::default();
        let mut warnings = Vec::new();
        let mut singletons = BTreeSet::new();
        visit(&document.root, &mut |node| {
            if store.panels.contains_key(&node.id) {
                warnings.push(format!(
                    "duplicate runtime panel id {} was ignored",
                    node.id
                ));
                return;
            }
            let mut panel = create(node);
            let name = kind(&panel);
            if matches!(name, "bev" | "scene-3d") && !singletons.insert(name.to_owned()) {
                let error = format!(
                    "panel {} is a duplicate {} instance and was replaced with a placeholder",
                    node.id, name
                );
                warnings.push(error.clone());
                panel = placeholder(node, error);
            }
            store.panels.insert(node.id.clone(), panel);
        });
        (store, warnings)
    }
    pub fn get(&self, id: &PanelId) -> Option<&P> {
        self.panels.get(id)
    }
    pub fn get_mut(&mut self, id: &PanelId) -> Option<&mut P> {
        self.panels.get_mut(id)
    }
    pub fn values(&self) -> impl Iterator<Item = &P> {
        self.panels.values()
    }
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut P> {
        self.panels.values_mut()
    }
    pub fn iter(&self) -> impl Iterator<Item = (&PanelId, &P)> {
        self.panels.iter()
    }
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&PanelId, &mut P)> {
        self.panels.iter_mut()
    }
    pub fn len(&self) -> usize {
        self.panels.len()
    }
    pub fn is_empty(&self) -> bool {
        self.panels.is_empty()
    }
}
fn visit(node: &LayoutNode, visitor: &mut impl FnMut(&PanelNode)) {
    match node {
        LayoutNode::Panel(node) => visitor(node),
        LayoutNode::Split { children, .. } => {
            for child in children {
                visit(&child.node, visitor)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use viewer_layout::{SplitChild, SplitDirection};

    fn node(id: &str, kind: &str) -> LayoutNode {
        LayoutNode::Panel(PanelNode {
            id: PanelId::new(id).unwrap(),
            panel_type: kind.into(),
            config_version: 1,
            title: None,
            config: json!({}),
        })
    }
    #[test]
    fn web_monitor_uses_the_common_factory_and_validated_layout() {
        let document =
            LayoutDocument::from_json(include_str!("../../../config/layouts/web_monitor.json"))
                .unwrap();
        document.validate().unwrap();
        let (store, warnings) = PanelStore::from_layout(
            &document,
            |node| crate::panel::Panel::create(node).unwrap().unwrap(),
            crate::panel::Panel::kind_name,
            |node, error| {
                crate::panel::Panel::Placeholder(
                    crate::placeholder::PlaceholderPanel::invalid_config(node, error),
                )
            },
        );
        assert_eq!(store.len(), 3);
        assert!(warnings.is_empty());
    }

    #[test]
    fn both_hosts_apply_the_same_singleton_and_duplicate_identity_policy() {
        let document = LayoutDocument {
            schema_version: 1,
            root: LayoutNode::Split {
                direction: SplitDirection::Row,
                children: [
                    node("first", "scene-3d"),
                    node("second", "scene-3d"),
                    node("first", "camera"),
                ]
                .into_iter()
                .map(|node| SplitChild { weight: 1.0, node })
                .collect(),
            },
        };
        let (store, warnings) = PanelStore::from_layout(
            &document,
            |node| {
                if node.panel_type == "scene-3d" {
                    "scene-3d"
                } else {
                    "camera"
                }
            },
            |kind| *kind,
            |_, _| "placeholder",
        );
        assert_eq!(store.len(), 2);
        assert_eq!(
            store.get(&PanelId::new("first").unwrap()),
            Some(&"scene-3d")
        );
        assert_eq!(
            store.get(&PanelId::new("second").unwrap()),
            Some(&"placeholder")
        );
        assert_eq!(warnings.len(), 2);
    }
}
