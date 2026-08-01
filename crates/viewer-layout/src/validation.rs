use crate::{
    CURRENT_LAYOUT_SCHEMA_VERSION, LayoutDocument, LayoutNode, LayoutValidationError, PanelId,
};
use std::collections::BTreeSet;

pub const MAX_LAYOUT_DEPTH: usize = 64;
pub const MAX_PANEL_COUNT: usize = 256;

pub(crate) fn validate(document: &LayoutDocument) -> Result<(), LayoutValidationError> {
    if document.schema_version != CURRENT_LAYOUT_SCHEMA_VERSION {
        return Err(LayoutValidationError::new(format!(
            "unsupported schema version: {}",
            document.schema_version
        )));
    }
    let mut panel_ids = BTreeSet::new();
    let mut panel_count = 0;
    validate_node(&document.root, "root", 1, &mut panel_count, &mut panel_ids)
}

fn validate_node(
    node: &LayoutNode,
    path: &str,
    depth: usize,
    panel_count: &mut usize,
    panel_ids: &mut BTreeSet<PanelId>,
) -> Result<(), LayoutValidationError> {
    if depth > MAX_LAYOUT_DEPTH {
        return Err(LayoutValidationError::new(format!(
            "layout nesting exceeds {MAX_LAYOUT_DEPTH} at {path}"
        )));
    }
    match node {
        LayoutNode::Split {
            direction: _,
            children,
        } => {
            if children.len() < 2 {
                return Err(LayoutValidationError::new(format!(
                    "{path}.children must contain at least 2 nodes"
                )));
            }
            let mut total = 0.0_f32;
            for (index, child) in children.iter().enumerate() {
                let weight_path = format!("{path}.children[{index}].weight");
                if !child.weight.is_finite() {
                    return Err(LayoutValidationError::new(format!(
                        "{weight_path} must be finite"
                    )));
                }
                if child.weight <= 0.0 {
                    return Err(LayoutValidationError::new(format!(
                        "{weight_path} must be positive"
                    )));
                }
                total += child.weight;
                validate_node(
                    &child.node,
                    &format!("{path}.children[{index}].node"),
                    depth + 1,
                    panel_count,
                    panel_ids,
                )?;
            }
            if !total.is_finite() || total <= 0.0 {
                return Err(LayoutValidationError::new(format!(
                    "{path}.children weight sum must be finite and positive"
                )));
            }
        }
        LayoutNode::Panel(panel) => {
            *panel_count += 1;
            if *panel_count > MAX_PANEL_COUNT {
                return Err(LayoutValidationError::new(format!(
                    "layout panel count exceeds {MAX_PANEL_COUNT} at {path}"
                )));
            }
            if panel.id.as_str().trim().is_empty() {
                return Err(LayoutValidationError::new(format!(
                    "{path}.id must not be empty"
                )));
            }
            if !panel_ids.insert(panel.id.clone()) {
                return Err(LayoutValidationError::new(format!(
                    "duplicate panel id: {}",
                    panel.id
                )));
            }
            if panel.panel_type.trim().is_empty() {
                return Err(LayoutValidationError::new(format!(
                    "{path}.panelType must not be empty"
                )));
            }
            if panel.config_version < 1 {
                return Err(LayoutValidationError::new(format!(
                    "{path}.configVersion must be at least 1"
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PanelNode, SplitChild, SplitDirection};
    use serde_json::json;

    fn panel(id: &str) -> LayoutNode {
        LayoutNode::Panel(PanelNode {
            id: PanelId::new(id).unwrap(),
            panel_type: "camera".to_owned(),
            config_version: 1,
            title: None,
            config: json!({}),
        })
    }

    fn document(root: LayoutNode) -> LayoutDocument {
        LayoutDocument {
            schema_version: CURRENT_LAYOUT_SCHEMA_VERSION,
            root,
        }
    }

    fn split(direction: SplitDirection, weights: &[f32]) -> LayoutNode {
        LayoutNode::Split {
            direction,
            children: weights
                .iter()
                .enumerate()
                .map(|(index, weight)| SplitChild {
                    weight: *weight,
                    node: panel(&format!("panel-{index}")),
                })
                .collect(),
        }
    }

    #[test]
    fn accepts_both_directions_and_non_normalized_positive_weights() {
        document(split(SplitDirection::Row, &[2.0, 1.0]))
            .validate()
            .unwrap();
        document(split(SplitDirection::Column, &[3.0, 2.0]))
            .validate()
            .unwrap();
    }

    #[test]
    fn rejects_unsupported_schema_versions() {
        for schema_version in [0, CURRENT_LAYOUT_SCHEMA_VERSION + 1] {
            let document = LayoutDocument {
                schema_version,
                root: panel("main"),
            };
            assert!(
                document
                    .validate()
                    .unwrap_err()
                    .to_string()
                    .contains("unsupported schema version")
            );
        }
    }

    #[test]
    fn rejects_invalid_split_weights_and_too_few_children() {
        for weight in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            let error = document(split(SplitDirection::Row, &[weight, 1.0]))
                .validate()
                .unwrap_err()
                .to_string();
            assert!(error.contains("weight"));
        }
        let error = document(split(SplitDirection::Row, &[1.0]))
            .validate()
            .unwrap_err()
            .to_string();
        assert!(error.contains("at least 2"));
    }

    #[test]
    fn rejects_duplicate_panel_ids() {
        let root = LayoutNode::Split {
            direction: SplitDirection::Row,
            children: vec![
                SplitChild {
                    weight: 1.0,
                    node: panel("duplicate"),
                },
                SplitChild {
                    weight: 1.0,
                    node: panel("duplicate"),
                },
            ],
        };
        assert_eq!(
            document(root).validate().unwrap_err().to_string(),
            "duplicate panel id: duplicate"
        );
    }

    #[test]
    fn rejects_invalid_panel_fields() {
        let invalid_id = LayoutDocument::from_json(
            r#"{
                "schemaVersion": 1,
                "root": {
                    "kind": "panel",
                    "id": " ",
                    "panelType": "camera",
                    "configVersion": 1
                }
            }"#,
        )
        .unwrap();
        assert!(
            invalid_id
                .validate()
                .unwrap_err()
                .to_string()
                .contains(".id")
        );

        let mut empty_type = match panel("empty-type") {
            LayoutNode::Panel(panel) => panel,
            LayoutNode::Split { .. } => unreachable!(),
        };
        empty_type.panel_type = " ".to_owned();
        assert!(
            document(LayoutNode::Panel(empty_type))
                .validate()
                .unwrap_err()
                .to_string()
                .contains("panelType")
        );

        let mut zero_version = match panel("zero-version") {
            LayoutNode::Panel(panel) => panel,
            LayoutNode::Split { .. } => unreachable!(),
        };
        zero_version.config_version = 0;
        assert!(
            document(LayoutNode::Panel(zero_version))
                .validate()
                .unwrap_err()
                .to_string()
                .contains("configVersion")
        );
    }

    #[test]
    fn rejects_excessive_depth_and_panel_count() {
        let mut deep = panel("deep");
        for depth in 0..MAX_LAYOUT_DEPTH {
            deep = LayoutNode::Split {
                direction: SplitDirection::Column,
                children: vec![
                    SplitChild {
                        weight: 1.0,
                        node: deep,
                    },
                    SplitChild {
                        weight: 1.0,
                        node: panel(&format!("depth-{depth}")),
                    },
                ],
            };
        }
        assert!(
            document(deep)
                .validate()
                .unwrap_err()
                .to_string()
                .contains("nesting exceeds")
        );

        let too_many = LayoutNode::Split {
            direction: SplitDirection::Row,
            children: (0..=MAX_PANEL_COUNT)
                .map(|index| SplitChild {
                    weight: 1.0,
                    node: panel(&format!("many-{index}")),
                })
                .collect(),
        };
        assert!(
            document(too_many)
                .validate()
                .unwrap_err()
                .to_string()
                .contains("panel count exceeds")
        );
    }
}
