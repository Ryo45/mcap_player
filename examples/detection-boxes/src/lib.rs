//! Typed ROS detection input from the shared guest SDK. Never parses CDR or touches a GPU.
use viewer_core::{SceneBox, SceneObjectId, SceneOverlay};
use viewer_extension::wire::{self, DecodedInput};
const INPUT_BYTES: usize = viewer_extension::MAX_INPUT_BYTES;
const OUTPUT_BYTES: usize = 256 * wire::BOX_BYTES;
static mut INPUT: [u8; INPUT_BYTES] = [0; INPUT_BYTES];
static mut OUTPUT: [u8; OUTPUT_BYTES] = [0; OUTPUT_BYTES];
#[unsafe(no_mangle)]
pub extern "C" fn api_version() -> u32 {
    viewer_extension::API_VERSION
}
#[unsafe(no_mangle)]
pub extern "C" fn init() {}
#[unsafe(no_mangle)]
pub extern "C" fn input_ptr() -> *mut u8 {
    &raw mut INPUT as *mut u8
}
#[unsafe(no_mangle)]
pub extern "C" fn output_ptr() -> *mut u8 {
    &raw mut OUTPUT as *mut u8
}

// A demo policy, not a lossless replacement for the ROS string ID (which remains in input).
fn object_id(id: &str, index: usize) -> SceneObjectId {
    if id.is_empty() {
        return SceneObjectId(index as u64);
    }
    let hash = id.bytes().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    SceneObjectId(hash | (1 << 63))
}
#[unsafe(no_mangle)]
pub extern "C" fn update(bytes: u32) -> u32 {
    if bytes as usize > INPUT_BYTES {
        return u32::MAX;
    }
    // SAFETY: bytes is bounded by this instance's scratch array; exports run serially.
    let input = unsafe { std::slice::from_raw_parts(input_ptr(), bytes as usize) };
    let Ok(DecodedInput::Detections(frame)) = wire::decode_input(input) else {
        return u32::MAX;
    };
    let boxes = frame
        .message
        .detections
        .iter()
        .enumerate()
        .map(|(index, d)| {
            let best = d
                .results
                .iter()
                .max_by(|a, b| a.hypothesis.score.total_cmp(&b.hypothesis.score));
            let class = best.map_or("", |r| r.hypothesis.class_id.as_str());
            let color = match class {
                "person" => [1., 0.08, 0.25],
                "car" => [0.2, 1., 0.15],
                _ => [0.3, 0.5, 1.],
            };
            SceneBox {
                id: object_id(&d.id, index),
                center: d.bbox.center.position.map(|v| v as f32),
                orientation: d.bbox.center.orientation.map(|v| v as f32),
                size: d.bbox.size.map(|v| v as f32),
                color,
            }
        })
        .collect();
    let Ok(overlay) = SceneOverlay::new(vec![], boxes) else {
        return u32::MAX;
    };
    let Ok(output) = wire::encode_boxes(&overlay) else {
        return u32::MAX;
    };
    // SAFETY: the validated bounded snapshot fits the fixed output buffer.
    unsafe {
        std::ptr::copy_nonoverlapping(output.as_ptr(), output_ptr(), output.len());
    }
    overlay.boxes().len() as u32
}
