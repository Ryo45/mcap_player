//! path_positions_v1 guest. Wire records only; never decodes CDR or touches a GPU.
const INPUT_BYTES: usize = 16 + 1024 * 24;
const OUTPUT_BYTES: usize = 256 * 60;
static mut INPUT: [u8; INPUT_BYTES] = [0; INPUT_BYTES];
static mut OUTPUT: [u8; OUTPUT_BYTES] = [0; OUTPUT_BYTES];

#[unsafe(no_mangle)]
pub extern "C" fn api_version() -> u32 {
    2
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
pub extern "C" fn update(count: u32) -> u32 {
    if count > 1024 {
        return u32::MAX;
    }
    // At most 32 boxes; class-like alternating colors are selected by guest code.
    let mut objects = 0usize;
    let stride = (count as usize / 32).max(1);
    for index in (0..count as usize).step_by(stride).take(32) {
        let mut center = [0f32; 3];
        for (axis, value) in center.iter_mut().enumerate() {
            let offset = 16 + index * 24 + axis * 8;
            let mut bytes = [0; 8];
            // SAFETY: count <= 1024 bounds the complete input; guest exports run serially.
            unsafe {
                std::ptr::copy_nonoverlapping(input_ptr().add(offset), bytes.as_mut_ptr(), 8);
            }
            *value = f64::from_le_bytes(bytes) as f32;
        }
        let color = if index % 2 == 0 {
            [1., 0.08, 0.25]
        } else {
            [0.2, 1., 0.15]
        };
        let mut record = [0u8; 60];
        record[..8].copy_from_slice(&(index as u64).to_le_bytes());
        let mut field = 8;
        // The guest emits one oriented box, not 12 anonymous edges.
        // Box bottom lies on the input position; the metric centre is 0.25 m above it.
        let half_yaw = index as f32 * 0.05;
        let values = [
            center[0],
            center[1],
            center[2] + 0.25,
            0.,
            0.,
            half_yaw.sin(),
            half_yaw.cos(),
            0.3,
            0.3,
            0.5,
            color[0],
            color[1],
            color[2],
        ];
        for v in values {
            record[field..field + 4].copy_from_slice(&v.to_le_bytes());
            field += 4;
        }
        // SAFETY: at most 32 bounded records; guest exports run serially.
        unsafe {
            std::ptr::copy_nonoverlapping(record.as_ptr(), output_ptr().add(objects * 60), 60);
        }
        objects += 1;
    }
    objects as u32
}
