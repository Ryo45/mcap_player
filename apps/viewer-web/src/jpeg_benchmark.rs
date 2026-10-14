//! Opt-in browser comparison of the previous production RGBA/Canvas path.
use wasm_bindgen::{Clamped, JsCast, prelude::*};
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, ImageData};
#[wasm_bindgen]
pub fn benchmark_jpeg_canvas(bytes: &[u8], canvas: HtmlCanvasElement) -> Result<(), JsValue> {
    let image =
        viewer_renderer::decode_jpeg(bytes).map_err(|e| JsValue::from_str(&e.to_string()))?;
    if canvas.width() != image.width {
        canvas.set_width(image.width);
    }
    if canvas.height() != image.height {
        canvas.set_height(image.height);
    }
    let context: CanvasRenderingContext2d = canvas
        .get_context("2d")?
        .ok_or_else(|| JsValue::from_str("canvas context"))?
        .dyn_into()?;
    let data = ImageData::new_with_u8_clamped_array_and_sh(
        Clamped(&image.rgba),
        image.width,
        image.height,
    )?;
    context.put_image_data(&data, 0.0, 0.0)
}
#[wasm_bindgen]
pub fn benchmark_wasm_bytes() -> u32 {
    wasm_bindgen::memory()
        .unchecked_into::<js_sys::WebAssembly::Memory>()
        .buffer()
        .unchecked_into::<js_sys::ArrayBuffer>()
        .byte_length()
}
