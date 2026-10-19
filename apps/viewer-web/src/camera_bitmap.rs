//! Browser camera pixel work; source coordinates remain independent of output pixels.
/// While playing, choose the pending focus frame first, then rotate backgrounds.
/// A completed result still occupies its slot until presentation drains it.
#[cfg(any(target_arch = "wasm32", test))]
fn next_camera(
    pending: impl Iterator<Item = viewer_core::CameraId> + Clone,
    occupied: impl Iterator<Item = viewer_core::CameraId> + Clone,
    last_background: Option<viewer_core::CameraId>,
    focus: Option<viewer_core::CameraId>,
) -> Option<viewer_core::CameraId> {
    if occupied.clone().count() >= 2 {
        return None;
    }
    if let Some(focus) = focus
        && pending.clone().any(|id| id == focus)
        && !occupied.clone().any(|id| id == focus)
    {
        return Some(focus);
    }
    let mut candidates =
        pending.filter(|id| Some(*id) != focus && !occupied.clone().any(|active| active == *id));
    candidates
        .clone()
        .find(|id| last_background.is_none_or(|last| *id > last))
        .or_else(|| candidates.next())
}

fn image_size(source: (u32, u32), target: (u32, u32)) -> (u32, u32) {
    let ratio = (target.0.max(1) as f64 / source.0 as f64)
        .min(target.1.max(1) as f64 / source.1 as f64)
        .min(1.0);
    (
        (source.0 as f64 * ratio).round().max(1.0) as u32,
        (source.1 as f64 * ratio).round().max(1.0) as u32,
    )
}
fn source_size(jpeg: &[u8]) -> Result<(u32, u32), String> {
    if jpeg.len() > 32 * 1024 * 1024 {
        return Err("camera JPEG exceeds 32 MiB".into());
    }
    let size = viewer_renderer::jpeg_dimensions(jpeg).map_err(|e| e.to_string())?;
    if size.0 == 0 || size.1 == 0 || u64::from(size.0) * u64::from(size.1) > 32 * 1024 * 1024 {
        return Err("camera JPEG exceeds 32 megapixel source limit".into());
    }
    Ok(size)
}

#[cfg(target_arch = "wasm32")]
mod browser {
    use super::{image_size, source_size};
    use bytes::Bytes;
    use js_sys::{Date, Promise};
    use std::{
        cell::RefCell,
        collections::{BTreeMap, BTreeSet},
        rc::Rc,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };
    use viewer_core::{ArrivalTime, CameraFrame, CameraFrameMetadata, CameraId};
    use wasm_bindgen::{JsCast, prelude::*};
    use wasm_bindgen_futures::{JsFuture, spawn_local};
    use web_sys::ImageBitmap;

    #[wasm_bindgen(module = "/camera_bitmap.js")]
    extern "C" {
        #[wasm_bindgen(js_name = decodeCameraBitmap)]
        fn decode_bitmap(bytes: &[u8], width: u32, height: u32) -> Promise;
    }
    struct EncodedImage {
        data: Vec<u8>,
        bytes: Arc<AtomicUsize>,
    }
    impl AsRef<[u8]> for EncodedImage {
        fn as_ref(&self) -> &[u8] {
            &self.data
        }
    }
    impl Drop for EncodedImage {
        fn drop(&mut self) {
            self.bytes.fetch_sub(self.data.len(), Ordering::Relaxed);
        }
    }
    pub(crate) struct Bitmap(pub(crate) ImageBitmap);
    impl Drop for Bitmap {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    pub(crate) struct CompletedImage {
        pub(crate) frame: CameraFrameMetadata,
        pub(crate) source_size: (u32, u32),
        pub(crate) pixel_size: (u32, u32),
        pub(crate) image: Result<Bitmap, String>,
        pub(crate) decode_time: Duration,
    }
    struct Request {
        frame: CameraFrame,
        source: (u32, u32),
        pixels: (u32, u32),
    }
    #[derive(Default)]
    struct State {
        generation: u64,
        active: BTreeSet<CameraId>,
        pending: BTreeMap<CameraId, Request>,
        ready: Vec<CompletedImage>,
        submitted: BTreeMap<CameraId, (ArrivalTime, (u32, u32))>,
        last_background: Option<CameraId>,
    }
    #[derive(Default)]
    pub(crate) struct CameraBitmapQueue {
        state: Rc<RefCell<State>>,
        encoded_bytes: Arc<AtomicUsize>,
    }
    impl CameraBitmapQueue {
        pub(crate) fn reset(&mut self) {
            let mut state = self.state.borrow_mut();
            state.generation = state.generation.wrapping_add(1);
            state.pending.clear();
            state.ready.clear();
            state.submitted.clear();
            // Old in-flight tasks still count toward the two-worker limit.
        }
        pub(crate) fn submit(
            &mut self,
            frame: &CameraFrame,
            target: (u32, u32),
        ) -> Result<(), String> {
            let mut state = self.state.borrow_mut();
            if !state.submitted.contains_key(&frame.camera_id) && state.submitted.len() >= 32 {
                return Err("camera pixel queue exceeds 32 camera identities".into());
            }
            let source = source_size(&frame.jpeg)?;
            let pixels = image_size(source, target);
            let key = (frame.arrival_time, pixels);
            if state.submitted.get(&frame.camera_id) == Some(&key) {
                return Ok(());
            }
            if frame.frame_id.len() > 4096 {
                return Err("camera frame identifier exceeds 4096 bytes".into());
            }
            state.pending.remove(&frame.camera_id);
            let len = frame.jpeg.len();
            if self
                .encoded_bytes
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |bytes| {
                    bytes.checked_add(len).filter(|n| *n <= 64 * 1024 * 1024)
                })
                .is_err()
            {
                return Ok(());
            }
            let mut data = Vec::new();
            if let Err(e) = data.try_reserve_exact(len) {
                self.encoded_bytes.fetch_sub(len, Ordering::Relaxed);
                return Err(e.to_string());
            }
            data.extend_from_slice(&frame.jpeg);
            let mut frame = frame.clone();
            frame.jpeg = Bytes::from_owner(EncodedImage {
                data,
                bytes: Arc::clone(&self.encoded_bytes),
            });
            state.submitted.insert(frame.camera_id, key);
            state.pending.insert(
                frame.camera_id,
                Request {
                    frame,
                    source,
                    pixels,
                },
            );
            Ok(())
        }
        pub(crate) fn drain(&mut self) -> Vec<CompletedImage> {
            std::mem::take(&mut self.state.borrow_mut().ready)
        }
        pub(crate) fn start_pending(&mut self, focus: Option<CameraId>) {
            loop {
                let mut state = self.state.borrow_mut();
                let id = super::next_camera(
                    state.pending.keys().copied(),
                    state
                        .active
                        .iter()
                        .copied()
                        .chain(state.ready.iter().map(|image| image.frame.camera_id)),
                    state.last_background,
                    focus,
                );
                let Some(id) = id else {
                    return;
                };
                let request = state.pending.remove(&id).unwrap();
                state.active.insert(id);
                if Some(id) != focus {
                    state.last_background = Some(id);
                }
                let generation = state.generation;
                drop(state);
                let state = Rc::clone(&self.state);
                spawn_local(async move {
                    let started = Date::now();
                    let image = JsFuture::from(decode_bitmap(
                        &request.frame.jpeg,
                        request.pixels.0,
                        request.pixels.1,
                    ))
                    .await
                    .map_err(|e| format!("camera bitmap decode: {e:?}"))
                    .and_then(|v| {
                        v.dyn_into::<ImageBitmap>()
                            .map(Bitmap)
                            .map_err(|_| "camera worker returned invalid bitmap".into())
                    });
                    let mut state = state.borrow_mut();
                    state.active.remove(&id);
                    if state.generation == generation {
                        state.ready.push(CompletedImage {
                            frame: request.frame.into(),
                            source_size: request.source,
                            pixel_size: request.pixels,
                            image,
                            decode_time: Duration::from_secs_f64(
                                ((Date::now() - started).max(0.0)) / 1000.0,
                            ),
                        });
                    }
                    // A stale bitmap closes when image drops here.
                });
            }
        }
    }
    #[cfg(feature = "jpeg-benchmark")]
    #[wasm_bindgen(
        inline_js = "export function cameraTestTick() { return new Promise(r => setTimeout(r, 1)); }"
    )]
    extern "C" {
        fn cameraTestTick() -> Promise;
    }

    #[cfg(feature = "jpeg-benchmark")]
    #[wasm_bindgen]
    pub async fn check_camera_bitmap_queue(bytes: Vec<u8>) -> Result<(), JsValue> {
        let mut queue = CameraBitmapQueue::default();
        let mut frame = CameraFrame {
            camera_id: CameraId(0),
            arrival_time: ArrivalTime(1),
            measurement_time: viewer_core::MeasurementTime(0),
            frame_id: "camera_optical".into(),
            jpeg: bytes.into(),
        };
        queue
            .submit(&frame, (1920, 1080))
            .map_err(|e| JsValue::from_str(&e))?;
        queue.start_pending(None);
        for time in 2..50 {
            frame.arrival_time = ArrivalTime(time);
            queue
                .submit(&frame, (1920, 1080))
                .map_err(|e| JsValue::from_str(&e))?;
        }
        if queue.state.borrow().pending.len() != 1 {
            return Err(JsValue::from_str("pending coalescing failed"));
        }
        queue.reset();
        frame.arrival_time = ArrivalTime(-10);
        queue
            .submit(&frame, (1920, 1080))
            .map_err(|e| JsValue::from_str(&e))?;
        frame.camera_id = CameraId(1);
        queue
            .submit(&frame, (160, 120))
            .map_err(|e| JsValue::from_str(&e))?;
        queue.start_pending(Some(CameraId(1)));
        if queue.state.borrow().active.len() != 2
            || !queue.state.borrow().active.contains(&CameraId(1))
            || queue.state.borrow().last_background != Some(CameraId(0))
        {
            return Err(JsValue::from_str(
                "focus dispatch lost the stale job or background rotation",
            ));
        }
        let mut completed = BTreeSet::new();
        let started = Date::now();
        while completed.len() < 2 {
            queue.start_pending(Some(CameraId(1)));
            if queue.state.borrow().active.len() + queue.state.borrow().ready.len() > 2 {
                return Err(JsValue::from_str("worker result limit exceeded"));
            }
            JsFuture::from(cameraTestTick()).await?;
            for result in queue.drain() {
                if result.frame.arrival_time != ArrivalTime(-10) {
                    return Err(JsValue::from_str("stale result after seek"));
                }
                let bitmap = result.image.map_err(|e| JsValue::from_str(&e))?;
                if (bitmap.0.width(), bitmap.0.height()) != result.pixel_size {
                    return Err(JsValue::from_str("resize dimensions"));
                }
                completed.insert(result.frame.camera_id);
            }
            if Date::now() - started > 10000. {
                return Err(JsValue::from_str("worker queue timed out"));
            }
        }
        if queue.encoded_bytes.load(Ordering::Relaxed) != 0 {
            return Err(JsValue::from_str("encoded budget leaked"));
        }
        // Saturate the encoded budget with valid padded JPEGs, then retain displayed
        // metadata across completion and verify that another camera can still advance.
        queue.reset();
        let mut padded = frame.jpeg.to_vec();
        padded.resize(32 * 1024 * 1024, 0);
        frame.jpeg = padded.into();
        frame.arrival_time = ArrivalTime(123456789);
        frame.measurement_time = viewer_core::MeasurementTime(987654321);
        for id in 0..2 {
            frame.camera_id = CameraId(id);
            queue
                .submit(&frame, (160, 120))
                .map_err(|e| JsValue::from_str(&e))?;
        }
        frame.camera_id = CameraId(2);
        queue
            .submit(&frame, (160, 120))
            .map_err(|e| JsValue::from_str(&e))?;
        if queue.encoded_bytes.load(Ordering::Relaxed) != 64 * 1024 * 1024
            || queue.state.borrow().pending.len() != 2
            || queue.state.borrow().submitted.contains_key(&CameraId(2))
        {
            return Err(JsValue::from_str("encoded backpressure failed"));
        }
        let mut displayed = Vec::new();
        let started = Date::now();
        while displayed.len() < 2 {
            queue.start_pending(None);
            JsFuture::from(cameraTestTick()).await?;
            for result in queue.drain() {
                result.image.map_err(|e| JsValue::from_str(&e))?;
                if result.frame.arrival_time != frame.arrival_time
                    || result.frame.measurement_time != frame.measurement_time
                    || result.frame.frame_id != frame.frame_id
                {
                    return Err(JsValue::from_str("displayed metadata mismatch"));
                }
                displayed.push(result.frame);
            }
            if Date::now() - started > 20000. {
                return Err(JsValue::from_str("saturated queue timed out"));
            }
        }
        if queue.encoded_bytes.load(Ordering::Relaxed) != 0 {
            return Err(JsValue::from_str(
                "displayed metadata retained encoded budget",
            ));
        }
        queue
            .submit(&frame, (160, 120))
            .map_err(|e| JsValue::from_str(&e))?;
        let started = Date::now();
        loop {
            queue.start_pending(None);
            JsFuture::from(cameraTestTick()).await?;
            let ready = queue.drain();
            if let Some(result) = ready.into_iter().next() {
                result.image.map_err(|e| JsValue::from_str(&e))?;
                if result.frame.camera_id != CameraId(2)
                    || queue.encoded_bytes.load(Ordering::Relaxed) != 0
                    || displayed.len() != 2
                {
                    return Err(JsValue::from_str(
                        "camera did not recover from backpressure",
                    ));
                }
                break;
            }
            if Date::now() - started > 20000. {
                return Err(JsValue::from_str("post-budget update timed out"));
            }
        }
        Ok(())
    }

    pub(crate) fn target_size(source: (u32, u32), target: (u32, u32)) -> (u32, u32) {
        image_size(source, target)
    }
}
#[cfg(target_arch = "wasm32")]
pub(crate) use browser::{CameraBitmapQueue, target_size};

#[cfg(test)]
mod tests {
    use super::*;
    use viewer_core::CameraId;

    fn pick(
        pending: &[u16],
        occupied: &[u16],
        last: Option<u16>,
        focus: Option<u16>,
    ) -> Option<u16> {
        next_camera(
            pending.iter().copied().map(CameraId),
            occupied.iter().copied().map(CameraId),
            last.map(CameraId),
            focus.map(CameraId),
        )
        .map(|id| id.0)
    }

    #[test]
    fn playing_focus_has_priority_without_starving_background_rotation() {
        assert_eq!(pick(&[0, 1, 2], &[], Some(1), Some(0)), Some(0));
        // If focus has no pending frame, lend all available capacity to background.
        assert_eq!(pick(&[1, 2], &[1], None, Some(0)), Some(2));
        assert_eq!(pick(&[0, 1, 2], &[1], None, Some(0)), Some(0));
        assert_eq!(pick(&[0, 1, 2], &[0], Some(1), Some(0)), Some(2));
        assert_eq!(pick(&[0, 1, 2], &[0], Some(2), Some(0)), Some(1));
        assert_eq!(pick(&[0, 1, 2], &[0, 1], None, Some(0)), None);
    }

    #[test]
    fn switching_focus_waits_for_old_jobs_and_keeps_background_order() {
        assert_eq!(pick(&[0, 1, 2], &[0, 1], Some(1), Some(2)), None);
        assert_eq!(pick(&[0, 1, 2], &[0], Some(1), Some(2)), Some(2));
        assert_eq!(pick(&[0, 1, 2, 3], &[2], Some(1), Some(2)), Some(3));
    }

    #[test]
    fn paused_restore_and_no_focus_use_both_workers() {
        assert_eq!(pick(&[0, 1, 2], &[1], Some(1), None), Some(2));
        assert_eq!(pick(&[0, 1, 2], &[2], Some(2), None), Some(0));
        assert_eq!(pick(&[], &[], None, Some(0)), None);
        assert_eq!(pick(&[0], &[0], None, Some(0)), None);
    }

    #[test]
    fn fhd_and_thumbnails_preserve_source_aspect_without_upscale() {
        assert_eq!(image_size((3840, 2160), (1920, 1080)), (1920, 1080));
        assert_eq!(image_size((3840, 2160), (160, 120)), (160, 90));
        assert_eq!(image_size((641, 479), (160, 120)), (160, 120));
        assert_eq!(image_size((320, 240), (1920, 1080)), (320, 240));
        assert_eq!(image_size((3840, 2160), (0, 0)), (1, 1));
    }
    #[test]
    fn damaged_and_oversized_jpeg_headers_are_rejected_before_browser_decode() {
        assert!(source_size(b"not jpeg").is_err());
        let oversized = [
            0xff, 0xd8, 0xff, 0xc0, 0, 11, 8, 0xff, 0xff, 0xff, 0xff, 1, 1, 0x11, 0,
        ];
        assert!(
            source_size(&oversized)
                .unwrap_err()
                .contains("32 megapixel")
        );
        assert!(source_size(&vec![0; 32 * 1024 * 1024 + 1]).is_err());
    }
}
