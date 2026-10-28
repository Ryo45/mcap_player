#![cfg(not(target_arch = "wasm32"))]
use viewer_core::{ArrivalTime, Detection3DFrame, decode_detection3d_array};
use viewer_extension::{
    ExtensionInput, VisualizationExtension, WasmExtension,
    wire::{self, DecodedInput},
};
fn frame() -> Detection3DFrame {
    Detection3DFrame {
        arrival_time: ArrivalTime(99),
        message: decode_detection3d_array(include_bytes!(
            "../../../tests/fixtures/ros2-detection3d/jazzy.cdr"
        ))
        .unwrap(),
    }
}
#[test]
fn sdk_wire_preserves_all_fields_and_rejects_truncation() {
    let frame = frame();
    let bytes = wire::encode_input(ExtensionInput::Detections(&frame)).unwrap();
    let DecodedInput::Detections(decoded) = wire::decode_input(&bytes).unwrap() else {
        panic!("wrong type");
    };
    assert_eq!(decoded, frame);
    for n in 0..bytes.len() {
        assert!(wire::decode_input(&bytes[..n]).is_err());
    }
    let mut extra = bytes;
    extra.push(0);
    assert!(wire::decode_input(&extra).is_err());
    let mut oversized = frame;
    oversized.message.detections[0].id = "x".repeat(257);
    assert!(wire::encode_input(ExtensionInput::Detections(&oversized)).is_err());
}
#[test]
#[ignore = "build independent examples/detection-boxes guest first"]
fn actual_detection_guest_uses_bbox_ids_and_classification() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/detection-boxes/extension.json");
    let mut guest = WasmExtension::load(&manifest).unwrap();
    let mut frame = frame();
    frame.message.detections[1].id = "track-second".into();
    let first = guest.update(ExtensionInput::Detections(&frame)).unwrap();
    assert_eq!(first.boxes().len(), 2);
    let box1 = first.boxes().iter().find(|b| b.center[0] == 1.25).unwrap();
    assert_eq!(box1.size, [0.4, 0.3, 1.]);
    assert_eq!(box1.color, [1., 0.08, 0.25]);
    assert!((f64::from(box1.orientation[2]) - 0.3826834323650898).abs() < 1e-6);
    let id = box1.id;
    frame.message.detections.reverse();
    assert_eq!(
        guest.update(ExtensionInput::Detections(&frame)).unwrap(),
        first
    );
    let d = frame
        .message
        .detections
        .iter_mut()
        .find(|d| d.id == "track-α-42")
        .unwrap();
    d.results[1].hypothesis.score = 0.99;
    let changed = guest.update(ExtensionInput::Detections(&frame)).unwrap();
    assert_eq!(
        changed.boxes().iter().find(|b| b.id == id).unwrap().color,
        [0.2, 1., 0.15]
    );
    frame.message.detections[0].header.frame_id = "different".into();
    assert!(guest.update(ExtensionInput::Detections(&frame)).is_err());
    guest.reset().unwrap();
    frame.message.detections.clear();
    assert!(
        guest
            .update(ExtensionInput::Detections(&frame))
            .unwrap()
            .boxes()
            .is_empty()
    );
}

#[test]
#[ignore = "manual release measurement; build detection-boxes first"]
fn measure_detection_guest_updates() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/detection-boxes/extension.json");
    let mut guest = WasmExtension::load(&manifest).unwrap();
    let mut input = frame();
    let prototype = input.message.detections[0].clone();
    input.message.detections = (0..8)
        .map(|i| {
            let mut d = prototype.clone();
            d.id = format!("track-{i}");
            d.bbox.center.position[0] += i as f64;
            d
        })
        .collect();
    let bytes = wire::encode_input(ExtensionInput::Detections(&input))
        .unwrap()
        .len();
    let mut times = Vec::new();
    for _ in 0..1000 {
        let start = std::time::Instant::now();
        let overlay = guest.update(ExtensionInput::Detections(&input)).unwrap();
        times.push(start.elapsed().as_secs_f64() * 1000.);
        assert_eq!(overlay.boxes().len(), 8);
        std::hint::black_box(overlay);
    }
    times.sort_by(f64::total_cmp);
    println!(
        "detection_guest updates=1000 detections=8 hypotheses_each=2 input_bytes={bytes} output_bytes=480 median_ms={:.6} p95_ms={:.6} max_ms={:.6}",
        times[500], times[950], times[999]
    );
}
