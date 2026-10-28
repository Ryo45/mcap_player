//! path_positions_v1 guest. Wire records only; never decodes CDR or touches a GPU.
const INPUT_BYTES: usize = 16 + 1024 * 24;
const OUTPUT_BYTES: usize = 4096 * 40;
static mut INPUT: [u8; INPUT_BYTES] = [0; INPUT_BYTES];
static mut OUTPUT: [u8; OUTPUT_BYTES] = [0; OUTPUT_BYTES];

#[unsafe(no_mangle)]
pub extern "C" fn api_version() -> u32 {
    1
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
    let mut lines = 0usize;
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
        let corners: [[f32; 3]; 8] = std::array::from_fn(|i| {
            [
                center[0] + if i & 1 == 0 { -0.15 } else { 0.15 },
                center[1] + if i & 2 == 0 { -0.15 } else { 0.15 },
                center[2] + if i & 4 == 0 { 0. } else { 0.5 },
            ]
        });
        for a in 0..8 {
            for bit in [1, 2, 4] {
                let b = a ^ bit;
                if a >= b {
                    continue;
                }
                let offset = lines * 40;
                let mut record = [0u8; 40];
                record[..4].copy_from_slice(&(lines as u32).to_le_bytes());
                let mut field = 4;
                for v in corners[a].into_iter().chain(corners[b]).chain(color) {
                    record[field..field + 4].copy_from_slice(&v.to_le_bytes());
                    field += 4;
                }
                // SAFETY: at most 32 * 12 records fit the fixed output buffer.
                unsafe {
                    std::ptr::copy_nonoverlapping(record.as_ptr(), output_ptr().add(offset), 40);
                }
                lines += 1;
            }
        }
    }
    lines as u32
}
