//! Path demo using the common ABI 3 guest SDK; no CDR parsing or GPU calls.
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

#[unsafe(no_mangle)]
pub extern "C" fn update(bytes: u32) -> u32 {
    if bytes as usize > INPUT_BYTES {
        return u32::MAX;
    }
    // SAFETY: bounded guest scratch; exports are invoked serially by the host.
    let bytes = unsafe { std::slice::from_raw_parts(input_ptr(), bytes as usize) };
    let Ok(DecodedInput::Path(path)) = wire::decode_input(bytes) else {
        return u32::MAX;
    };
    let stride = (path.points.len() / 32).max(1);
    let boxes = path
        .points
        .iter()
        .enumerate()
        .step_by(stride)
        .take(32)
        .map(|(index, point)| {
            let half_yaw = index as f32 * 0.05;
            SceneBox {
                id: SceneObjectId(index as u64),
                center: [point[0] as f32, point[1] as f32, point[2] as f32 + 0.25],
                orientation: [0., 0., half_yaw.sin(), half_yaw.cos()],
                size: [0.3, 0.3, 0.5],
                color: if index % 2 == 0 {
                    [1., 0.08, 0.25]
                } else {
                    [0.2, 1., 0.15]
                },
            }
        })
        .collect();
    let Ok(overlay) = SceneOverlay::new(vec![], boxes) else {
        return u32::MAX;
    };
    let Ok(output) = wire::encode_boxes(&overlay) else {
        return u32::MAX;
    };
    // SAFETY: SceneOverlay caps at 256 boxes, fitting the fixed output buffer.
    unsafe {
        std::ptr::copy_nonoverlapping(output.as_ptr(), output_ptr(), output.len());
    }
    overlay.boxes().len() as u32
}
