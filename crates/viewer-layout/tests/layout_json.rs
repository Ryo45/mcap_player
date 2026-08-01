use serde_json::json;
use viewer_layout::{LayoutDocument, LayoutNode, PanelId, SplitDirection};

const DEFAULT_LAYOUT: &str = include_str!("../../../config/layouts/native_default.json");

#[test]
fn bundled_default_layout_round_trips_and_validates() {
    let document = LayoutDocument::from_json(DEFAULT_LAYOUT).unwrap();
    document.validate().unwrap();
    let pretty = document.to_json_pretty().unwrap();
    let round_trip = LayoutDocument::from_json(&pretty).unwrap();
    assert_eq!(round_trip, document);
}

#[test]
fn reads_unknown_fields_and_defaults_missing_config() {
    let document = LayoutDocument::from_json(
        &json!({
            "schemaVersion": 1,
            "futureDocumentField": true,
            "root": {
                "kind": "panel",
                "id": "camera-main",
                "panelType": "camera",
                "configVersion": 1,
                "futurePanelField": 42
            }
        })
        .to_string(),
    )
    .unwrap();
    document.validate().unwrap();
    let LayoutNode::Panel(panel) = document.root else {
        panic!("expected panel")
    };
    assert!(panel.config.is_null());
}

#[test]
fn bundled_default_contains_row_and_column_splits() {
    let document = LayoutDocument::from_json(DEFAULT_LAYOUT).unwrap();
    let LayoutNode::Split {
        direction,
        children,
    } = document.root
    else {
        panic!("expected root split")
    };
    assert_eq!(direction, SplitDirection::Column);
    assert_eq!(children.len(), 3);
    let LayoutNode::Split { direction, .. } = &children[0].node else {
        panic!("expected top split")
    };
    assert_eq!(*direction, SplitDirection::Row);
}

#[test]
fn bundled_default_panel_ids_are_in_expected_layout_order() {
    fn collect_ids(node: &LayoutNode, ids: &mut Vec<PanelId>) {
        match node {
            LayoutNode::Split {
                direction: _,
                children,
            } => {
                for child in children {
                    collect_ids(&child.node, ids);
                }
            }
            LayoutNode::Panel(panel) => ids.push(panel.id.clone()),
        }
    }

    let document = LayoutDocument::from_json(DEFAULT_LAYOUT).unwrap();
    let mut ids = Vec::new();
    collect_ids(&document.root, &mut ids);
    assert_eq!(
        ids.iter().map(PanelId::as_str).collect::<Vec<_>>(),
        ["camera-main", "bev-main", "speed-main", "scene-main"]
    );
}
