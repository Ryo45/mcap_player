#![cfg(not(target_arch = "wasm32"))]
use std::collections::BTreeMap;
use viewer_extension::{
    WasmPanel,
    panel::{
        HostCommand, InputStatus, PanelDefinition, PanelEvent, PanelInputDefinition,
        PanelInputSnapshot, PanelOutput, PanelUpdate, PanelValue, UiNode,
    },
};
fn definition() -> PanelDefinition {
    PanelDefinition {
        api_version: 2,
        id: "test".into(),
        title: "Test".into(),
        inputs: vec![PanelInputDefinition {
            id: "status".into(),
            topic: "/custom".into(),
            fields: vec!["value".into()],
            plots: vec!["value".into()],
            history_seconds: 10,
            max_points: 128,
        }],
    }
}
fn update() -> PanelUpdate {
    PanelUpdate {
        generation: 1,
        revision: 1,
        cursor_ns: "100".into(),
        source_start_ns: "0".into(),
        inputs: BTreeMap::from([(
            "status".into(),
            PanelInputSnapshot {
                status: InputStatus::Ready,
                log_time_ns: Some("100".into()),
                values: BTreeMap::new(),
                histories: BTreeMap::new(),
                errors: vec![],
            },
        )]),
        events: vec![],
        settings: serde_json::Value::Null,
    }
}
fn guest(body: &str, output: &str) -> WasmPanel {
    let definition = serde_json::to_vec(&definition()).unwrap();
    let encode = |data: &[u8]| {
        data.iter()
            .map(|b| format!("\\{b:02x}"))
            .collect::<String>()
    };
    let wat = format!(
        r#"(module (memory (export "memory") 4)
 (data (i32.const 150000) "{}") (data (i32.const 160000) "{}")
 (global $ptr (mut i32) (i32.const 150000))
 (func (export "panel_api_version") (result i32) i32.const 2)
 (func (export "init")) (func (export "input_ptr") (result i32) i32.const 0)
 (func (export "output_ptr") (result i32) global.get $ptr)
 (func (export "define") (param i32) (result i32) i32.const {})
 (func (export "update") (param i32) (result i32) i32.const 160000 global.set $ptr {body}))"#,
        encode(&definition),
        encode(output.as_bytes()),
        definition.len()
    );
    WasmPanel::from_bytes(&wat::parse_str(wat).unwrap(), serde_json::Value::Null).unwrap()
}
#[test]
fn bounded_output_traps_and_command_permissions() {
    let output = r#"{"root":{"type":"text","text":"hello"},"commands":[]}"#;
    let mut panel = guest(&format!("i32.const {}", output.len()), output);
    assert_eq!(
        panel.update(&update()).unwrap().root,
        UiNode::Text {
            text: "hello".into()
        }
    );
    panel.reset().unwrap();
    assert!(panel.update(&update()).is_ok());
    for code in [
        "unreachable",
        "(loop $forever br $forever) i32.const 0",
        "i32.const 65537",
        "i32.const -1",
    ] {
        assert!(guest(code, "").update(&update()).is_err());
    }
    assert!(guest("i32.const 1", "!").update(&update()).is_err());
    let output = PanelOutput {
        root: UiNode::Text {
            text: "click".into(),
        },
        commands: vec![HostCommand::TogglePlayback],
    };
    assert!(output.validate(&definition(), false).is_err());
    assert!(output.validate(&definition(), true).is_ok());
    assert!(
        PanelOutput {
            root: UiNode::Plot {
                title: "x".into(),
                series: vec!["status/unknown".into()]
            },
            commands: vec![]
        }
        .validate(&definition(), true)
        .is_err()
    );
    let mut root = UiNode::Text {
        text: "leaf".into(),
    };
    for _ in 0..10 {
        root = UiNode::Column {
            children: vec![root],
        };
    }
    assert!(
        PanelOutput {
            root,
            commands: vec![]
        }
        .validate(&definition(), false)
        .is_err()
    );
}
#[test]
#[ignore = "build examples/robot-status-panel Wasm first"]
fn actual_robot_status_panel_guest() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/robot-status-panel/panel.json");
    let mut panel = WasmPanel::load(&manifest).unwrap();
    assert_eq!(panel.definition().inputs[0].topic, "/robot/status");
    let mut input = update();
    input
        .inputs
        .get_mut("status")
        .unwrap()
        .values
        .insert("state".into(), PanelValue::Text("working".into()));
    input.inputs.get_mut("status").unwrap().values.insert(
        "sequence".into(),
        PanelValue::Unsigned("9007199254740993".into()),
    );
    input
        .inputs
        .get_mut("status")
        .unwrap()
        .histories
        .insert("battery".into(), vec![[0., 90.], [1., 89.]]);
    let output = panel.update(&input).unwrap();
    let text = serde_json::to_string(&output).unwrap();
    assert!(text.contains("working"));
    assert!(text.contains("9007199254740993"));
    assert!(text.contains("2 reduced samples"));
    input.events.push(PanelEvent::Click {
        id: "toggle".into(),
    });
    assert_eq!(
        panel.update(&input).unwrap().commands,
        vec![HostCommand::TogglePlayback]
    );
    panel.reset().unwrap();
    input.events.clear();
    input.inputs.get_mut("status").unwrap().status = InputStatus::MissingTopic;
    input.inputs.get_mut("status").unwrap().values.clear();
    input.inputs.get_mut("status").unwrap().histories.clear();
    let text = serde_json::to_string(&panel.update(&input).unwrap()).unwrap();
    assert!(!text.contains("working"));
    assert!(!text.contains("9007199254740993"));
    assert!(text.contains("MissingTopic"));
}

#[test]
#[ignore = "manual bounded-input timing; build RobotStatus sample first"]
fn actual_robot_status_panel_update_timing() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/robot-status-panel/panel.json");
    let mut panel = WasmPanel::load(&manifest).unwrap();
    let mut input = update();
    for field in ["battery", "temperature"] {
        input
            .inputs
            .get_mut("status")
            .unwrap()
            .values
            .insert(field.into(), PanelValue::Number(42.));
        input.inputs.get_mut("status").unwrap().histories.insert(
            field.into(),
            (0..128)
                .map(|i| [i as f64 * 0.04, 42. + (i as f64).sin()])
                .collect(),
        );
    }
    input
        .inputs
        .get_mut("status")
        .unwrap()
        .values
        .insert("state".into(), PanelValue::Text("working".into()));
    input.inputs.get_mut("status").unwrap().values.insert(
        "sequence".into(),
        PanelValue::Unsigned("9007199254740993".into()),
    );
    input
        .inputs
        .get_mut("status")
        .unwrap()
        .values
        .insert("enabled".into(), PanelValue::Bool(true));
    let mut times = Vec::new();
    for revision in 0..200 {
        input.revision = revision;
        let start = std::time::Instant::now();
        panel.update(&input).unwrap();
        times.push(start.elapsed().as_secs_f64() * 1000.);
    }
    times.sort_by(f64::total_cmp);
    println!(
        "Panel update (256 history points, includes JSON/validation/wasmi): median={:.3}ms p95={:.3}ms max={:.3}ms input={}bytes",
        times[100],
        times[190],
        times[199],
        serde_json::to_vec(&input).unwrap().len()
    );
}

#[test]
fn multiple_input_definitions_and_snapshot_bounds() {
    let mut panel = definition();
    let mut second = panel.inputs[0].clone();
    second.id = "tracks".into();
    second.topic = "/tracks".into();
    second.fields = vec!["objects[].id".into()];
    second.plots.clear();
    panel.inputs.push(second);
    panel.validate().unwrap();
    assert!(panel.has_series("status/value"));
    assert!(!panel.has_series("tracks/value"));
    assert!(!panel.has_series("value"));
    panel.inputs[1].id = "status".into();
    assert!(panel.validate().is_err());
    panel.inputs[1].id = "tracks".into();
    panel.inputs[1].plots = vec!["objects[].score".into()];
    assert!(panel.validate().is_err());
    let mut data = update();
    let input = data.inputs.get_mut("status").unwrap();
    input
        .values
        .insert("array".into(), PanelValue::Array(vec![]));
    input.validate_size().unwrap();
    input.values.insert(
        "array".into(),
        PanelValue::Array(
            (0..128)
                .map(|_| PanelValue::Text("x".repeat(256)))
                .collect(),
        ),
    );
    assert!(input.validate_size().is_err());
}
