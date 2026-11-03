#![cfg(not(target_arch = "wasm32"))]

use viewer_core::{ArrivalTime, MeasurementTime, PathFrame};
use viewer_extension::{
    ExtensionDescriptor, ExtensionInput, VisualizationExtension, WasmExtension,
};

fn descriptor() -> ExtensionDescriptor {
    ExtensionDescriptor {
        id: "test".into(),
        api_version: 3,
        topic: "/planning/path".into(),
        schema: "nav_msgs/msg/Path".into(),
        input: "path_positions_v1".into(),
    }
}
fn path() -> PathFrame {
    PathFrame {
        arrival_time: ArrivalTime(20),
        measurement_time: MeasurementTime(10),
        frame_id: "odom".into(),
        points: vec![[1., 2., 3.]],
    }
}
fn guest(body: &str) -> WasmExtension {
    let wat = format!(
        r#"(module
      (memory (export "memory") 1)
      (func (export "api_version") (result i32) i32.const 3)
      (func (export "init"))
      (func (export "input_ptr") (result i32) i32.const 0)
      (func (export "output_ptr") (result i32) i32.const 32768)
      (func (export "update") (param i32) (result i32) {body}))"#
    );
    WasmExtension::from_bytes(descriptor(), &wat::parse_str(wat).unwrap()).unwrap()
}
#[test]
fn empty_output_and_fresh_reset() {
    let mut extension = guest("i32.const 0");
    assert!(
        extension
            .update(ExtensionInput::Path(&path()))
            .unwrap()
            .lines()
            .is_empty()
    );
    extension.reset().unwrap();
    assert!(
        extension
            .update(ExtensionInput::Path(&path()))
            .unwrap()
            .lines()
            .is_empty()
    );
}
#[test]
fn traps_budget_and_output_validation_are_errors() {
    for code in [
        "unreachable",
        "(loop $forever br $forever) i32.const 0",
        "i32.const 257",
        "i32.const -1",
        "i32.const 32776 f32.const nan f32.store i32.const 1",
    ] {
        assert!(
            guest(code).update(ExtensionInput::Path(&path())).is_err(),
            "{code}"
        );
    }
    assert!(
        guest("memory.size drop i32.const 129 memory.grow drop i32.const 0")
            .update(ExtensionInput::Path(&path()))
            .is_err()
    );
    let mut extension = guest("i32.const 0");
    let mut oversized = path();
    oversized.points = vec![[0.; 3]; 1025];
    assert!(extension.update(ExtensionInput::Path(&oversized)).is_err());
}
#[test]
fn imports_start_and_initial_excess_memory_rejected() {
    for wat in [
        r#"(module (import "wasi_snapshot_preview1" "proc_exit" (func (param i32))))"#,
        "(module (memory 129))",
        "(module (func $start) (start $start))",
    ] {
        assert!(WasmExtension::from_bytes(descriptor(), &wat::parse_str(wat).unwrap()).is_err());
    }
}
#[test]
#[ignore = "build independent examples/path-boxes Wasm first; see docs/scene_visualization_extensions.md"]
fn independent_rust_guest_produces_boxes_and_recovers_after_reset() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/path-boxes/extension.json");
    let mut extension = WasmExtension::load(&manifest).unwrap();
    let output = extension.update(ExtensionInput::Path(&path())).unwrap();
    assert_eq!(output.boxes().len(), 1);
    assert!(
        output
            .boxes()
            .iter()
            .all(|line| line.center == [1., 2., 3.25] && line.size == [0.3, 0.3, 0.5])
    );
    assert_eq!(output.boxes()[0].color, [1., 0.08, 0.25]);
    extension.reset().unwrap();
    assert_eq!(
        extension.update(ExtensionInput::Path(&path())).unwrap(),
        output
    );
    let mut moved = path();
    moved.points[0][0] += 10.;
    assert_ne!(
        extension.update(ExtensionInput::Path(&moved)).unwrap(),
        output
    );
}

#[test]
fn malformed_abi_and_memory_ranges_are_rejected() {
    let basic = r#"(module
      (memory (export "memory") 1)
      (func (export "api_version") (result i32) i32.const 3)
      (func (export "init"))
      (func (export "input_ptr") (result i32) i32.const 0)
      (func (export "output_ptr") (result i32) i32.const 32768)
      (func (export "update") (param i32) (result i32) i32.const 2))"#;
    for modified in [
        basic.replace("i32.const 3)", "i32.const 1)"),
        basic.replace(
            "(func (export \"init\"))",
            "(func (export \"init\") (loop $x br $x))",
        ),
    ] {
        assert!(
            WasmExtension::from_bytes(descriptor(), &wat::parse_str(modified).unwrap()).is_err()
        );
    }
    for modified in [
        basic.replace("i32.const 0)", "i32.const -1)"),
        basic.replace("i32.const 32768)", "i32.const 65530)"),
    ] {
        let mut guest =
            WasmExtension::from_bytes(descriptor(), &wat::parse_str(modified).unwrap()).unwrap();
        assert!(guest.update(ExtensionInput::Path(&path())).is_err());
    }
    let mut descriptor = descriptor();
    descriptor.api_version = 1;
    assert!(WasmExtension::from_bytes(descriptor, &wat::parse_str(basic).unwrap()).is_err());
}

#[test]
#[ignore = "manual release timing; requires independent path-boxes guest build"]
fn measure_independent_guest_updates() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/path-boxes/extension.json");
    let mut guest = WasmExtension::load(&manifest).unwrap();
    let mut frame = path();
    frame.points = (0..1024).map(|i| [i as f64 * 0.01, 0., 0.]).collect();
    let mut durations = Vec::new();
    for _ in 0..1000 {
        let started = std::time::Instant::now();
        let output = guest.update(ExtensionInput::Path(&frame)).unwrap();
        assert_eq!(output.boxes().len(), 32);
        durations.push(started.elapsed().as_secs_f64() * 1000.);
        std::hint::black_box(output);
    }
    durations.sort_by(f64::total_cmp);
    println!(
        "guest_update input_points=1024 output_boxes=32 n=1000 median_ms={:.6} p95_ms={:.6} max_ms={:.6}",
        durations[500], durations[950], durations[999]
    );
}

#[test]
fn box_wire_preserves_64bit_identity_and_rejects_duplicates_and_bad_dimensions() {
    let valid = "i32.const 32768 i64.const 4294967297 i64.store\n i32.const 32800 f32.const 1 f32.store\n i32.const 32804 f32.const 2 f32.store\n i32.const 32808 f32.const 3 f32.store\n i32.const 32812 f32.const 4 f32.store\n i32.const 32816 f32.const 1 f32.store";
    let output = guest(&format!("{valid} i32.const 1"))
        .update(ExtensionInput::Path(&path()))
        .unwrap();
    assert_eq!(output.boxes()[0].id.0, 4294967297);
    assert_eq!(output.boxes()[0].size, [2., 3., 4.]);
    assert!(
        guest(&format!(
            "{valid} i32.const 32804 f32.const -1 f32.store i32.const 1"
        ))
        .update(ExtensionInput::Path(&path()))
        .is_err()
    );
    assert!(
        guest(&format!(
            "{valid} i32.const 32828 i32.const 32768 i32.const 60 memory.copy i32.const 2"
        ))
        .update(ExtensionInput::Path(&path()))
        .is_err()
    );
}
